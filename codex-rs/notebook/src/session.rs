mod execution;
mod provider;

pub(crate) use execution::result_error;
use execution::run_session;
pub use provider::DenoNotebookSessionProvider;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::DEFAULT_EXEC_YIELD_TIME_MS;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use codex_code_mode_protocol::normalize_code_mode_identifier;
use serde_json::Value;
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bridge::Bridge;
use crate::cell::Cell;
use crate::control::NotebookControlResult;
use crate::control::NotebookRequest;
use crate::lifecycle::Lifecycle;
use crate::lifecycle::status_result;

#[derive(Default)]
pub(crate) struct Registry {
    state: Mutex<SessionState>,
    no_cleanup: CancellationToken,
}

#[derive(Default)]
struct SessionState {
    cells: HashMap<CellId, Arc<Cell>>,
    active: Option<CellId>,
    closed: bool,
    shutdown_mode: ShutdownMode,
    broken: bool,
    managing: bool,
    status: Value,
    checkpoint: Value,
    persistence_error: Option<String>,
    completed_cells: u64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ShutdownMode {
    #[default]
    Immediate,
    Graceful,
}

impl Registry {
    fn update(&self, lifecycle: &Lifecycle) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.broken = lifecycle.kernel.is_none();
        state.status = lifecycle.status.clone();
        state.status["userCells"] = json!(state.completed_cells);
        state.checkpoint = lifecycle.checkpoint_details.clone();
        state.persistence_error = lifecycle.persistence_error.clone();
    }
    pub(crate) fn running_cell(&self, id: &CellId) -> Result<Arc<Cell>, String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cell = state
            .cells
            .get(id)
            .filter(|cell| !state.closed && cell.is_running() && !cell.cancellation.is_cancelled());
        cell.cloned()
            .ok_or_else(|| "notebook cell is unknown or closed".to_string())
    }

    async fn observe(
        &self,
        cell: Arc<Cell>,
        duration: Duration,
        preempt: Option<CancellationToken>,
    ) -> WaitOutcome {
        match cell.observe(duration, preempt).await {
            Some(response) => {
                if !matches!(response, RuntimeResponse::Yielded { .. }) {
                    self.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cells
                        .remove(&cell.id);
                }
                WaitOutcome::LiveCell(response)
            }
            None => missing(cell.id.clone()),
        }
    }
}

struct Session {
    registry: Arc<Registry>,
    commands: mpsc::UnboundedSender<Command>,
    resources: Mutex<Option<Resources>>,
    shutdown_gate: Semaphore,
    cancellation: CancellationToken,
}

struct CellCommand {
    cell: Arc<Cell>,
    source: String,
    user_source: String,
}

enum Command {
    Execute(CellCommand),
    Control(
        NotebookRequest,
        oneshot::Sender<Result<NotebookControlResult, String>>,
    ),
}

impl Session {
    async fn control(&self, request: NotebookRequest) -> Result<NotebookControlResult, String> {
        let response = {
            let mut state = self
                .registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed {
                return Err("notebook session is shut down".to_string());
            }
            if let NotebookRequest::Status { query } = &request
                && (state.active.is_some() || state.managing)
            {
                let mut result = status_result(
                    &state.status,
                    &state.checkpoint,
                    state.persistence_error.as_deref(),
                    query.as_deref(),
                    state.active.is_some(),
                );
                if state.broken {
                    result.details["state"] = json!("invalidated");
                    result
                        .message
                        .push_str("\nKernel unavailable. The next operation will restore the last completed checkpoint");
                }
                return Ok(result);
            }
            if state.managing {
                return Err("notebook management is already in progress".to_string());
            }
            let recovery = matches!(request, NotebookRequest::Restart | NotebookRequest::Reset);
            if state.active.is_some() && !recovery {
                return Err(
                    "cannot mutate notebook while exec is active. Wait or terminate it first"
                        .to_string(),
                );
            }
            if recovery && let Some(cell) = state.active.as_ref().and_then(|id| state.cells.get(id))
            {
                cell.cancellation.cancel();
            }
            let (reply, response) = oneshot::channel();
            state.managing = true;
            if self
                .commands
                .send(Command::Control(request, reply))
                .is_err()
            {
                state.managing = false;
                state.broken = true;
                return Err("notebook worker ended unexpectedly".to_string());
            }
            response
        };
        response
            .await
            .map_err(|_| "notebook management worker stopped".to_string())?
    }
}

impl Session {
    async fn shutdown_inner(&self, mode: ShutdownMode) -> Result<(), String> {
        if mode == ShutdownMode::Immediate {
            self.registry.no_cleanup.cancel();
        }
        {
            let mut state = self
                .registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.closed {
                state.shutdown_mode = mode;
                state.closed = true;
            }
        }
        // Close admission before cancellation. The worker must never restore a
        // runtime once the session has begun shutting down.
        self.cancellation.cancel();
        let _shutdown = self
            .shutdown_gate
            .acquire()
            .await
            .map_err(|e| e.to_string())?;
        let resources = self
            .resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match resources {
            Some(resources) => resources.shutdown().await,
            None => Ok(()),
        }
    }
}

