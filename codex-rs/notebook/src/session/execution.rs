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
use crate::lifecycle::RECOVERY_NOTICE;
use crate::lifecycle::RESTORED_NOTICE;
use crate::lifecycle::bootstrap_failure;

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
                run_cell(
                    &mut lifecycle,
                    journal.as_ref(),
                    &registry,
                    &cancellation,
                    command,
                )
                .await
            }
            Command::Control(request, reply) => {
                let result = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        let _ = lifecycle.shutdown().await;
                        Err("notebook session is shut down".to_string())
                    }
                    result = lifecycle.control(request) => result,
                };
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
    let graceful = registry
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .shutdown_mode
        == super::ShutdownMode::Graceful;
    if graceful {
        tokio::select! {
            biased;
            _ = registry.no_cleanup.cancelled() => lifecycle.shutdown().await,
            result = lifecycle.finish() => result,
        }
    } else {
        lifecycle.shutdown().await
    }
}

async fn run_cell(
    lifecycle: &mut Lifecycle,
    journal: Option<&Journal>,
    registry: &Registry,
    session_cancellation: &CancellationToken,
    command: CellCommand,
) {
    let CellCommand {
        cell,
        source,
        user_source,
    } = command;
    let ready = tokio::select! {
        biased;
        _ = cell.cancellation.cancelled() => Err("notebook execution cancelled before startup".to_string()),
        result = lifecycle.ensure() => result,
    };
    match ready {
        Ok(true) => cell.push(text_item(RESTORED_NOTICE)),
        Ok(false) => {}
        Err(error) => {
            let _ = lifecycle.shutdown().await;
            registry.update(lifecycle);
            registry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active = None;
            cell.finish(Some(error));
            return;
        }
    }
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
        cell.finish(Some(RECOVERY_NOTICE.to_string()));
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
            "if (typeof globalThis.__codexNotebook?.flush !== 'function' || typeof globalThis.__codexNotebook?.completed !== 'function') throw new Error('Notebook runtime bootstrap unavailable: __codexNotebook.finish'); try {{ await globalThis.__codexNotebook.flush({0}); }} finally {{ globalThis.__codexNotebook.completed({0}); }}",
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
    let bootstrap_lost = error.as_deref().is_some_and(bootstrap_failure);
    broken |= bootstrap_lost;
    // Import literals in failed or interrupted cells are not added to the known inventory.
    // This records use, not user consent, and never authorizes or blocks an import.
    if !broken
        && error.is_none()
        && let Err(inventory_error) = lifecycle.record_imports(&user_source).await
    {
        cell.push(text_item(format!(
            "Notebook npm inventory was not updated: {inventory_error}"
        )));
    }
    if broken {
        if let Err(shutdown_error) = lifecycle.shutdown().await {
            append_error(
                &mut error,
                format!("Kernel cleanup failed: {shutdown_error}"),
            );
        }
        if !cell.cancellation.is_cancelled()
            && !session_cancellation.is_cancelled()
            && !bootstrap_lost
        {
            let recovered = tokio::select! {
                biased;
                _ = cell.cancellation.cancelled() => Err("notebook recovery cancelled".to_string()),
                result = lifecycle.ensure() => result,
            };
            match recovered {
                Ok(_) => append_error(&mut error, RESTORED_NOTICE.to_string()),
                Err(recovery_error) => {
                    let _ = lifecycle.shutdown().await;
                    append_error(
                        &mut error,
                        format!("Notebook recovery failed: {recovery_error}. {RECOVERY_NOTICE}"),
                    );
                }
            }
        } else {
            cell.push(text_item(RECOVERY_NOTICE));
        }
    } else if error.is_none() {
        // Automatic checkpoints belong only to successful completed cells.
        // JS exceptions retain live mutations, but never promote failed work
        // into the checkpoint used for interruption/fatal recovery.
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
