//! Maps accepted native tool calls to reached directories. Direct and Notebook dispatch share this boundary.

use super::AgentsMdManager;
use crate::agents_md::nested::InstructionTarget;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::unified_exec::ExecCommandArgs;
use crate::tools::handlers::unified_exec::get_command;
use crate::tools::handlers::unified_exec::shell_mode_for_environment;
use codex_exec_server::GetMetadataOptions;
use codex_protocol::parse_command::ParsedCommand;
use codex_shell_command::parse_command::discovery_output_cwds;
use codex_shell_command::parse_command::parse_command;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use std::io;
use std::sync::Arc;

// Inspect bounded, model-visible discovery output, never walk a returned subtree.
const MAX_OUTPUT_PATHS: usize = 256;

#[derive(Clone)]
pub(super) struct DiscoveryCommand {
    environment_id: String,
    cwd: PathUri,
    commands: Vec<ParsedCommand>,
    output_bases: Vec<SearchOutputBase>,
}

#[derive(Clone)]
struct SearchOutputBase {
    cwd: PathUri,
    filenames_only: bool,
}

#[derive(Deserialize)]
struct DiscoveryArgs {
    environment_id: Option<String>,
    workdir: Option<String>,
    path: Option<String>,
    session_id: Option<i64>,
}

impl AgentsMdManager {
    pub(crate) fn observe_tool_result<'a>(
        &'a self,
        invocation: &'a ToolInvocation,
        result: &dyn ToolOutput,
    ) -> impl std::future::Future<Output = ()> + Send + 'a + use<'a> {
        // ToolOutput is Send, not Sync. Copy its owned discovery projection before awaiting.
        let observation = (|| {
            if !invocation.tool_name.is_default_namespace() || !result.success_for_logging() {
                return None;
            }
            if invocation.tool_name.name == "apply_patch" {
                return Some((
                    DiscoveryArgs {
                        environment_id: None,
                        workdir: None,
                        path: None,
                        session_id: None,
                    },
                    None,
                ));
            }
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return None;
            };
            let args = serde_json::from_str::<DiscoveryArgs>(arguments).ok()?;
            let name = invocation.tool_name.name.as_str();
            if !matches!(name, "exec_command" | "write_stdin" | "view_image") {
                return None;
            }
            let output =
                (name != "view_image").then(|| result.code_mode_result(&invocation.payload));
            Some((args, output))
        })();
        self.observe_discovery(invocation, observation)
    }

    async fn observe_discovery(
        &self,
        invocation: &ToolInvocation,
        observation: Option<(DiscoveryArgs, Option<serde_json::Value>)>,
    ) {
        let Some((args, output)) = observation else {
            return;
        };
        let name = invocation.tool_name.name.as_str();
        // Commands may edit already-reached policy files even when the parser classifies
        // them as Unknown. Invalidate once per filesystem tool, without executing probes.
        self.state.lock().await.nested_revision += 1;
        if name == "apply_patch" {
            return;
        }
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return;
        };
        let command = match name {
            "write_stdin" => {
                let Some(id) = args.session_id else { return };
                let mut state = self.state.lock().await;
                let command = state.discovery_processes.get(&id).cloned();
                if output.as_ref().is_some_and(|output| {
                    output
                        .get("session_id")
                        .and_then(serde_json::Value::as_i64)
                        .is_none()
                }) {
                    state.discovery_processes.remove(&id);
                }
                let Some(command) = command else { return };
                command
            }
            _ => {
                let environment = match args.environment_id.as_deref() {
                    Some(id) => invocation
                        .step_context
                        .environments
                        .turn_environments()
                        .find(|environment| environment.selection.environment_id == id),
                    None => invocation.step_context.environments.primary(),
                };
                let Some(environment) = environment else {
                    return;
                };
                let Ok(cwd) = environment
                    .cwd()
                    .join(args.workdir.as_deref().unwrap_or(""))
                else {
                    return;
                };
                if name == "view_image" {
                    if let Some(path) = args.path.and_then(|path| cwd.join(&path).ok()) {
                        self.remember_reached(environment, vec![path]).await;
                    }
                    return;
                }
                let Ok(exec_args) = serde_json::from_str::<ExecCommandArgs>(arguments) else {
                    return;
                };
                let shell = environment
                    .shell
                    .clone()
                    .map(Arc::new)
                    .unwrap_or_else(|| invocation.session.user_shell());
                let mode = shell_mode_for_environment(
                    &invocation.turn.unified_exec_shell_mode,
                    environment.environment.as_ref(),
                );
                let Ok(resolved) = get_command(
                    &exec_args,
                    shell,
                    &mode,
                    environment.config().allow_login_shell,
                ) else {
                    return;
                };
                let commands = parse_command(&resolved.command);
                if commands
                    .iter()
                    .all(|command| matches!(command, ParsedCommand::Unknown { .. }))
                {
                    return;
                }
                let output_bases = discovery_output_cwds(&exec_args.cmd)
                    .into_iter()
                    .filter_map(|base| {
                        cwd.join(&base.cwd).ok().map(|cwd| SearchOutputBase {
                            cwd,
                            filenames_only: base.filenames_only,
                        })
                    })
                    .collect();
                let command = DiscoveryCommand {
                    environment_id: environment.selection.environment_id.clone(),
                    cwd,
                    commands,
                    output_bases,
                };
                if let Some(id) = output
                    .as_ref()
                    .and_then(|output| output.get("session_id"))
                    .and_then(serde_json::Value::as_i64)
                {
                    self.state
                        .lock()
                        .await
                        .discovery_processes
                        .insert(id, command.clone());
                }
                command
            }
        };
        let Some(environment) = invocation
            .step_context
            .environments
            .turn_environments()
            .find(|environment| environment.selection.environment_id == command.environment_id)
        else {
            return;
        };
        let mut paths = command
            .commands
            .iter()
            .filter_map(|parsed| {
                let path = match parsed {
                    ParsedCommand::Read { path, .. } => path.to_string_lossy().into_owned(),
                    ParsedCommand::ListFiles { path, .. } | ParsedCommand::Search { path, .. } => {
                        path.clone().unwrap_or_default()
                    }
                    ParsedCommand::Unknown { .. } => return None,
                };
                command.cwd.join(&path).ok()
            })
            .collect::<Vec<_>>();
        let text = output
            .as_ref()
            .and_then(|output| output.get("output"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        for line in text.lines().take(MAX_OUTPUT_PATHS) {
            for base in &command.output_bases {
                let Some(candidate) = output_path(line, base.filenames_only) else {
                    continue;
                };
                if let Ok(path) = base.cwd.join(candidate) {
                    paths.push(path);
                }
            }
        }
        self.remember_reached(environment, paths).await;
    }

    async fn remember_reached(&self, environment: &TurnEnvironment, paths: Vec<PathUri>) {
        let fs = environment.environment.get_filesystem();
        let sandbox = (!environment
            .permission_profile()
            .file_system_sandbox_policy()
            .has_full_disk_read_access())
        .then(|| environment.sandbox_context(/*additional_permissions*/ None));
        let mut targets = Vec::new();
        for path in paths {
            let metadata = match fs
                .get_metadata(&path, GetMetadataOptions::default(), sandbox.as_ref())
                .await
            {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%path, %error, "could not inspect nested instruction discovery target");
                    continue;
                }
            };
            let directory = if metadata.is_directory {
                Some(path)
            } else if metadata.is_file {
                path.parent()
            } else {
                None
            };
            if let Some(directory) = directory {
                targets.push(InstructionTarget {
                    environment_id: environment.selection.environment_id.clone(),
                    directory,
                });
            }
        }
        let mut state = self.state.lock().await;
        for target in targets {
            if !state.nested_targets.contains(&target) {
                state.nested_targets.push(target);
            }
        }
    }
}

fn output_path(line: &str, filenames_only: bool) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.contains('\0') || line.starts_with('<') {
        return None;
    }
    // Skip a drive prefix before splitting grep's file:line:content form.
    let start = usize::from(
        line.as_bytes().get(1) == Some(&b':')
            && line
                .as_bytes()
                .get(2)
                .is_some_and(|byte| matches!(byte, b'/' | b'\\')),
    ) * 2;
    if let Some(separator) = line[start..].find(':') {
        return Some(&line[..start + separator]);
    }
    filenames_only.then_some(line)
}