/// Owns the handles even if the caller drops an in-progress shutdown future.
struct Resources {
    bridge: Bridge,
    worker: JoinHandle<Result<(), String>>,
}

impl Resources {
    async fn shutdown(mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        match tokio::time::timeout(Duration::from_secs(8), &mut self.worker).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => errors.push(error),
            Ok(Err(error)) => errors.push(format!("notebook worker failed: {error}")),
            Err(_) => {
                self.worker.abort();
                let _ = (&mut self.worker).await;
                errors.push("notebook worker shutdown timed out".to_string());
            }
        }
        if let Err(error) = self.bridge.shutdown().await {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

impl CodeModeSession for Session {
    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(async move {
            let started = Instant::now();
            let mut definitions = request.enabled_tools;
            let mut tools = HashMap::new();
            for definition in &mut definitions {
                definition.name = normalize_code_mode_identifier(&definition.name);
                if tools
                    .insert(definition.name.clone(), definition.clone())
                    .is_some()
                {
                    return Err(format!(
                        "duplicate normalized notebook tool name: {}",
                        definition.name
                    ));
                }
            }
            let id = CellId::new(Uuid::new_v4().to_string());
            let source = format!(
                "if (typeof globalThis.__codexNotebook?.begin !== 'function') throw new Error('Notebook runtime bootstrap unavailable: __codexNotebook.begin');\nawait globalThis.__codexNotebook.begin({}, {});\n{}\n;if (typeof globalThis.__codexNotebook?.flush !== 'function') throw new Error('Notebook runtime bootstrap unavailable: __codexNotebook.flush');\nawait globalThis.__codexNotebook.flush({});",
                serde_json::to_string(id.as_str()).map_err(|e| e.to_string())?,
                serde_json::to_string(&definitions).map_err(|e| e.to_string())?,
                request.source,
                serde_json::to_string(id.as_str()).map_err(|e| e.to_string())?
            );
            let cell = Arc::new(Cell::new(
                id.clone(),
                request.tool_call_id,
                delegate,
                tools,
                self.cancellation.child_token(),
            ));
            {
                let mut state = self
                    .registry
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.closed {
                    return Err("notebook session is shut down".to_string());
                }
                if state.managing {
                    return Err("notebook management is in progress".to_string());
                }
                if state.active.is_some() {
                    return Err(
                        "notebook already has an active cell. Use wait or terminate it before exec"
                            .to_string(),
                    );
                }
                if state.cells.len() >= 32 {
                    return Err(
                        "too many unobserved completed cells. Use wait to drain them".to_string(),
                    );
                }
                state.active = Some(id.clone());
                state.cells.insert(id.clone(), cell.clone());
                // Reservation and enqueue are synchronous, with no cancellation gap that
                // could leave a reserved cell without a kernel owner.
                if self
                    .commands
                    .send(Command::Execute(CellCommand {
                        cell: cell.clone(),
                        source,
                        user_source: request.source,
                    }))
                    .is_err()
                {
                    state.active = None;
                    state.broken = true;
                    drop(state);
                    cell.finish(Some("notebook worker ended unexpectedly".to_string()));
                    return Err("notebook worker ended unexpectedly".to_string());
                }
            }
            let registry = self.registry.clone();
            let duration =
                Duration::from_millis(request.yield_time_ms.unwrap_or(DEFAULT_EXEC_YIELD_TIME_MS));
            Ok(StartedCell::from_future(id, async move {
                let response =
                    RuntimeResponse::from(registry.observe(cell, duration, preempt).await);
                Ok(response.with_code_mode_host_duration(started.elapsed()))
            }))
        })
    }

    fn wait<'a>(
        &'a self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            let cell = self
                .registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cells
                .get(&request.cell_id)
                .cloned();
            match cell {
                Some(cell) => Ok(self
                    .registry
                    .observe(cell, Duration::from_millis(request.yield_time_ms), preempt)
                    .await),
                None => Ok(missing(request.cell_id)),
            }
        })
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            let cell = self
                .registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cells
                .get(&cell_id)
                .cloned();
            match cell {
                Some(cell) => {
                    if cell.is_running() {
                        cell.cancellation.cancel();
                    }
                    tokio::time::timeout(
                        Duration::from_secs(8),
                        self.registry.observe(cell, Duration::from_secs(8), None),
                    )
                    .await
                    .map_err(|_| "Deno notebook termination timed out".to_string())
                }
                None => Ok(missing(cell_id)),
            }
        })
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(self.shutdown_inner(ShutdownMode::Graceful))
    }

    fn shutdown_without_cleanup<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(self.shutdown_inner(ShutdownMode::Immediate))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

fn missing(cell_id: CellId) -> WaitOutcome {
    WaitOutcome::MissingCell(RuntimeResponse::Result {
        cell_id,
        content_items: Vec::new(),
        error_text: Some(
            "notebook cell is unknown or its terminal output was already observed".to_string(),
        ),
        code_mode_host_duration: Some(Duration::ZERO),
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
