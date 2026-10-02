use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use jupyter_protocol::ConnectionInfo;
use jupyter_protocol::connection_info::Transport;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::Command;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::KernelError;
use crate::KernelOptions;

const STDERR_BYTES: usize = 16 * 1024;

pub(crate) struct Process {
    pub(crate) child: Child,
    #[cfg(unix)]
    process_group: Option<u32>,
    directory: Option<TempDir>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
}

impl Process {
    pub(crate) async fn spawn(
        options: &KernelOptions,
    ) -> Result<(Self, ConnectionInfo), KernelError> {
        let mut directory = tempfile::Builder::new();
        directory.prefix("codex-deno-kernel-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            directory.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = directory.tempdir()?;
        let (ports, reservations) =
            jupyter_zmq_client::peek_ports_with_listeners(std::net::Ipv4Addr::LOCALHOST.into(), 5)
                .await?;
        let info = ConnectionInfo {
            transport: Transport::TCP,
            ip: "127.0.0.1".into(),
            shell_port: ports[0],
            iopub_port: ports[1],
            stdin_port: ports[2],
            control_port: ports[3],
            hb_port: ports[4],
            signature_scheme: "hmac-sha256".into(),
            key: Uuid::new_v4().to_string(),
            kernel_name: Some("deno".into()),
        };
        let path = directory.path().join("connection.json");
        let mut open = tokio::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        open.mode(0o600);
        let mut file = open.open(&path).await?;
        file.write_all(&serde_json::to_vec(&info)?).await?;
        file.flush().await?;
        drop(file);

        let mut command = Command::new(&options.deno);
        command
            .args(["jupyter", "--kernel", "--conn"])
            .arg(&path)
            .current_dir(options.cwd.as_deref().unwrap_or(directory.path()))
            .envs(&options.env)
            .env("DENO_NO_PACKAGE_JSON", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        if let Some(mib) = options.max_heap_mib {
            let mut flags = options
                .env
                .get(std::ffi::OsStr::new("DENO_V8_FLAGS"))
                .cloned()
                .or_else(|| std::env::var_os("DENO_V8_FLAGS"))
                .unwrap_or_default();
            if !flags.is_empty() {
                flags.push(",");
            }
            flags.push(format!("--max-old-space-size={mib}"));
            command.env("DENO_V8_FLAGS", flags);
        }
        // Deno cannot inherit these listeners. A small handoff race is unavoidable.
        // A competing bind causes an explicit startup failure, never unauthenticated reuse.
        // Release before spawn: the child may bind before the parent is scheduled again.
        drop(reservations);
        let mut child = command.spawn()?;
        #[cfg(unix)]
        let process_group = child.id();
        let mut reader = child
            .stderr
            .take()
            .ok_or(KernelError::InvalidOptions("missing stderr pipe"))?;
        let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
        let captured = Arc::clone(&stderr);
        let stderr_task = tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                let mut tail = captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                tail.extend_from_slice(&buffer[..count]);
                let excess = tail.len().saturating_sub(STDERR_BYTES);
                tail.drain(..excess);
            }
        });
        Ok((
            Self {
                child,
                #[cfg(unix)]
                process_group,
                directory: Some(directory),
                stderr,
                stderr_task,
            },
            info,
        ))
    }

    pub(crate) fn exited(&self, status: std::process::ExitStatus) -> KernelError {
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        KernelError::Exited {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }

    pub(crate) fn kill(&mut self) -> Result<(), KernelError> {
        #[cfg(unix)]
        let group_result = if let Some(group) = self.process_group {
            // Keep the saved PGID even after the leader exits. Its children may remain.
            codex_utils_pty::process_group::kill_process_group(group)
        } else {
            Ok(())
        };
        let child_result = self.child.start_kill();
        #[cfg(unix)]
        group_result?;
        child_result?;
        Ok(())
    }

    pub(crate) async fn reap(&mut self, grace: Duration) -> Result<(), KernelError> {
        if self.directory.is_none() {
            return Ok(());
        }
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(status) => {
                status?;
            }
            Err(_) => {
                self.kill()?;
                tokio::time::timeout(grace, self.child.wait())
                    .await
                    .map_err(|_| KernelError::Timeout("process reaping"))??;
            }
        }
        // A graceful Deno exit does not imply its spawned commands have exited.
        self.kill()?;
        #[cfg(unix)]
        {
            self.process_group = None;
        }
        self.stderr_task.abort();
        // Join the owned drainer after abort, then delete credentials only after exit.
        let _ = (&mut self.stderr_task).await;
        if let Some(directory) = self.directory.take() {
            directory.close()?;
        }
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
        // Child's kill_on_drop also registers it with Tokio's orphan reaper.
        let _ = self.kill();
    }
}
