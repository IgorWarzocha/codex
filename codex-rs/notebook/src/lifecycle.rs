use std::collections::HashSet;
use std::time::Duration;

use codex_notebook_kernel::Kernel;
use codex_notebook_kernel::KernelOptions;
use codex_notebook_kernel::Output;
use serde_json::Value;
use serde_json::json;
use uuid::Uuid;

use crate::control::NotebookControlResult;
use crate::control::NotebookHook;
use crate::control::NotebookRequest;
use crate::control::format_details;
use crate::control::identifier;
use crate::control::matches;
use crate::import_history;
use crate::import_history::ImportHistory;
use crate::persistence::PersistenceBudget;
use crate::session::result_error;
use crate::storage::Store;

#[cfg(test)]
#[path = "lifecycle/default_profile_tests.rs"]
mod default_profile_tests;

/// The worker exclusively owns the kernel, discovery baseline, and durable transactions.
pub(crate) struct Lifecycle {
    pub(crate) kernel: Option<Kernel>,
    options: KernelOptions,
    bootstrap: String,
    baseline: HashSet<String>,
    store: Option<Store>,
    import_history: Option<ImportHistory>,
    imports: Vec<String>,
    imports_error: Option<String>,
    snapshot: Value,
    default_profile: Option<Value>,
    pub(crate) status: Value,
    pub(crate) checkpoint_details: Value,
    pub(crate) persistence_error: Option<String>,
}

impl Lifecycle {
    pub(crate) async fn start(
        options: KernelOptions,
        bootstrap: String,
        store: Option<Store>,
        default_profile: Option<(Store, String)>,
    ) -> Result<Self, String> {
        let import_history = match &store {
            Some(store) => Some(store.import_history().await?),
            None => None,
        };
        let mut lifecycle = Self {
            kernel: None,
            options,
            bootstrap,
            store,
            import_history,
            imports: Vec::new(),
            imports_error: None,
            baseline: HashSet::new(),
            snapshot: json!({"entries":[],"skipped":[]}),
            default_profile: None,
            status: json!({}),
            checkpoint_details: json!({"state":"not_saved"}),
            persistence_error: None,
        };
        lifecycle.restore(false, default_profile).await?;
        // Persist the seed and startup-hook mutations before exposing the fresh
        // session. A later resume must never need the profile or replay its seed.
        if lifecycle.default_profile.is_some()
            && let Err(error) = lifecycle.checkpoint(&[]).await
        {
            let _ = lifecycle.shutdown().await;
            return Err(format!("checkpoint default notebook profile: {error}"));
        }
        Ok(lifecycle)
    }

    async fn restore(
        &mut self,
        reset: bool,
        default_profile: Option<(Store, String)>,
    ) -> Result<(), String> {
        self.shutdown().await?;
        if let Some(history) = &self.import_history {
            match history.read().await {
                Ok(imports) => {
                    self.imports = imports;
                    self.imports_error = None;
                }
                Err(error) => self.imports_error = Some(error),
            }
        }
        if reset && let Some(store) = &mut self.store {
            store.reset_session().await?;
        }
        let mut kernel = Kernel::start(self.options.clone())
            .await
            .map_err(|e| e.to_string())?;
        let initialization = async {
            let result = kernel
                .execute(include_str!("kernel-state.js"))
                .await
                .map_err(|e| e.to_string())?;
            if let Some(error) = result_error(&result) {
                return Err(error);
            }
            let result = kernel
                .execute(&self.bootstrap)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(error) = result_error(&result) {
                return Err(error);
            }
            kernel.complete("", 0).await.map_err(|e| e.to_string())
        }
        .await;
        let names = match initialization {
            Ok(names) => names,
            Err(error) => {
                let _ = kernel.shutdown().await;
                return Err(format!("initialize notebook: {error}"));
            }
        };
        self.baseline = names.into_iter().collect();
        self.kernel = Some(kernel);
        let restored = self.restore_values(reset, default_profile).await;
        if let Err(error) = restored {
            let cleanup = self.shutdown().await;
            return Err(format!(
                "restore notebook failed: {error}{}",
                cleanup
                    .err()
                    .map(|e| format!("; cleanup: {e}"))
                    .unwrap_or_default()
            ));
        }
        self.refresh_status().await?;
        Ok(())
    }

