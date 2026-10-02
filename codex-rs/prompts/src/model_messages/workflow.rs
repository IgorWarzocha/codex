//! Compact generic workflow prose for catalogs owned by the default manager.
//! Explicit catalogs and runtime configuration remain authoritative elsewhere.

use super::ResolvedModelMessages;
use codex_protocol::openai_models::ModelMessages;

const TOKEN_BUDGET_GUIDANCE: &str = "Before a context reset, checkpoint the goal, decisions, progress, learnings, and next steps in `notes`. Include window and item IDs for active requests and important evidence. After reset, read the checkpoint and recover only missing details from `history`. The next window does not automatically include this conversation. Keep this bookkeeping out of user-facing replies.";
const TOKEN_BUDGET_REMINDER: &str = "<context_window_reminder>Only {n_remaining} tokens remain. Save a checkpoint in `notes` with the goal, decisions, progress, next steps, and relevant window and item IDs. Then call `functions.new_context`. The next window will not automatically include this conversation.</context_window_reminder>";
const TOKEN_BUDGET_FALLBACK: &str = "<context_window_reminder>Context is exhausted. Do not continue the task or answer here. Make exactly one write or append to `notes` with the goal, decisions, progress, learnings, next steps, and relevant window and item IDs. After its result, call `functions.new_context`. Use no other tools. The next window will not automatically include this conversation.</context_window_reminder>";

/// Call only for the default catalog. Replace named prose fields, not feature settings,
/// tool contracts, permission policies, or explicit empty-string suppression.
pub fn apply_default_catalog_workflow(messages: &mut ModelMessages) {
    let bundled = ResolvedModelMessages::bundled();
    replace_nonempty(
        &mut messages.persistent_instructions,
        bundled.persistent_instructions(),
    );
    if let Some(modes) = messages.collaboration_modes.as_mut() {
        let defaults = bundled.collaboration_modes();
        replace_nonempty(&mut modes.default, defaults.default.text());
        replace_nonempty(&mut modes.plan, defaults.plan.text());
    }
    if let Some(agents) = messages.multi_agent.as_mut() {
        let defaults = bundled.multi_agent();
        if let Some(role) = agents.role.as_mut() {
            replace_nonempty(&mut role.root, defaults.root.text());
            replace_nonempty(&mut role.subagent, defaults.subagent.text());
        }
        if let Some(mode) = agents.mode.as_mut() {
            replace_nonempty(&mut mode.explicit, defaults.explicit.text());
            replace_nonempty(&mut mode.proactive, defaults.proactive.text());
        }
    }
    if let Some(budget) = messages.token_budget.as_mut() {
        replace_nonempty_text(&mut budget.guidance_message, TOKEN_BUDGET_GUIDANCE);
        replace_nonempty_text(&mut budget.reminder_message_template, TOKEN_BUDGET_REMINDER);
        replace_nonempty_text(
            &mut budget.auto_compact_fallback_prompt,
            TOKEN_BUDGET_FALLBACK,
        );
    }
}

fn replace_nonempty(text: &mut Option<String>, compact: &str) {
    if let Some(text) = text.as_mut() {
        replace_nonempty_text(text, compact);
    }
}

fn replace_nonempty_text(text: &mut String, compact: &str) {
    if !text.trim().is_empty() {
        *text = compact.to_owned();
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
