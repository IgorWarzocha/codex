use std::sync::Arc;

use codex_notebook_kernel::ExecutionResult;
use codex_notebook_kernel::ExecutionStatus;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::CellCommand;
use super::Command;
use super::Registry;
use crate::cell::text_item;
use crate::journal::Journal;
use crate::lifecycle::Lifecycle;

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

pub(super) async fn run_session(
    mut lifecycle: Lifecycle,
    journal: Option<Journal>,
    registry: Arc<Registry>,
    cancellation: CancellationToken,
    mut commands: mpsc::UnboundedReceiver<Command>,
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
        match command {
            Command::Execute(command) => {
                run_cell(&mut lifecycle, journal.as_ref(), &registry, command).await
            }
            Command::Control(request, reply) => {
                let result = lifecycle.control(request).await;
                registry.update(&lifecycle);
                registry
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .managing = false;
                let _ = reply.send(result);
            }
        }
    }
    lifecycle.shutdown().await
}

async fn run_cell(
    lifecycle: &mut Lifecycle,
    journal: Option<&Journal>,
    registry: &Registry,
    command: CellCommand,
) {
    let CellCommand {
        cell,
        source,
        user_source,
    } = command;
    if let Some(journal) = journal {
        let journal = journal.clone();
        let (id, source) = (cell.id.as_str().to_string(), user_source.clone());
        let begin = tokio::task::spawn_blocking(move || journal.begin(&id, &source))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        if let Err(error) = begin {
            registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active = None;
            cell.finish(Some(format!(
                "notebook journal persistence failed before execution: {error}"
            )));
            return;
        }
    }
    let Some(kernel) = lifecycle.kernel.as_mut() else {
        registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active = None;
        cell.finish(Some(
            "kernel unavailable. Use notebook restart or reset".to_string(),
        ));
        return;
    };
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
    if !broken && !cell.cancellation.is_cancelled() {
        let flush_source = format!(
            "try {{ await globalThis.__codexNotebook.flush({0}); }} finally {{ globalThis.__codexNotebook.completed({0}); }}",
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
    if broken {
        if let Err(shutdown_error) = lifecycle.shutdown().await {
            append_error(
                &mut error,
                format!("Kernel cleanup failed: {shutdown_error}"),
            );
        }
    } else {
        let checkpoint = tokio::select! {
            biased;
            _ = cell.cancellation.cancelled() => Err("checkpoint interrupted by notebook cancellation".to_string()),
            result = lifecycle.checkpoint(&[]) => result.map(|_| ()),
        };
        if let Err(checkpoint_error) = checkpoint {
            append_error(
                &mut error,
                format!("Notebook persistence failed: {checkpoint_error}"),
            );
        }
        if cell.cancellation.is_cancelled() {
            let _ = lifecycle.shutdown().await;
        }
    }
    if let Some(journal) = journal {
        let journal = journal.clone();
        let (id, source, journal_error, items) = (
            cell.id.as_str().to_string(),
            user_source,
            error.clone(),
            cell.journal_items(),
        );
        let status = if cell.cancellation.is_cancelled() {
            "terminated"
        } else if error.is_some() {
            "error"
        } else {
            "ok"
        };
        let result = tokio::task::spawn_blocking(move || {
            journal.append(&id, &source, status, journal_error.as_deref(), &items)
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        if let Err(journal_error) = result {
            append_error(
                &mut error,
                format!("Notebook journal persistence failed: {journal_error}"),
            );
        }
    }
    registry.update(lifecycle);
    {
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = None;
        state.completed_cells += 1;
        state.status["userCells"] = json!(state.completed_cells);
    }
    cell.finish(error);
}

pub(crate) fn result_error(result: &ExecutionResult) -> Option<String> {
    if result.error.as_ref().is_some_and(|error| {
        error.name == "CodexNotebookExit" && error.value == "__CodexNotebookExit__"
    }) {
        return None;
    }
    result
        .error
        .as_ref()
        .map(|error| format!("{}: {}", error.name, error.value))
        .or_else(|| {
            (result.status != ExecutionStatus::Ok)
                .then(|| format!("Deno execution {:?}", result.status))
        })
}

fn append_error(error: &mut Option<String>, extra: String) {
    *error = Some(match error.take() {
        Some(error) => format!("{error}\n{extra}"),
        None => extra,
    });
}
