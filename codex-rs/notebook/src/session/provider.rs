use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::time::Duration;

use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeSessionProviderFuture;
use codex_notebook_kernel::KernelOptions;
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::Registry;
use super::Resources;
use super::Session;
use super::run_session;
use crate::bridge::Bridge;
use crate::control::NotebookControlResult;
use crate::control::NotebookRequest;
use crate::journal::Journal;
use crate::lifecycle::Lifecycle;
use crate::storage::Store;

/// One unsandboxed Deno Jupyter kernel per Codex thread. No TypeScript host controller.
pub struct DenoNotebookSessionProvider {
    pub(super) deno_program: PathBuf,
    cwd: PathBuf,
    identity: Option<Identity>,
    ephemeral: bool,
    session: Mutex<Weak<Session>>,
    creation_gate: Semaphore,
    startup_error: Mutex<Option<String>>,
}

#[derive(Clone)]
struct Identity {
    codex_home: PathBuf,
    thread_id: String,
}

impl DenoNotebookSessionProvider {
    pub fn new(deno_program: PathBuf, cwd: PathBuf) -> Self {
        Self {
            deno_program,
            cwd,
            identity: None,
            ephemeral: false,
            session: Mutex::new(Weak::new()),
            creation_gate: Semaphore::new(1),
            startup_error: Mutex::new(None),
        }
    }

    pub fn new_with_identity(
        deno_program: PathBuf,
        cwd: PathBuf,
        codex_home: PathBuf,
        thread_id: String,
    ) -> Self {
        Self {
            identity: Some(Identity {
                codex_home,
                thread_id,
            }),
            ..Self::new(deno_program, cwd)
        }
    }

    pub fn with_ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }

    pub async fn control(&self, request: NotebookRequest) -> Result<NotebookControlResult, String> {
        if let NotebookRequest::List { query } = &request {
            let details = match self.identity.as_ref().filter(|_| !self.ephemeral) {
                Some(identity) => {
                    Store::new(identity.codex_home.clone(), &self.cwd, &identity.thread_id)?
                        .list_profiles(query.as_deref())
                        .await?
                }
                None => json!({"profiles":[]}),
            };
            return Ok(NotebookControlResult {
                message: format!("Notebook profiles: {details}"),
                details,
            });
        }
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade();
        // Historical diagnostics must not initialize the kernel or execute startup hooks.
        if matches!(request, NotebookRequest::Diagnostics) {
            let (health, bindings) = match &session {
                Some(session) => {
                    let state = session
                        .registry
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let health = if state.closed {
                        "not_started"
                    } else if state.broken {
                        "invalidated"
                    } else {
                        "ready"
                    };
                    let bindings = state.status["bindings"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|b| b["name"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    (health, bindings)
                }
                None => {
                    let failed = self
                        .startup_error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_some();
                    (
                        if failed { "invalidated" } else { "not_started" },
                        Vec::new(),
                    )
                }
            };
            return match self.identity.as_ref().filter(|_| !self.ephemeral) {
                Some(identity) => {
                    crate::diagnostics::diagnostics_with_runtime(
                        &self.deno_program,
                        &self.cwd,
                        &identity.codex_home,
                        &identity.thread_id,
                        health,
                        &bindings,
                    )
                    .await
                }
                None => Ok(NotebookControlResult {
                    message: "Notebook diagnostics unavailable without a durable journal"
                        .to_string(),
                    details: json!({"runtimeHealth":health,"ephemeral":true}),
                }),
            };
        }
        if session.is_none()
            && let Some(identity) = self.identity.as_ref().filter(|_| !self.ephemeral)
        {
            let mut store =
                Store::new(identity.codex_home.clone(), &self.cwd, &identity.thread_id)?;
            match &request {
                NotebookRequest::Unpin { names } => {
                    return Ok(NotebookControlResult {
                        message: "Notebook pins removed without starting kernel".to_string(),
                        details: store.unpin(names).await?,
                    });
                }
                NotebookRequest::Reset => {
                    store.reset_session().await?;
                    return Ok(NotebookControlResult { message: "Notebook private checkpoint reset. Project state and profiles preserved".to_string(), details: json!({"reset":true,"projectPreserved":true}) });
                }
                _ => {}
            }
        }
        let session =
            session.ok_or_else(|| "notebook session has not been initialized".to_string())?;
        session.control(request).await
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
            let _creation = self
                .creation_gate
                .acquire()
                .await
                .map_err(|e| e.to_string())?;
            if let Some(session) = self
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .upgrade()
            {
                let closed = session
                    .registry
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .closed;
                if !closed {
                    return Ok(session as Arc<dyn CodeModeSession>);
                }
            }
            self.availability()?;
            let options = KernelOptions {
                deno: self.deno_program.clone(),
                cwd: Some(self.cwd.clone()),
                // Foreground observation deadlines yield, they do not kill the kernel.
                execute_timeout: Duration::from_secs(24 * 60 * 60),
                ..KernelOptions::default()
            };
            let identity = self.identity.as_ref().filter(|_| !self.ephemeral);
            let store = identity
                .map(|identity| {
                    Store::new(identity.codex_home.clone(), &self.cwd, &identity.thread_id)
                })
                .transpose()?;
            let journal = identity
                .map(|identity| Journal::new(&identity.codex_home, &self.cwd, &identity.thread_id))
                .transpose()?;
            let registry = Arc::new(Registry::default());
            let mut bridge = Bridge::start(registry.clone()).await?;
            let bootstrap = include_str!("../bootstrap.js")
                .replace(
                    "__ENDPOINT__",
                    &serde_json::to_string(&bridge.endpoint).map_err(|e| e.to_string())?,
                )
                .replace(
                    "__CREDENTIAL__",
                    &serde_json::to_string(&bridge.credential).map_err(|e| e.to_string())?,
                );
            let lifecycle = match Lifecycle::start(options, bootstrap, store).await {
                Ok(lifecycle) => lifecycle,
                Err(error) => {
                    *self
                        .startup_error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.clone());
                    let _ = bridge.shutdown().await;
                    return Err(error);
                }
            };
            *self
                .startup_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            registry.update(&lifecycle);
            let cancellation = CancellationToken::new();
            let (commands, incoming) = mpsc::unbounded_channel();
            let worker_registry = registry.clone();
            let worker_cancel = cancellation.clone();
            let worker = tokio::spawn(async move {
                run_session(lifecycle, journal, worker_registry, worker_cancel, incoming).await
            });
            let session = Arc::new(Session {
                registry,
                commands,
                resources: Mutex::new(Some(Resources { bridge, worker })),
                shutdown_gate: Semaphore::new(1),
                cancellation,
            });
            *self
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&session);
            Ok(session as Arc<dyn CodeModeSession>)
        })
    }
}
