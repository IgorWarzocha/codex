use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::CodeModeToolMessages;
use codex_protocol::openai_models::ToolMessage;

use crate::session::step_context::StepContext;

pub(super) fn validate_access(
    step: &StepContext,
    kernel_cwd: &codex_utils_absolute_path::AbsolutePathBuf,
) -> Result<(), String> {
    let config = &step.turn.config;
    let environment = step
        .environments
        .single_local_environment()
        .ok_or_else(|| "Notebook requires a single ready local environment".to_string())?;
    if !matches!(
        config.permissions.effective_permission_profile(),
        PermissionProfile::Disabled
    ) || !matches!(
        environment.permission_profile(),
        PermissionProfile::Disabled
    ) || config.permissions.network.is_some()
        || step.turn.network.is_some()
    {
        return Err("Deno Jupyter has unrestricted filesystem, network and subprocess access. Notebook requires danger-full-access without a managed network proxy; use the V8 runtime for sandboxed execution".to_string());
    }
    if step.environments.single_local_environment_cwd().as_ref() != Some(kernel_cwd) {
        return Err("Notebook cannot change execution environments. Start a new thread in the desired directory".to_string());
    }
    Ok(())
}

pub(crate) fn tool_messages(base: Option<&CodeModeToolMessages>) -> CodeModeToolMessages {
    let mut messages = base.cloned().unwrap_or_default();
    messages.exec = Some(ToolMessage {
        description: Some(include_str!("notebook_prompt.md").to_string()),
        ..Default::default()
    });
    messages
}
