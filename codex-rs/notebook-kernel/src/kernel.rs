use std::time::Duration;

use jupyter_protocol::CompleteRequest;
use jupyter_protocol::ExecuteRequest;
use jupyter_protocol::ExecutionState;
use jupyter_protocol::JupyterMessage;
use jupyter_protocol::JupyterMessageContent;
use jupyter_protocol::KernelInfoRequest;
use jupyter_protocol::ReplyStatus;
use jupyter_protocol::ShutdownRequest;
use jupyter_zmq_client::ClientControlConnection;
use jupyter_zmq_client::ClientIoPubConnection;
use jupyter_zmq_client::ClientShellConnection;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ExecutionResult;
use crate::KernelError;
use crate::KernelOptions;
use crate::Output;
use crate::output::Collector;
use crate::process::Process;

struct Channels {
    shell: ClientShellConnection,
    control: ClientControlConnection,
    iopub: ClientIoPubConnection,
}

#[derive(PartialEq, Eq)]
enum State {
    Ready,
    Running,
    Closed,
}

/// One persistent JS/TS isolate. Exclusive mutable execution prevents concurrent cells.
/// Dropping an in-flight execution future terminates this kernel, as does cancellation.
pub struct Kernel {
    options: KernelOptions,
    process: Process,
    channels: Option<Channels>,
    state: State,
}

impl Kernel {
    pub async fn start(options: KernelOptions) -> Result<Self, KernelError> {
        Self::start_cancellable(options, CancellationToken::new()).await
    }

