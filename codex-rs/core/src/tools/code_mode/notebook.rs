use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::CodeModeToolMessages;
use codex_protocol::openai_models::ToolMessage;

use crate::session::step_context::StepContext;

use super::CodeModeService;

pub(crate) async fn context_status(
    service: &CodeModeService,
    step: &StepContext,
) -> Option<String> {
    if step.turn.config.code_mode.runtime != codex_features::CodeModeRuntime::Notebook
        || !step.tool_router.requires_code_mode_worker()
    {
        return None;
    }
    Some(
        match service
            .control_notebook(
                codex_notebook::NotebookRequest::Status {
                    query: Some("*".to_string()),
                },
                step,
            )
            .await
        {
            Ok(status) => status.message,
            Err(error) => format!("Notebook status unavailable: {error}"),
        },
    )
}

pub(super) fn validate_access(
    step: &StepContext,
    kernel_cwd: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<(), String> {
    validate_environment(
        &step.turn.config,
        &step.environments,
        step.turn.network.is_some(),
        kernel_cwd,
    )
}

pub(super) fn validate_prewarm_access(
    turn: &crate::session::turn_context::TurnContext,
    kernel_cwd: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<(), String> {
    validate_environment(
        &turn.config,
        &turn.initial_environments,
        turn.network.is_some(),
        kernel_cwd,
    )
}

fn validate_environment(
    config: &crate::config::Config,
    environments: &crate::environment_selection::TurnEnvironmentSnapshot,
    has_network_proxy: bool,
    kernel_cwd: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<(), String> {
    let environment = environments
        .single_local_environment()
        .ok_or_else(|| "Notebook requires a single ready local environment".to_string())?;
    if !matches!(
        config.permissions.effective_permission_profile(),
        PermissionProfile::Disabled
    ) || !matches!(
        environment.permission_profile(),
        PermissionProfile::Disabled
    ) || config.permissions.network.is_some()
        || has_network_proxy
    {
        return Err("Deno Jupyter has unrestricted filesystem, network and subprocess access. Notebook requires danger-full-access without a managed network proxy; use the V8 runtime for sandboxed execution".to_string());
    }
    if environments.single_local_environment_cwd().as_ref() != Some(kernel_cwd) {
        return Err("Notebook cannot change execution environments. Start a new thread in the desired directory".to_string());
    }
    Ok(())
}

pub(crate) fn tool_messages(base: Option<&CodeModeToolMessages>) -> CodeModeToolMessages {
    let mut messages = base.cloned().unwrap_or_default();
    // The runtime owns exec's source/output contract. Other catalog overrides
    // remain available to the notebook usage section and the native wait tool.
    messages.exec = Some(ToolMessage {
        description: Some(include_str!("notebook_prompt.md").to_string()),
        ..Default::default()
    });
    messages
}
