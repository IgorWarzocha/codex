//! Loads only instruction chains reached by tool discovery, using the startup loader's policy.

use super::InstructionProvenance;
use super::LoadedAgentsMd;
use super::agents_md_paths_in_dirs;
use super::project_search_dirs;
use super::read_agents_md_files;
use crate::config::Config;
use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::session::turn_context::TurnEnvironment;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::io;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct InstructionTarget {
    pub(crate) environment_id: String,
    pub(crate) directory: PathUri,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct NestedInstruction {
    pub(crate) environment_id: String,
    pub(crate) source_path: PathUri,
    pub(crate) contents: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct NestedAgentsMd {
    #[serde(default)]
    pub(crate) targets: Vec<InstructionTarget>,
    #[serde(default)]
    pub(crate) entries: Vec<NestedInstruction>,
}

pub(crate) async fn load_nested_instructions(
    config: &Config,
    environments: &TurnEnvironmentSnapshot,
    startup: Option<&LoadedAgentsMd>,
    targets: Vec<InstructionTarget>,
) -> io::Result<NestedAgentsMd> {
    let mut loaded = NestedAgentsMd::default();
    if config.active_project.is_untrusted() || config.project_doc_max_bytes == 0 {
        // Discovery is continuity metadata, not permission to expose file contents.
        // Retain it across restricted windows so later trust/budget restoration can reload.
        return Ok(NestedAgentsMd {
            targets,
            ..Default::default()
        });
    }
    let mut remaining = config.project_doc_max_bytes;
    if let Some(startup) = startup {
        for entry in &startup.entries {
            if matches!(entry.provenance, InstructionProvenance::Project { .. }) {
                remaining = remaining.saturating_sub(entry.contents.len());
            }
        }
    }
    for environment in environments.turn_environments() {
        let environment_id = &environment.selection.environment_id;
        let applicable = targets
            .iter()
            .filter(|target| &target.environment_id == environment_id)
            .cloned()
            .collect::<Vec<_>>();
        if applicable.is_empty() {
            continue;
        }
        let sandbox = (!environment
            .permission_profile()
            .file_system_sandbox_policy()
            .has_full_disk_read_access())
        .then(|| environment.sandbox_context(/*additional_permissions*/ None));
        match load_environment(config, environment, applicable, remaining, sandbox.as_ref()).await {
            Ok(nested) => {
                remaining = remaining.saturating_sub(
                    nested
                        .entries
                        .iter()
                        .map(|entry| entry.contents.len())
                        .sum(),
                );
                loaded.targets.extend(nested.targets);
                loaded.entries.extend(nested.entries);
            }
            Err(error) if sandbox.is_none() => {
                tracing::error!(environment_id, %error, "error loading nested AGENTS.md instructions")
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "failed to load nested AGENTS.md instructions for environment `{environment_id}`: {error}"
                    ),
                ));
            }
        }
    }
    Ok(loaded)
}

async fn load_environment(
    config: &Config,
    environment: &TurnEnvironment,
    targets: Vec<InstructionTarget>,
    remaining: usize,
    sandbox: Option<&codex_file_system::FileSystemSandboxContext>,
) -> io::Result<NestedAgentsMd> {
    let fs = environment.environment.get_filesystem();
    let startup_dirs = project_search_dirs(config, environment.cwd(), fs.as_ref(), sandbox).await?;
    let root = startup_dirs.first().unwrap_or(environment.cwd());
    let mut loaded = NestedAgentsMd::default();
    let mut dirs = Vec::new();
    let mut seen = startup_dirs.iter().cloned().collect::<HashSet<_>>();
    for target in targets {
        // A trusted project does not authorize other projects. Startup directories
        // remain owned by the original loader, even if their override choice changes.
        if !target.directory.starts_with(root) || startup_dirs.contains(&target.directory) {
            continue;
        }
        let mut chain = Vec::new();
        let mut cursor = target.directory.clone();
        while cursor != *root {
            chain.push(cursor.clone());
            let Some(parent) = cursor.parent() else { break };
            cursor = parent;
        }
        chain.reverse();
        dirs.extend(chain.into_iter().filter(|dir| seen.insert(dir.clone())));
        loaded.targets.push(target);
    }
    if remaining == 0 || dirs.is_empty() {
        return Ok(loaded);
    }
    // One bounded parallel probe per unique reached ancestor, not one per output file.
    let paths =
        agents_md_paths_in_dirs(config, environment.cwd(), dirs, fs.as_ref(), sandbox).await?;
    let environment_id = &environment.selection.environment_id;
    if let Some(docs) = read_agents_md_files(
        fs.as_ref(),
        environment_id,
        environment.cwd(),
        paths,
        remaining,
        sandbox,
    )
    .await?
    {
        loaded.entries = docs
            .entries
            .into_iter()
            .filter_map(|entry| {
                let InstructionProvenance::Project { source_path, .. } = entry.provenance else {
                    return None;
                };
                Some(NestedInstruction {
                    environment_id: environment_id.clone(),
                    source_path,
                    contents: entry.contents,
                })
            })
            .collect();
    }
    Ok(loaded)
}