    pub async fn start_cancellable(
        options: KernelOptions,
        cancellation: CancellationToken,
    ) -> Result<Self, KernelError> {
        if !cfg!(unix) {
            return Err(KernelError::UnsupportedPlatform);
        }
        if options.max_heap_mib == Some(0)
            || options.startup_timeout.is_zero()
            || options.execute_timeout.is_zero()
            || options.shutdown_timeout.is_zero()
        {
            return Err(KernelError::InvalidOptions(
                "heap size and timeouts must be positive",
            ));
        }
        let deadline = tokio::time::Instant::now() + options.startup_timeout;
        let (process, info) = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(KernelError::Cancelled),
            result = tokio::time::timeout_at(deadline, Process::spawn(&options)) => result.map_err(|_| KernelError::Timeout("startup"))??,
        };
        let mut kernel = Self {
            options,
            process,
            channels: None,
            state: State::Closed,
        };
        let startup = async {
            let session = Uuid::new_v4().to_string();
            let connected = async {
                let iopub =
                    jupyter_zmq_client::create_client_iopub_connection(&info, "", &session).await?;
                let identity = jupyter_zmq_client::peer_identity_for_session(&session)?;
                let shell = jupyter_zmq_client::create_client_shell_connection_with_identity(
                    &info, &session, identity,
                )
                .await?;
                let control =
                    jupyter_zmq_client::create_client_control_connection(&info, &session).await?;
                Ok::<_, KernelError>(Channels {
                    shell,
                    control,
                    iopub,
                })
            };
            let result = tokio::select! {
                result = connected => result,
                exit = kernel.process.child.wait() => Err(kernel.process.exited(exit?)),
            };
            kernel.channels = Some(result?);
            kernel.ready().await
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(KernelError::Cancelled),
            result = tokio::time::timeout_at(deadline, startup) => result.map_err(|_| KernelError::Timeout("startup")).and_then(|r| r),
        };
        if let Err(error) = result {
            kernel.force_shutdown().await?;
            return Err(error);
        }
        kernel.state = State::Ready;
        Ok(kernel)
    }

    pub async fn execute(&mut self, source: &str) -> Result<ExecutionResult, KernelError> {
        self.execute_streaming(source, CancellationToken::new(), |_| {})
            .await
    }

    /// Host management payloads have a separate bound from user-visible cell output.
    pub async fn execute_with_output_limit(
        &mut self,
        source: &str,
        max_bytes: usize,
    ) -> Result<ExecutionResult, KernelError> {
        let previous = self.options.max_output_bytes;
        self.options.max_output_bytes = max_bytes;
        let result = self.execute(source).await;
        self.options.max_output_bytes = previous;
        result
    }

    /// Deno's completions expose persistent lexical bindings as well as global properties.
    pub async fn complete(
        &mut self,
        code: &str,
        cursor_pos: usize,
    ) -> Result<Vec<String>, KernelError> {
        if self.state != State::Ready {
            return Err(KernelError::Closed);
        }
        self.state = State::Running;
        let guard = ExecutionGuard(self);
        let timeout = guard.0.options.startup_timeout;
        let request: JupyterMessage = CompleteRequest {
            code: code.to_owned(),
            cursor_pos,
        }
        .into();
        let id = request.header.msg_id.clone();
        let operation = async {
            let channels = guard.0.channels.as_mut().ok_or(KernelError::Closed)?;
            channels.shell.send(request).await?;
            loop {
                tokio::select! {
                    message = channels.shell.read() => {
                        let message = message?;
                        if !correlated(&message, &id) { continue; }
                        return match message.content {
                            JupyterMessageContent::CompleteReply(reply) if reply.status == ReplyStatus::Ok => Ok(reply.matches),
                            JupyterMessageContent::CompleteReply(reply) => Err(KernelError::UnexpectedReply(
                                reply.error.map(|e| format!("completion failed: {}: {}", e.ename, e.evalue)).unwrap_or_else(|| format!("completion {:?}", reply.status))
                            )),
                            _ => Err(KernelError::UnexpectedReply(message.header.msg_type)),
                        };
                    }
                    exit = guard.0.process.child.wait() => return Err(guard.0.process.exited(exit?)),
                }
            }
        };
        match tokio::time::timeout(timeout, operation).await {
            Ok(Ok(names)) => {
                guard.0.state = State::Ready;
                Ok(names)
            }
            Ok(Err(error)) => {
                guard.0.force_shutdown().await?;
                Err(error)
            }
            Err(_) => {
                guard.0.force_shutdown().await?;
                Err(KernelError::Timeout("completion"))
            }
        }
    }

    /// Callback runs synchronously on receipt of each retained output. Keep it nonblocking.
    /// Result contains the same events. Timeouts and cancellation invalidate all state.
    pub async fn execute_streaming(
        &mut self,
        source: &str,
        cancellation: CancellationToken,
        mut on_output: impl FnMut(&Output),
    ) -> Result<ExecutionResult, KernelError> {
        if self.state != State::Ready {
            return Err(KernelError::Closed);
        }
        if cancellation.is_cancelled() {
            self.force_shutdown().await?;
            return Err(KernelError::Cancelled);
        }
        self.state = State::Running;
        let guard = ExecutionGuard(self);
        let deadline = guard.0.options.execute_timeout;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(KernelError::Cancelled),
            result = tokio::time::timeout(deadline, guard.0.run(source, &mut on_output)) => result.map_err(|_| KernelError::Timeout("execution")).and_then(|r| r),
        };
        match result {
            Ok(result) => {
                guard.0.state = State::Ready;
                Ok(result)
            }
            Err(error) => {
                guard.0.force_shutdown().await?;
                Err(error)
            }
        }
    }

    /// Idempotent graceful shutdown, followed by force-kill and bounded reaping if needed.
    pub async fn shutdown(&mut self) -> Result<(), KernelError> {
        self.state = State::Closed;
        let mut channels = self.channels.take();
        if let Some(channels) = channels.as_mut() {
            // Failure to deliver the protocol request is handled by process termination below.
            let _ = tokio::time::timeout(
                self.options.shutdown_timeout,
                channels
                    .control
                    .send(ShutdownRequest { restart: false }.into()),
            )
            .await;
        }
        let result = self.process.reap(self.options.shutdown_timeout).await;
        // Keep the control socket alive until the queued shutdown message can be delivered.
        drop(channels);
        result
    }

    async fn force_shutdown(&mut self) -> Result<(), KernelError> {
        self.state = State::Closed;
        self.channels.take();
        self.process.kill()?;
        self.process.reap(self.options.shutdown_timeout).await
    }

    async fn ready(&mut self) -> Result<(), KernelError> {
        let channels = self.channels.as_mut().ok_or(KernelError::Closed)?;
        let mut request: JupyterMessage = KernelInfoRequest {}.into();
        channels.shell.send(request.clone()).await?;
        let mut replied = false;
        let mut published = false;
        let mut retry = tokio::time::interval(Duration::from_millis(100));
        retry.tick().await;
        loop {
            tokio::select! {
                message = channels.shell.read() => {
                    let message = message?;
                    if correlated(&message, &request.header.msg_id) {
                        if !matches!(message.content, JupyterMessageContent::KernelInfoReply(_)) {
                            return Err(KernelError::UnexpectedReply(message.header.msg_type));
                        }
                        replied = true;
                    }
                }
                message = channels.iopub.read() => {
                    let _ = message?;
                    published = true;
                }
                exit = self.process.child.wait() => return Err(self.process.exited(exit?)),
                _ = retry.tick(), if replied && !published => {
                    request = KernelInfoRequest {}.into();
                    replied = false;
                    channels.shell.send(request.clone()).await?;
                }
            }
            // Prove both shell responsiveness and the PUB subscription. No fixed sleep guess.
            if replied && published {
                return Ok(());
            }
        }
    }

    async fn run(
        &mut self,
        source: &str,
        on_output: &mut impl FnMut(&Output),
    ) -> Result<ExecutionResult, KernelError> {
        let channels = self.channels.as_mut().ok_or(KernelError::Closed)?;
        let request: JupyterMessage = ExecuteRequest::new(source.to_owned()).into();
        let id = request.header.msg_id.clone();
        channels.shell.send(request).await?;
        let mut reply = None;
        let mut idle = false;
        let mut collector = Collector::new(self.options.max_output_bytes);
        loop {
            tokio::select! {
                message = channels.shell.read(), if reply.is_none() => {
                    let message = message?;
                    if !correlated(&message, &id) { continue; }
                    match message.content {
                        JupyterMessageContent::ExecuteReply(value) => reply = Some(value),
                        _ => return Err(KernelError::UnexpectedReply(message.header.msg_type)),
                    }
                }
                message = channels.iopub.read(), if !idle => {
                    let message = message?;
                    if !correlated(&message, &id) { continue; }
                    match message.content {
                        JupyterMessageContent::Status(s) if s.execution_state == ExecutionState::Idle => idle = true,
                        content => collector.push(content, on_output)?,
                    }
                }
                exit = self.process.child.wait() => return Err(self.process.exited(exit?)),
            }
            if idle && let Some(reply) = reply.take() {
                return Ok(collector.finish(reply));
            }
        }
    }
}

fn correlated(message: &JupyterMessage, id: &str) -> bool {
    message
        .parent_header
        .as_ref()
        .is_some_and(|h| h.msg_id == id)
}

struct ExecutionGuard<'a>(&'a mut Kernel);

impl Drop for ExecutionGuard<'_> {
    fn drop(&mut self) {
        if self.0.state == State::Running {
            self.0.state = State::Closed;
            self.0.channels.take();
            let _ = self.0.process.kill();
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
