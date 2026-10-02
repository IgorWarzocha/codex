use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::ToolDefinition;
use codex_notebook_kernel::Output;
use tokio::sync::Notify;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub(crate) const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct Cell {
    pub(crate) id: CellId,
    pub(crate) call_id: String,
    pub(crate) delegate: Arc<dyn CodeModeSessionDelegate>,
    pub(crate) tools: HashMap<String, ToolDefinition>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) tool_cancellation: CancellationToken,
    state: Mutex<CellState>,
    changed: Notify,
    observer: Semaphore,
}

enum Phase {
    Running,
    Closing,
    Completed(Option<String>),
    Terminated,
    Observed,
}

struct CellState {
    phase: Phase,
    pending: Vec<FunctionCallOutputContentItem>,
    pending_bytes: usize,
    overflow: bool,
    yield_pending: bool,
    journal_items: Vec<FunctionCallOutputContentItem>,
    journal_bytes: usize,
    journal_overflow: bool,
}

impl Cell {
    pub(crate) fn new(
        id: CellId,
        call_id: String,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        tools: HashMap<String, ToolDefinition>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            id,
            call_id,
            delegate,
            tools,
            tool_cancellation: cancellation.child_token(),
            cancellation,
            state: Mutex::new(CellState {
                phase: Phase::Running,
                pending: Vec::new(),
                pending_bytes: 0,
                overflow: false,
                yield_pending: false,
                journal_items: Vec::new(),
                journal_bytes: 0,
                journal_overflow: false,
            }),
            changed: Notify::new(),
            observer: Semaphore::new(1),
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .phase,
            Phase::Running
        )
    }

    pub(crate) fn push(&self, item: FunctionCallOutputContentItem) {
        let size = serde_json::to_vec(&item).map_or(MAX_PENDING_BYTES + 1, |json| json.len());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(state.phase, Phase::Running) {
            return;
        }
        if state.journal_bytes.saturating_add(size) <= MAX_PENDING_BYTES {
            state.journal_bytes += size;
            state.journal_items.push(item.clone());
        } else {
            state.journal_overflow = true;
        }
        if state.pending_bytes.saturating_add(size) > MAX_PENDING_BYTES {
            state.overflow = true;
        } else {
            state.pending_bytes += size;
            state.pending.push(item);
        }
    }

    pub(crate) fn journal_items(&self) -> Vec<FunctionCallOutputContentItem> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut items = state.journal_items.clone();
        if state.journal_overflow {
            items.push(text_item("[Notebook journal output truncated]"));
        }
        items
    }

    pub(crate) fn push_kernel_output(&self, output: &Output) {
        match output {
            Output::Stream { text, .. } => self.push(text_item(text.clone())),
            Output::Display { data, update, .. } => {
                if *update {
                    self.push(text_item("[Jupyter display update rendered as a new item]"));
                }
                for mime in ["image/png", "image/jpeg", "image/webp", "image/gif"] {
                    if let Some(data) = data.get(mime).and_then(serde_json::Value::as_str) {
                        self.push(FunctionCallOutputContentItem::InputImage {
                            image_url: format!("data:{mime};base64,{data}"),
                            detail: None,
                        });
                        return;
                    }
                }
                if let Some(text) = data.get("text/plain").and_then(serde_json::Value::as_str) {
                    self.push(text_item(text));
                } else {
                    self.push(text_item("[Unsupported Jupyter display MIME type]"));
                }
            }
            Output::Clear { .. } => self.push(text_item("[Jupyter clear_output is unsupported]")),
            // Bare expression values are intentionally discarded. Errors belong in error_text.
            Output::ExecuteResult { .. } | Output::Error(_) => {}
        }
    }

    pub(crate) fn yield_now(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .yield_pending = true;
        self.changed.notify_waiters();
    }

    pub(crate) fn finish(&self, error: Option<String>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(state.phase, Phase::Running) {
            let terminal = if self.cancellation.is_cancelled() {
                Phase::Terminated
            } else {
                Phase::Completed(error)
            };
            state.phase = Phase::Closing;
            drop(state);
            // Stop routing callbacks before closing delegate state. Publish the terminal
            // observation only after cell_closed has finished.
            self.cancellation.cancel();
            self.delegate.cell_closed(&self.id);
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .phase = terminal;
            self.changed.notify_waiters();
        }
    }

    /// Every observation owns one drain. Completion and yield notifications cannot replay output.
    pub(crate) async fn observe(
        &self,
        yield_time: Duration,
        preempt: Option<CancellationToken>,
    ) -> Option<RuntimeResponse> {
        let started = Instant::now();
        let _observer = self.observer.acquire().await.ok()?;
        let timer = tokio::time::sleep(yield_time);
        tokio::pin!(timer);
        let preempt = preempt.unwrap_or_default();
        let mut yield_requested = false;
        loop {
            // Register before inspecting phase, so a terminal transition cannot lose its wakeup.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match &state.phase {
                    Phase::Observed => return None,
                    Phase::Closing => {}
                    Phase::Running if !yield_requested && !state.yield_pending => {}
                    _ => {
                        let content_items = drain(&mut state);
                        let response = match std::mem::replace(&mut state.phase, Phase::Observed) {
                            Phase::Running => {
                                state.phase = Phase::Running;
                                RuntimeResponse::Yielded {
                                    cell_id: self.id.clone(),
                                    content_items,
                                    code_mode_host_duration: None,
                                }
                            }
                            Phase::Completed(error_text) => RuntimeResponse::Result {
                                cell_id: self.id.clone(),
                                content_items,
                                error_text,
                                code_mode_host_duration: None,
                            },
                            Phase::Terminated => RuntimeResponse::Terminated {
                                cell_id: self.id.clone(),
                                content_items,
                                code_mode_host_duration: None,
                            },
                            Phase::Observed => return None,
                            Phase::Closing => return None,
                        };
                        state.yield_pending = false;
                        return Some(response.with_code_mode_host_duration(started.elapsed()));
                    }
                }
            }
            tokio::select! {
                _ = &mut notified => {},
                _ = &mut timer, if !yield_requested => yield_requested = true,
                _ = preempt.cancelled(), if !yield_requested => yield_requested = true,
            }
        }
    }
}

pub(crate) fn text_item(text: impl Into<String>) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputText { text: text.into() }
}

fn drain(state: &mut CellState) -> Vec<FunctionCallOutputContentItem> {
    let mut items = std::mem::take(&mut state.pending);
    // Per-observation token budgets belong to the exec/wait caller, not the cell.
    if state.overflow {
        items.push(text_item("[Notebook output truncated]"));
    }
    state.pending_bytes = 0;
    state.overflow = false;
    items
}
