//! Host-owned by-value persistence. Kernels only capture and restore snapshots.
//! Project writes merge private forks under an OS lock, never into live kernels.

pub(crate) mod files;
mod format;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::persistence::PersistenceBudget;
use files::Paths;
use format::Snapshot;

pub(crate) struct Store {
    state: Arc<Mutex<State>>,
}

struct State {
    paths: Paths,
    baseline: Option<Snapshot>,
    budget: PersistenceBudget,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Project {
    schema: u32,
    project: PathBuf,
    snapshot: Snapshot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Session {
    schema: u32,
    snapshot: Snapshot,
    baseline: Snapshot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    schema: u32,
    name: String,
    created_at: u64,
    source_project: PathBuf,
    snapshot: Snapshot,
}

impl Store {
    pub(crate) fn new(codex_home: PathBuf, cwd: &Path, thread_id: &str) -> Result<Self, String> {
        // Cold profile listing and metadata recovery must not depend on a live heap.
        Self::with_budget(
            codex_home,
            cwd,
            thread_id,
            PersistenceBudget::from_heap_mib(None),
        )
    }

    pub(crate) fn with_budget(
        codex_home: PathBuf,
        cwd: &Path,
        thread_id: &str,
        budget: PersistenceBudget,
    ) -> Result<Self, String> {
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                paths: Paths::new(codex_home, cwd, thread_id)?,
                baseline: None,
                budget,
            })),
        })
    }

    /// Construct a source for profile loading without creating project or session paths.
    pub(crate) fn for_profile_reads(
        codex_home: PathBuf,
        cwd: &Path,
        thread_id: &str,
        budget: PersistenceBudget,
    ) -> Result<Self, String> {
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                paths: Paths::for_profile_reads(codex_home, cwd, thread_id)?,
                baseline: None,
                budget,
            })),
        })
    }

    // Filesystem work, lock waits, validation and base64 decoding stay off the
    // async executor. The shared state serializes this Store's operations too.
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| "Notebook storage state lock poisoned")?;
            operation(&mut state)
        })
        .await
        .map_err(|error| format!("Notebook storage task failed: {error}"))?
    }

    pub(crate) async fn load_project(&self) -> Result<Option<Value>, String> {
        self.run(|state| {
            let _lock = files::lock(&state.paths.directory)?;
            let project = read_project(&state.paths, state.budget)?;
            if state.baseline.is_none() {
                // A restarted private fork retains its own observed candidate,
                // even when the shared project advanced while it was stopped.
                state.baseline = read_session(&state.paths, state.budget)?
                    .map(|session| session.baseline)
                    .or_else(|| project.as_ref().map(|project| project.snapshot.clone()));
            }
            project.map(|project| project.snapshot.value()).transpose()
        })
        .await
    }

    pub(crate) async fn load_session(&self) -> Result<Option<Value>, String> {
        self.run(|state| {
            let _lock = files::lock(&state.paths.directory)?;
            read_session(&state.paths, state.budget)?
                .map(|session| session.snapshot.value())
                .transpose()
        })
        .await
    }

    pub(crate) async fn checkpoint(
        &mut self,
        snapshot: &Value,
        project_snapshot: &Value,
    ) -> Result<Value, String> {
        let snapshot = snapshot.clone();
        let project_snapshot = project_snapshot.clone();
        self.run(move |state| {
            let snapshot = Snapshot::parse(&snapshot, state.budget)?;
            let candidate = Snapshot::parse(&project_snapshot, state.budget)?;
            let _lock = files::lock(&state.paths.directory)?;
            let current = read_project(&state.paths, state.budget)?;
            let session = read_session(&state.paths, state.budget)?;
            if state.baseline.is_none() {
                state.baseline = session.map(|session| session.baseline);
            }
            let merged = format::merge(
                state.baseline.as_ref(),
                current.as_ref().map(|project| &project.snapshot),
                &candidate,
                state.budget,
            )?;
            let details = json!({
                "conflicts": merged.conflicts,
                "applied": merged.applied,
                "sessionEntries": snapshot.entries.len(),
                "projectEntries": merged.snapshot.entries.len(),
                "skipped": snapshot.skipped,
            });
            // Encode both before committing, so input and size errors cannot
            // cause a partial commit. IO failures still require explicit recovery.
            let project_bytes = files::encode(&Project {
                schema: 1,
                project: state.paths.project.clone(),
                snapshot: merged.snapshot,
            }, state.budget.file_bytes())?;
            let session_bytes = files::encode(&Session {
                schema: 1,
                snapshot,
                baseline: merged.baseline.clone(),
            }, state.budget.file_bytes())?;
            // Project first: if the private write fails, a retry can re-merge.
            // Advancing a durable candidate baseline before the project commit
            // would instead permanently lose an unapplied change.
            files::atomic_write_bytes(
                &state.paths.directory.join("project.json"),
                &project_bytes,
            ).map_err(|error| format!("Notebook project checkpoint failed and may have committed: {error}. Restart before further mutations."))?;
            files::atomic_write_bytes(
                &state.paths.session,
                &session_bytes,
            ).map_err(|error| format!("Notebook project committed but private session checkpoint failed: {error}. Restart before further mutations."))?;
            state.baseline = Some(merged.baseline);
            Ok(details)
        })
        .await
    }

    pub(crate) async fn reset_session(&mut self) -> Result<(), String> {
        self.run(|state| {
            let _lock = files::lock(&state.paths.directory)?;
            files::remove(&state.paths.session)?;
            state.baseline = None;
            Ok(())
        })
        .await
    }

    pub(crate) async fn import_history(
        &self,
    ) -> Result<crate::import_history::ImportHistory, String> {
        self.run(|state| {
            Ok(crate::import_history::ImportHistory::new(
                state.paths.clone(),
            ))
        })
        .await
    }

    /// Metadata-only recovery, including when a startup hook prevents capture.
    pub(crate) async fn unpin(&mut self, names: &[String]) -> Result<Value, String> {
        let names: BTreeSet<_> = names.iter().cloned().collect();
        self.run(move |state| {
            if names.is_empty() || names.iter().any(|name| !format::identifier(name)) {
                return Err("Unpin requires valid notebook binding names".into());
            }
            let _lock = files::lock(&state.paths.directory)?;
            let mut project = read_project(&state.paths, state.budget)?;
            let mut session = read_session(&state.paths, state.budget)?;
            let mut found = BTreeSet::new();
            if let Some(project) = &mut project {
                found.extend(project.snapshot.unpin(&names));
            }
            if let Some(session) = &mut session {
                found.extend(session.snapshot.unpin(&names));
                session.baseline.unpin(&names);
            }
            let missing: Vec<_> = names.difference(&found).cloned().collect();
            let project_bytes = project.as_ref().map(|value| files::encode(value, state.budget.file_bytes())).transpose()?;
            let session_bytes = session.as_ref().map(|value| files::encode(value, state.budget.file_bytes())).transpose()?;
            if let Some(project) = &project_bytes {
                files::atomic_write_bytes(&state.paths.directory.join("project.json"), project)
                    .map_err(|error| format!("Notebook project unpin failed and may have committed: {error}. Restart before further mutations."))?;
            }
            if let Some(session) = &session_bytes {
                files::atomic_write_bytes(&state.paths.session, session)
                    .map_err(|error| format!("Notebook project unpin committed but private session update failed: {error}. Restart before further mutations."))?;
            }
            if let Some(baseline) = &mut state.baseline {
                baseline.unpin(&names);
            }
            Ok(json!({ "unpinned": found, "missing": missing }))
        })
        .await
    }

    pub(crate) async fn save_profile(&self, name: &str, snapshot: &Value) -> Result<Value, String> {
        format::profile_name(name)?;
        let name = name.to_owned();
        let snapshot = snapshot.clone();
        self.run(move |state| {
            let snapshot = Snapshot::parse(&snapshot, state.budget)?;
            let path = state.paths.profiles.join(&name);
            files::directory(&path)?;
            let _lock = files::lock(&path)?;
            let profile = Profile {
                schema: 1,
                name,
                created_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_millis()
                    .try_into()
                    .map_err(|_| "Notebook profile timestamp overflow")?,
                source_project: state.paths.project.clone(),
                snapshot,
            };
            files::atomic_write(
                &path.join("profile.json"),
                &profile,
                state.budget.file_bytes(),
            )?;
            Ok(profile.summary())
        })
        .await
    }

    pub(crate) async fn load_profile(&self, name: &str) -> Result<Value, String> {
        format::profile_name(name)?;
        let name = name.to_owned();
        self.run(move |state| {
            let path = state.paths.profiles.join(&name);
            if !path.exists() {
                return Err(format!("Notebook profile not found: {name}"));
            }
            // Writers atomically replace profile.json. Reads need no write lock,
            // which also lets ephemeral notebooks load profiles without disk writes.
            read_profile(&path, &name, state.budget)?
                .ok_or_else(|| format!("Notebook profile not found: {name}"))?
                .snapshot
                .value()
        })
        .await
    }

    pub(crate) async fn list_profiles(&self, query: Option<&str>) -> Result<Value, String> {
        let query = query.unwrap_or("*").to_owned();
        if query.len() > 4096 {
            return Err("Notebook profile query exceeds 4096 bytes".into());
        }
        self.run(move |state| {
            files::directory(&state.paths.profiles)?;
            let mut profiles = Vec::new();
            for entry in
                std::fs::read_dir(&state.paths.profiles).map_err(|error| error.to_string())?
            {
                let entry = entry.map_err(|error| error.to_string())?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if format::profile_name(&name).is_err() || !crate::control::matches(&query, &name) {
                    continue;
                }
                let _lock = files::lock(&entry.path())?;
                if let Some(profile) = read_profile(&entry.path(), &name, state.budget)? {
                    profiles.push(profile.summary());
                }
            }
            profiles.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Ok(json!({ "profiles": profiles }))
        })
        .await
    }
}