    async fn restore_values(
        &mut self,
        reset: bool,
        default_profile: Option<(Store, String)>,
    ) -> Result<(), String> {
        let (project, private) = match &mut self.store {
            Some(store) => (
                store.load_project().await?,
                if reset {
                    None
                } else {
                    store.load_session().await?
                },
            ),
            None => (
                None,
                if reset {
                    None
                } else {
                    Some(self.snapshot.clone())
                },
            ),
        };
        if let Some(project) = &project {
            self.rpc("restore", vec![project.clone()]).await?;
        }
        if let Some(private) = &private {
            if private.get("deno").is_some() {
                self.rpc("restore", vec![private.clone()]).await?;
            }
            self.snapshot = private.clone();
        } else if let Some(project) = &project {
            self.snapshot = project.clone();
        }
        // Shared pin metadata is authoritative even when private values are older.
        if let Some(project) = &project {
            self.rpc(
                "configurePins",
                vec![json!(
                    entries(project)
                        .iter()
                        .filter(|e| pinned(e))
                        .cloned()
                        .collect::<Vec<_>>()
                )],
            )
            .await?;
            self.rpc(
                "syncProjectBindings",
                vec![json!(
                    entries(project)
                        .iter()
                        .filter_map(|e| e["name"].as_str())
                        .collect::<Vec<_>>()
                )],
            )
            .await?;
        }
        // The source exists only on the initial start. An empty durable private
        // checkpoint is still authoritative, including bindings already released.
        if !(self.store.is_some() && private.is_some())
            && let Some((source, name)) = default_profile
        {
            self.seed_default_profile(&source, &name).await?;
        }
        self.rpc_expression("await globalThis.__codexNotebook.runStartupHooks()")
            .await?;
        Ok(())
    }

    async fn seed_default_profile(&mut self, source: &Store, name: &str) -> Result<(), String> {
        let mut snapshot = source
            .load_profile(name)
            .await
            .map_err(|error| format!("default notebook profile {name}: {error}"))?;
        let available: HashSet<_> = self
            .kernel
            .as_mut()
            .ok_or_else(recovery_error)?
            .complete("", 0)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect();
        let mut skipped = snapshot["skipped"].as_array().cloned().unwrap_or_default();
        let mut selected = Vec::new();
        for mut entry in entries(&snapshot) {
            if entry["name"]
                .as_str()
                .is_some_and(|n| available.contains(n))
            {
                skipped.push(json!({"name":entry["name"],"reason":"existing binding"}));
                continue;
            }
            // Profile values cannot introduce pin or startup-hook registrations.
            if let Some(entry) = entry.as_object_mut() {
                entry.remove("pinned");
                entry.remove("hook");
            }
            selected.push(entry);
        }
        let loaded: Vec<_> = selected.iter().map(|entry| entry["name"].clone()).collect();
        snapshot["entries"] = json!(selected);
        self.rpc("restore", vec![snapshot])
            .await
            .map_err(|error| format!("default notebook profile {name}: {error}"))?;
        let loaded_count = loaded.len();
        let skipped_count = skipped.len();
        let loaded = bounded_bindings(loaded, 2048);
        let skipped = bounded_bindings(skipped, 4096);
        self.default_profile = Some(json!({
            "name":name,
            "loadedBindings":loaded_count,
            "skippedBindings":skipped_count,
            "omittedLoaded":loaded_count - loaded.len(),
            "omittedSkipped":skipped_count - skipped.len(),
            "loaded":loaded,
            "skipped":skipped,
        }));
        Ok(())
    }

