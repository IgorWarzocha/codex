use jupyter_protocol::ExecuteReply;
use jupyter_protocol::JupyterMessageContent;
use jupyter_protocol::ReplyStatus;
use jupyter_protocol::Stdio;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::KernelError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamName {
    Stdout,
    Stderr,
}

/// Ordered Jupyter events, not a rendered cell. Callers apply clear/update semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Output {
    Stream {
        name: StreamName,
        text: String,
    },
    Display {
        data: Value,
        metadata: Value,
        display_id: Option<String>,
        update: bool,
    },
    ExecuteResult {
        data: Value,
        metadata: Value,
    },
    Clear {
        wait: bool,
    },
    Error(ExecutionError),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionError {
    pub name: String,
    pub value: String,
    pub traceback: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Ok,
    Error,
    Aborted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub status: ExecutionStatus,
    /// The same events delivered to the streaming callback. Do not render twice.
    pub outputs: Vec<Output>,
    pub error: Option<ExecutionError>,
    pub output_truncated: bool,
}

pub(crate) struct Collector {
    result: ExecutionResult,
    remaining: usize,
    error_budget: usize,
}

impl Collector {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            result: ExecutionResult {
                status: ExecutionStatus::Ok,
                outputs: Vec::new(),
                error: None,
                output_truncated: false,
            },
            remaining: budget,
            error_budget: budget.min(16 * 1024),
        }
    }

    pub(crate) fn push(
        &mut self,
        content: JupyterMessageContent,
        on_output: &mut impl FnMut(&Output),
    ) -> Result<(), KernelError> {
        let output = match content {
            JupyterMessageContent::StreamContent(s) => Output::Stream {
                name: match s.name {
                    Stdio::Stdout => StreamName::Stdout,
                    Stdio::Stderr => StreamName::Stderr,
                },
                text: s.text,
            },
            JupyterMessageContent::DisplayData(d) => Output::Display {
                data: serde_json::to_value(d.data)?,
                metadata: Value::Object(d.metadata),
                display_id: d.transient.and_then(|t| t.display_id),
                update: false,
            },
            JupyterMessageContent::UpdateDisplayData(d) => Output::Display {
                data: serde_json::to_value(d.data)?,
                metadata: Value::Object(d.metadata),
                display_id: d.transient.display_id,
                update: true,
            },
            JupyterMessageContent::ExecuteResult(d) => Output::ExecuteResult {
                data: serde_json::to_value(d.data)?,
                metadata: Value::Object(d.metadata),
            },
            JupyterMessageContent::ClearOutput(c) => Output::Clear { wait: c.wait },
            JupyterMessageContent::ErrorOutput(e) => Output::Error(ExecutionError {
                name: e.ename,
                value: e.evalue,
                traceback: e.traceback,
            }),
            _ => return Ok(()),
        };
        // Preserve a bounded exception even if preceding console output exhausted the budget.
        if let Output::Error(error) = &output {
            let (error, truncated) = bound_error(error.clone(), self.error_budget);
            self.result.error = Some(error);
            self.result.output_truncated |= truncated;
        }
        // Charge serialized size including per-event overhead, so tiny events are bounded too.
        let size = serde_json::to_vec(&output)?.len();
        if size > self.remaining {
            self.result.output_truncated = true;
            return Ok(());
        }
        self.remaining -= size;
        on_output(&output);
        self.result.outputs.push(output);
        Ok(())
    }

    pub(crate) fn finish(mut self, reply: ExecuteReply) -> ExecutionResult {
        self.result.status = match reply.status {
            ReplyStatus::Ok => ExecutionStatus::Ok,
            ReplyStatus::Error => ExecutionStatus::Error,
            ReplyStatus::Aborted => ExecutionStatus::Aborted,
        };
        // IOPub carries the specific exception on Deno versions whose shell reply is generic.
        if self.result.error.is_none()
            && let Some(e) = reply.error
        {
            let (error, truncated) = bound_error(
                ExecutionError {
                    name: e.ename,
                    value: e.evalue,
                    traceback: e.traceback,
                },
                self.error_budget,
            );
            self.result.error = Some(error);
            self.result.output_truncated |= truncated;
        }
        self.result
    }
}

fn bound_error(mut error: ExecutionError, mut remaining: usize) -> (ExecutionError, bool) {
    let mut truncated = false;
    for text in std::iter::once(&mut error.name)
        .chain(std::iter::once(&mut error.value))
        .chain(error.traceback.iter_mut())
    {
        let mut end = remaining.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        truncated |= end < text.len();
        text.truncate(end);
        remaining -= end;
    }
    error.traceback.retain(|line| !line.is_empty());
    (error, truncated)
}