fn read_project(paths: &Paths, budget: PersistenceBudget) -> Result<Option<Project>, String> {
    let mut project: Option<Project> =
        files::read(&paths.directory.join("project.json"), budget.file_bytes())?;
    if let Some(project) = &mut project {
        if project.schema != 1 || project.project != paths.project {
            return Err("Notebook project schema or identity mismatch".into());
        }
        project.snapshot.validate(budget)?;
    }
    Ok(project)
}

fn read_session(paths: &Paths, budget: PersistenceBudget) -> Result<Option<Session>, String> {
    let mut session: Option<Session> = files::read(&paths.session, budget.file_bytes())?;
    if let Some(session) = &mut session {
        if session.schema != 1 {
            return Err("Notebook session schema mismatch".into());
        }
        session.snapshot.validate(budget)?;
        session.baseline.validate(budget)?;
    }
    Ok(session)
}

fn read_profile(
    path: &Path,
    name: &str,
    budget: PersistenceBudget,
) -> Result<Option<Profile>, String> {
    let mut profile: Option<Profile> =
        files::read(&path.join("profile.json"), budget.file_bytes())?;
    if let Some(profile) = &mut profile {
        if profile.schema != 1 || profile.name != name {
            return Err("Notebook profile schema or identity mismatch".into());
        }
        profile.snapshot.validate(budget)?;
    }
    Ok(profile)
}

impl Profile {
    fn summary(&self) -> Value {
        let values = self
            .snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == format::Kind::Value)
            .count();
        json!({
            "name": self.name,
            "createdAt": self.created_at,
            "sourceProject": self.source_project,
            "values": values,
            "definitions": self.snapshot.entries.len() - values,
            "skipped": self.snapshot.skipped.len(),
        })
    }
}