    async fn names(&mut self) -> Result<Vec<String>, String> {
        let names = self
            .kernel
            .as_mut()
            .ok_or_else(recovery_error)?
            .complete("", 0)
            .await
            .map_err(|e| e.to_string())?;
        let reserved = [
            "tools",
            "ALL_TOOLS",
            "text",
            "image",
            "audio",
            "generatedImage",
            "store",
            "load",
            "notify",
            "yield_control",
            "exit",
        ];
        let mut names: Vec<_> = names
            .into_iter()
            .filter(|name| {
                !self.baseline.contains(name)
                    && !name.starts_with("__codex")
                    && !reserved.contains(&name.as_str())
                    && identifier(name)
            })
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    async fn rpc(&mut self, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let arguments = args
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(",");
        self.rpc_expression(&format!(
            "await globalThis.__codexNotebookState.{method}({arguments})"
        ))
        .await
    }

    async fn rpc_expression(&mut self, expression: &str) -> Result<Value, String> {
        let marker = format!("__CODEX_NOTEBOOK_{}__", Uuid::new_v4());
        let source = format!(
            "{{const value = ({expression}); console.log({}+JSON.stringify(value ?? null));}}",
            json!(marker)
        );
        let kernel = self.kernel.as_mut().ok_or_else(recovery_error)?;
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            kernel.execute_with_output_limit(
                &source,
                PersistenceBudget::from_heap_mib(self.options.max_heap_mib).snapshot_json_bytes(),
            ),
        )
        .await;
        let result = match result {
            Ok(Ok(result)) => result,
            other => {
                let error = match other {
                    Ok(Err(error)) => error.to_string(),
                    _ => "notebook management timed out".to_string(),
                };
                let cleanup = self.shutdown().await;
                return Err(format!(
                    "{error}{}",
                    cleanup
                        .err()
                        .map(|e| format!("; cleanup: {e}"))
                        .unwrap_or_default()
                ));
            }
        };
        if let Some(error) = result_error(&result) {
            return Err(error);
        }
        if result.output_truncated {
            return Err("notebook management response exceeded transport limit".to_string());
        }
        let output = result
            .outputs
            .iter()
            .filter_map(|output| match output {
                Output::Stream { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        let payload = output
            .lines()
            .find_map(|line| line.strip_prefix(&marker))
            .ok_or_else(|| "notebook management did not return its response".to_string())?;
        serde_json::from_str(payload)
            .map_err(|e| format!("invalid notebook management response: {e}"))
    }

    pub(crate) async fn checkpoint(&mut self, excluded: &[String]) -> Result<Value, String> {
        let outcome = self.capture_and_persist(excluded).await;
        match &outcome {
            Ok(details) => {
                self.persistence_error = None;
                self.checkpoint_details = details.clone();
            }
            Err(error) => {
                self.persistence_error = Some(error.clone());
                self.checkpoint_details = json!({"state":"failed","error":error});
            }
        }
        outcome
    }

    async fn capture_and_persist(&mut self, excluded: &[String]) -> Result<Value, String> {
        let names: Vec<_> = self
            .names()
            .await?
            .into_iter()
            .filter(|n| !excluded.contains(n))
            .collect();
        let snapshot = self
            .rpc(
                "capture",
                vec![
                    json!(names),
                    json!(
                        PersistenceBudget::from_heap_mib(self.options.max_heap_mib).payload_bytes()
                    ),
                ],
            )
            .await?;
        let project_names: Vec<String> =
            serde_json::from_value(self.rpc("projectBindings", vec![]).await?)
                .map_err(|e| e.to_string())?;
        let mut project_snapshot = snapshot.clone();
        project_snapshot["entries"] = json!(
            entries(&snapshot)
                .into_iter()
                .filter(|e| pinned(e)
                    || e["name"]
                        .as_str()
                        .is_some_and(|n| project_names.iter().any(|p| p == n)))
                .collect::<Vec<_>>()
        );
        // A skipped capture is not a deletion of the last durable value.
        project_snapshot["skipped"] = json!(
            snapshot["skipped"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e["name"]
                    .as_str()
                    .is_some_and(|n| project_names.iter().any(|p| p == n)))
                .collect::<Vec<_>>()
        );
        let details = match &mut self.store {
            Some(store) => store.checkpoint(&snapshot, &project_snapshot).await?,
            None => {
                json!({"state":"memory_only","bindings":entries(&snapshot).len(),"skipped":snapshot["skipped"]})
            }
        };
        self.snapshot = snapshot;
        self.refresh_status().await?;
        Ok(details)
    }

    /// An inventory failure is visible but does not turn successful code into a failed cell.
    pub(crate) async fn record_imports(&mut self, source: &str) -> Result<(), String> {
        let result = match &self.import_history {
            Some(history) => history.record(source).await,
            None => {
                let source = source.to_owned();
                let current = self.imports.clone();
                tokio::task::spawn_blocking(move || {
                    let mut imports = import_history::extract(&source)?;
                    imports.extend(current);
                    if imports.len() > import_history::MAX_IMPORTS {
                        return Err(format!(
                            "Notebook npm inventory exceeds {} imports",
                            import_history::MAX_IMPORTS
                        ));
                    }
                    Ok(Some(imports.into_iter().collect()))
                })
                .await
                .map_err(|error| format!("Notebook npm inventory task failed: {error}"))
                .and_then(|result| result)
            }
        };
        match result {
            Ok(Some(imports)) => {
                self.imports = imports;
                self.imports_error = None;
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                self.imports_error = Some(error.clone());
                Err(error)
            }
        }
    }

    async fn refresh_status(&mut self) -> Result<(), String> {
        let names = self.names().await?;
        self.status = self.rpc("status", vec![json!(names)]).await?;
        if let Some(profile) = &self.default_profile {
            self.status["defaultProfile"] = profile.clone();
        }
        self.status["npmImportsNotice"] = json!(import_history::notice(&self.imports));
        if let Some(error) = &self.imports_error {
            self.status["npmImportsError"] = json!(bound_text(error, 1024));
        }
        let retained = entries(&self.snapshot);
        self.status["retainedBindings"] = json!(retained.len());
        self.status["retainedBytes"] = json!(
            retained
                .iter()
                .filter_map(|entry| entry["length"].as_u64())
                .sum::<u64>()
        );
        if let Some(bindings) = self.status["bindings"].as_array_mut() {
            for binding in bindings {
                if let Some(entry) = retained
                    .iter()
                    .find(|entry| entry["name"] == binding["name"])
                {
                    binding["bytes"] = entry["length"].clone();
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn control(
        &mut self,
        request: NotebookRequest,
    ) -> Result<NotebookControlResult, String> {
        let label = match &request {
            NotebookRequest::Status { .. } => "Notebook status",
            NotebookRequest::Checkpoint => "Notebook checkpoint",
            NotebookRequest::Restart => "Notebook restarted",
            NotebookRequest::Reset => "Notebook private checkpoint reset",
            NotebookRequest::Save { .. } => "Notebook profile saved",
            NotebookRequest::Load { .. } => "Notebook profile loaded",
            NotebookRequest::Pin { .. } => "Notebook bindings pinned",
            NotebookRequest::Unpin { .. } => "Notebook bindings unpinned",
            NotebookRequest::Release { .. } => "Notebook release",
            NotebookRequest::Prune { .. } => "Notebook prune",
            NotebookRequest::List { .. } | NotebookRequest::Diagnostics => {
                return Err("read-only request was not routed by provider".to_string());
            }
        };
        let details = match request {
            NotebookRequest::Status { query } => {
                self.refresh_status().await?;
                return Ok(status_result(
                    &self.status,
                    &self.checkpoint_details,
                    self.persistence_error.as_deref(),
                    query.as_deref(),
                    false,
                ));
            }
            NotebookRequest::Checkpoint => self.checkpoint(&[]).await?,
            NotebookRequest::Restart => {
                let mut disposal = Value::Null;
                if self.kernel.is_some() {
                    self.checkpoint(&[]).await?;
                    disposal = self.dispose_all().await?;
                }
                self.restore(false, None).await?;
                json!({"restarted":true,"checkpoint":self.checkpoint_details,"disposal":disposal})
            }
            NotebookRequest::Reset => {
                let mut disposal = Value::Null;
                if self.kernel.is_some() {
                    disposal = self.dispose_all().await?;
                }
                self.restore(true, None).await?;
                self.checkpoint_details = json!({"state":"reset"});
                self.persistence_error = None;
                json!({"reset":true,"projectPreserved":true,"disposal":disposal})
            }
            NotebookRequest::Save { name } => {
                self.checkpoint(&[]).await?;
                let store = self
                    .store
                    .as_mut()
                    .ok_or_else(|| "profiles require a notebook storage identity".to_string())?;
                let mut snapshot = self.snapshot.clone();
                // Profiles carry values and function source, never live pin or hook registrations.
                if let Some(entries) = snapshot["entries"].as_array_mut() {
                    for entry in entries {
                        if let Some(entry) = entry.as_object_mut() {
                            entry.remove("pinned");
                            entry.remove("hook");
                        }
                    }
                }
                store.save_profile(&name, &snapshot).await?
            }
            NotebookRequest::Load { name } => {
                self.checkpoint(&[]).await?;
                let snapshot = self
                    .store
                    .as_mut()
                    .ok_or_else(|| "profiles require a notebook storage identity".to_string())?
                    .load_profile(&name)
                    .await?;
                let available = self.names().await?;
                let collisions: Vec<_> = entries(&snapshot)
                    .iter()
                    .filter_map(|e| e["name"].as_str())
                    .filter(|n| available.iter().any(|a| a == n) || self.baseline.contains(*n))
                    .map(str::to_owned)
                    .collect();
                if !collisions.is_empty() {
                    return Err(format!(
                        "profile conflicts with existing bindings: {}. Release them before loading",
                        collisions.join(", ")
                    ));
                }
                if let Err(error) = self.rpc("restore", vec![snapshot]).await {
                    self.restore(false, None).await?;
                    return Err(error);
                }
                self.checkpoint(&[]).await?;
                json!({"loaded":name})
            }
            NotebookRequest::Pin { names, hook } => self.pin(names, hook).await?,
            NotebookRequest::Unpin { names } => self.unpin(names).await?,
            NotebookRequest::Release { names } => self.release(names).await?,
            NotebookRequest::Prune { query } => {
                let pins = self.pinned_names();
                let names = self
                    .names()
                    .await?
                    .into_iter()
                    .filter(|n| matches(&query, n) && !pins.contains(n))
                    .collect();
                self.release(names).await?
            }
            NotebookRequest::List { .. } | NotebookRequest::Diagnostics => {
                return Err("read-only request was not routed by provider".to_string());
            }
        };
        Ok(NotebookControlResult::with_details(label, details))
    }

    fn pinned_names(&self) -> HashSet<String> {
        self.status["bindings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|e| pinned(e))
            .filter_map(|e| e["name"].as_str().map(str::to_owned))
            .collect()
    }

    async fn pin(
        &mut self,
        names: Vec<String>,
        hook: Option<NotebookHook>,
    ) -> Result<Value, String> {
        self.validate_names(&names).await?;
        let all_names = self.names().await?;
        let captured = self
            .rpc(
                "capture",
                vec![
                    json!(all_names),
                    json!(
                        PersistenceBudget::from_heap_mib(self.options.max_heap_mib).payload_bytes()
                    ),
                ],
            )
            .await?;
        for name in &names {
            let entry = entries(&captured)
                .into_iter()
                .find(|e| e["name"] == *name)
                .ok_or_else(|| format!("binding cannot be captured durably: {name}"))?;
            if hook.is_some_and(|h| h != NotebookHook::Removed) && entry["kind"] != "function" {
                return Err(format!("hooks require a self-contained function: {name}"));
            }
        }
        let previous: Vec<_> = self.status["bindings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(pinned_entry)
            .collect();
        let mut configured = previous.clone();
        for name in &names {
            let old = configured.iter().find(|e| e["name"] == *name).cloned();
            configured.retain(|e| e["name"] != *name);
            let mut entry = old.unwrap_or_else(|| json!({"name":name}));
            entry["pinned"] = json!(true);
            if let Some(hook) = hook {
                if hook == NotebookHook::Removed {
                    if let Some(object) = entry.as_object_mut() {
                        object.remove("hook");
                    }
                } else {
                    entry["hook"] = serde_json::to_value(hook).map_err(|e| e.to_string())?;
                }
            }
            configured.push(entry);
        }
        let previous_project_names = self.rpc("projectBindings", vec![]).await?;
        self.rpc("promote", vec![json!(names)]).await?;
        if let Err(error) = self.rpc("configurePins", vec![json!(configured)]).await {
            let rollback = self
                .rpc("syncProjectBindings", vec![previous_project_names])
                .await;
            return Err(format!(
                "{error}{}",
                rollback
                    .err()
                    .map(|e| format!("; promotion rollback failed: {e}. Restart before retrying"))
                    .unwrap_or_default()
            ));
        }
        if let Err(error) = self.checkpoint(&[]).await {
            // Project metadata may have committed before a failed private write.
            // A runtime-only rollback would falsely suggest durable pins reverted.
            let cleanup = self.shutdown().await;
            return Err(format!(
                "pin persistence failed: {error}. Runtime invalidated. Restart before retrying{}",
                cleanup
                    .err()
                    .map(|e| format!("; cleanup failed: {e}"))
                    .unwrap_or_default()
            ));
        }
        Ok(json!({"names":names,"pinned":true,"hook":hook,"checkpoint":self.checkpoint_details}))
    }

    async fn unpin(&mut self, names: Vec<String>) -> Result<Value, String> {
        if names.is_empty() || names.iter().any(|name| !identifier(name)) {
            return Err("unpin requires valid binding names".to_string());
        }
        if self.kernel.is_some() {
            let remaining: Vec<_> = self.status["bindings"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| pinned(entry) && !names.iter().any(|name| entry["name"] == *name))
                .collect();
            self.rpc("configurePins", vec![json!(remaining)]).await?;
        }
        let result = match &mut self.store {
            Some(store) => store.unpin(&names).await,
            None => Ok(json!({"unpinned":names})),
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.persistence_error = Some(error.clone());
                let cleanup = self.shutdown().await;
                return Err(format!(
                    "unpin persistence failed: {error}. Restart before retrying{}",
                    cleanup
                        .err()
                        .map(|e| format!("; cleanup failed: {e}"))
                        .unwrap_or_default()
                ));
            }
        };
        if let Some(entries) = self.snapshot["entries"].as_array_mut() {
            for entry in entries {
                if names.iter().any(|name| entry["name"] == *name)
                    && let Some(entry) = entry.as_object_mut()
                {
                    entry.remove("pinned");
                    entry.remove("hook");
                }
            }
        }
        if self.kernel.is_some() {
            self.refresh_status().await?;
        }
        Ok(result)
    }

    async fn validate_names(&mut self, names: &[String]) -> Result<(), String> {
        if names.is_empty() {
            return Err("names must not be empty".to_string());
        }
        let available = self.names().await?;
        for name in names {
            if !identifier(name) || !available.contains(name) {
                return Err(format!("notebook binding not found: {name}"));
            }
        }
        Ok(())
    }

    async fn release(&mut self, names: Vec<String>) -> Result<Value, String> {
        if names.is_empty() {
            return Ok(json!({"released":[],"disposed":[],"failures":[]}));
        }
        self.validate_names(&names).await?;
        let pins = self.pinned_names();
        if let Some(name) = names.iter().find(|n| pins.contains(*n)) {
            return Err(format!(
                "pinned binding cannot be released: {name}. Unpin it first"
            ));
        }
        let status = self.rpc("status", vec![json!(names)]).await?;
        let lexical = status["bindings"]
            .as_array()
            .is_some_and(|bindings| bindings.iter().any(|b| b["globalProperty"] == false));
        if lexical {
            self.checkpoint(&names).await?;
            let checkpoint = self.checkpoint_details.clone();
            let disposal = self.dispose_all().await?;
            self.restore(false, None).await?;
            // A concurrent project pin can defeat the checkpoint deletion. Report
            // the restored inventory, not merely the requested exclusions.
            let remaining = self.names().await?;
            let failures: Vec<_> = names
                .iter()
                .filter(|name| remaining.contains(name))
                .map(|name| json!({"name":name,"reason":"concurrent project state retained this binding"}))
                .collect();
            let released: Vec<_> = names
                .into_iter()
                .filter(|name| !remaining.contains(name))
                .collect();
            Ok(
                json!({"released":released,"failures":failures,"restartRequired":true,"disposal":disposal,"checkpoint":checkpoint}),
            )
        } else {
            let result = self.rpc("release", vec![json!(names)]).await?;
            self.checkpoint(&[]).await?;
            Ok(result)
        }
    }

    async fn dispose_all(&mut self) -> Result<Value, String> {
        let names = self.names().await?;
        self.rpc("dispose", vec![json!(names)]).await
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), String> {
        match self.kernel.take() {
            Some(mut kernel) => kernel.shutdown().await.map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

fn pinned(entry: &Value) -> bool {
    entry["pinned"] == true
}
fn pinned_entry(entry: &Value) -> bool {
    pinned(entry)
}
fn entries(snapshot: &Value) -> Vec<Value> {
    snapshot["entries"].as_array().cloned().unwrap_or_default()
}
fn recovery_error() -> String {
    "notebook kernel is unavailable. Use notebook restart or reset to recover in this thread"
        .to_string()
}
pub(crate) fn status_result(
    status: &Value,
    checkpoint: &Value,
    persistence_error: Option<&str>,
    query: Option<&str>,
    active: bool,
) -> NotebookControlResult {
    let mut details = status.clone();
    if !details.is_object() {
        details = json!({});
    }
    details["state"] = json!(if active { "running" } else { "idle" });
    details["cached"] = json!(active);
    details["checkpoint"] = bounded_checkpoint(checkpoint);
    if let Some(error) = persistence_error {
        details["persistenceError"] = json!(bound_text(error, 1024));
    }
    if let Some(query) = query {
        let bindings = status["bindings"].as_array().cloned().unwrap_or_default();
        let matching: Vec<_> = bindings
            .into_iter()
            .filter(|b| b["name"].as_str().is_some_and(|n| matches(query, n)))
            .collect();
        let total = matching.len();
        let matching = bounded_bindings(matching, 4096);
        details["omittedMatches"] = json!(total - matching.len());
        details["matches"] = json!(matching);
        details["query"] = json!(bound_text(query, 256));
    }
    // Never return a huge binding inventory on the provider's prompt path.
    let total = details["bindings"].as_array().map_or(0, Vec::len);
    details["userBindings"] = json!(total);
    let bindings = bounded_bindings(
        details["bindings"].as_array().cloned().unwrap_or_default(),
        4096,
    );
    details["omittedBindings"] = json!(total - bindings.len());
    details["pinnedBindings"] = json!(
        status["bindings"]
            .as_array()
            .map_or(0, |bindings| bindings.iter().filter(|b| pinned(b)).count())
    );
    details["bindings"] = json!(bindings);
    let npm_notice = status["npmImportsNotice"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| import_history::notice(&[]));
    let profile_notice = status
        .get("defaultProfile")
        .map(|profile| format!("\nDefault profile {}", format_details(profile, 2048)))
        .unwrap_or_default();
    let inventory = if query.is_some() {
        &details["matches"]
    } else {
        &details["bindings"]
    };
    let omitted = if query.is_some() {
        &details["omittedMatches"]
    } else {
        &details["omittedBindings"]
    };
    NotebookControlResult {
        message: format!(
            "{}\nNotebook {} · {} completed cells · {total} top-level binding(s)\nMemory {}\nRetained {} binding(s) · {} serialized bytes · {} pinned\nCheckpoint {}{}{}{}\nBindings{}: {} · {} omitted",
            npm_notice,
            if active { "running (cached)" } else { "idle" },
            status["userCells"].as_u64().unwrap_or(0),
            format_details(&details["memory"], 1024),
            status["retainedBindings"].as_u64().unwrap_or(0),
            status["retainedBytes"].as_u64().unwrap_or(0),
            details["pinnedBindings"],
            format_details(checkpoint, 4096),
            persistence_error
                .map(|e| format!("\nPersistence failed: {}", bound_text(e, 1024)))
                .unwrap_or_default(),
            status["npmImportsError"]
                .as_str()
                .map(|e| format!(
                    "\nNotebook npm inventory was not updated: {}",
                    bound_text(e, 1024)
                ))
                .unwrap_or_default(),
            profile_notice,
            query
                .map(|q| format!(" matching {}", json!(bound_text(q, 256))))
                .unwrap_or_default(),
            inventory,
            omitted,
        ),
        details,
    }
}

fn bounded_bindings(bindings: Vec<Value>, mut budget: usize) -> Vec<Value> {
    bindings
        .into_iter()
        .filter(|binding| {
            let bytes = binding.to_string().len() + 1;
            if bytes > budget {
                return false;
            }
            budget -= bytes;
            true
        })
        .take(100)
        .collect()
}

fn bound_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub(16).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &text[..end])
}

fn bounded_checkpoint(checkpoint: &Value) -> Value {
    if checkpoint.to_string().len() <= 2048 {
        return checkpoint.clone();
    }
    json!({"state":checkpoint["state"],"sessionEntries":checkpoint["sessionEntries"],"projectEntries":checkpoint["projectEntries"],"omittedConflicts":checkpoint["conflicts"].as_array().map_or(0, Vec::len),"omittedSkipped":checkpoint["skipped"].as_array().map_or(0, Vec::len),"truncated":true})
}
