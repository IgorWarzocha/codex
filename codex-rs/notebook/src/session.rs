use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::DEFAULT_EXEC_YIELD_TIME_MS;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use codex_code_mode_protocol::normalize_code_mode_identifier;
use codex_notebook_kernel::ExecutionResult;
use codex_notebook_kernel::ExecutionStatus;
use codex_notebook_kernel::Kernel;
use codex_notebook_kernel::KernelOptions;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bridge::Bridge;
use crate::cell::Cell;
use crate::cell::text_item;

/// One unsandboxed Deno Jupyter kernel per Codex thread. No TypeScript host controller.
pub struct DenoNotebookSessionProvider {
    deno_program: PathBuf,
    cwd: PathBuf,
}

impl DenoNotebookSessionProvider {
    pub fn new(deno_program: PathBuf, cwd: PathBuf) -> Self {
        Self { deno_program, cwd }
    }
}

impl CodeModeSessionProvider for DenoNotebookSessionProvider {
    fn availability(&self) -> Result<(), String> {
        let program_exists =
            if self.deno_program.components().count() > 1 || self.deno_program.is_absolute() {
                self.deno_program.is_file()
            } else {
                std::env::var_os("PATH").is_some_and(|path| {
                    std::env::split_paths(&path).any(|dir| dir.join(&self.deno_program).is_file())
                })
            };
        if !program_exists {
            return Err(format!(
                "Deno program not found: {}",
                self.deno_program.display()
            ));
        }
        if !self.cwd.is_dir() {
            return Err(format!(
                "notebook cwd is not a directory: {}",
                self.cwd.display()
            ));
        }
        Ok(())
    }

    fn create_session(&self) -> CodeModeSessionProviderFuture<'_> {
        Box::pin(async move {
            self.availability()?;
            let options = KernelOptions {
                deno: self.deno_program.clone(),
                cwd: Some(self.cwd.clone()),
                // Foreground observation deadlines yield, they do not kill the kernel.
                execute_timeout: Duration::from_secs(24 * 60 * 60),
                ..KernelOptions::default()
            };
            let mut kernel = Kernel::start(options)
                .await
                .map_err(|error| error.to_string())?;
            let registry = Arc::new(Registry::default());
            let mut bridge = match Bridge::start(registry.clone()).await {
                Ok(bridge) => bridge,
                Err(error) => {
                    let _ = kernel.shutdown().await;
                    return Err(error);
                }
            };
            let bootstrap = include_str!("bootstrap.js")
                .replace(
                    "__ENDPOINT__",
                    &serde_json::to_string(&bridge.endpoint).map_err(|e| e.to_string())?,
                )
                .replace(
                    "__CREDENTIAL__",
                    &serde_json::to_string(&bridge.credential).map_err(|e| e.to_string())?,
                );
            let startup = kernel
                .execute(&bootstrap)
                .await
                .map_err(|e| e.to_string())
                .and_then(|result| match result_error(&result) {
                    Some(error) => Err(error),
                    None => Ok(()),
                });
            if let Err(error) = startup {
                let _ = bridge.shutdown().await;
                let _ = kernel.shutdown().await;
                return Err(format!("initialize notebook globals: {error}"));
            }
            let cancellation = CancellationToken::new();
            let (commands, incoming) = mpsc::unbounded_channel();
            let worker_registry = registry.clone();
            let worker_cancel = cancellation.clone();
            let worker = tokio::spawn(async move {
                run_session(kernel, worker_registry, worker_cancel, incoming).await
            });
            Ok(Arc::new(Session {
                registry,
                commands,
                resources: Mutex::new(Some(Resources { bridge, worker })),
                shutdown_gate: Semaphore::new(1),
                cancellation,
            }) as Arc<dyn CodeModeSession>)
        })
    }
}

#[derive(Default)]
pub(crate) struct Registry {
    state: Mutex<SessionState>,
}

#[derive(Default)]
struct SessionState {
    cells: HashMap<CellId, Arc<Cell>>,
    active: Option<CellId>,
    closed: bool,
    broken: bool,
}

impl Registry {
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
    commands: mpsc::UnboundedSender<CellCommand>,
    resources: Mutex<Option<Resources>>,
    shutdown_gate: Semaphore,
    cancellation: CancellationToken,
}

struct CellCommand {
    cell: Arc<Cell>,
    source: String,
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
                "globalThis.__codexNotebook.begin({}, {});\n{}\n;await globalThis.__codexNotebook.flush({});",
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
                if state.broken {
                    return Err("Deno kernel was terminated. Start a new thread to create a fresh notebook session".to_string());
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
                    .send(CellCommand {
                        cell: cell.clone(),
                        source,
                    })
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
        Box::pin(async move {
            self.cancellation.cancel();
            self.registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .closed = true;
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
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

struct WorkerGuard {
    registry: Arc<Registry>,
    cancellation: CancellationToken,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let cells = {
            let mut state = self
                .registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.broken = true;
            state.active = None;
            state.cells.values().cloned().collect::<Vec<_>>()
        };
        for cell in cells {
            cell.finish(Some("notebook worker stopped".to_string()));
        }
    }
}

async fn run_session(
    mut kernel: Kernel,
    registry: Arc<Registry>,
    cancellation: CancellationToken,
    mut commands: mpsc::UnboundedReceiver<CellCommand>,
) -> Result<(), String> {
    let _guard = WorkerGuard {
        registry: registry.clone(),
        cancellation: cancellation.clone(),
    };
    loop {
        let command = tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            command = commands.recv() => match command { Some(command) => command, None => break },
        };
        run_cell(&mut kernel, &registry, command.cell, command.source).await;
        if registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .broken
        {
            break;
        }
    }
    kernel.shutdown().await.map_err(|error| error.to_string())
}

async fn run_cell(kernel: &mut Kernel, registry: &Registry, cell: Arc<Cell>, source: String) {
    let result = kernel
        .execute_streaming(&source, cell.cancellation.clone(), |output| {
            cell.push_kernel_output(output)
        })
        .await;
    let mut broken = result.is_err();
    let mut error = match result {
        Ok(result) => {
            if result.output_truncated {
                cell.push(text_item("[Deno Jupyter output truncated]"));
            }
            result_error(&result)
        }
        Err(error) => Some(error.to_string()),
    };
    // User code is not wrapped in a function or block: Deno's top-level bindings persist.
    // A separate flush request also drains synchronous text() calls before a JS exception.
    if !broken && error.is_some() && !cell.cancellation.is_cancelled() {
        let flush_source = format!(
            "await globalThis.__codexNotebook.flush({})",
            serde_json::json!(cell.id.as_str())
        );
        match kernel
            .execute_streaming(&flush_source, cell.cancellation.clone(), |output| {
                cell.push_kernel_output(output)
            })
            .await
        {
            Ok(result) => {
                if let Some(flush_error) = result_error(&result) {
                    error = Some(format!(
                        "{}\nOutput flush failed: {flush_error}",
                        error.unwrap_or_default()
                    ));
                }
            }
            Err(flush_error) => {
                broken = true;
                error = Some(format!(
                    "{}\nOutput flush failed: {flush_error}",
                    error.unwrap_or_default()
                ));
            }
        }
    }
    if cell.cancellation.is_cancelled() {
        broken = true;
    }
    if broken && let Err(shutdown_error) = kernel.shutdown().await {
        error = Some(format!(
            "{}\nKernel cleanup failed: {shutdown_error}",
            error.unwrap_or_default()
        ));
    }
    {
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = None;
        state.broken |= broken;
    }
    cell.finish(error);
}

fn result_error(result: &ExecutionResult) -> Option<String> {
    result
        .error
        .as_ref()
        .map(|error| format!("{}: {}", error.name, error.value))
        .or_else(|| {
            (result.status != ExecutionStatus::Ok)
                .then(|| format!("Deno execution {:?}", result.status))
        })
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
