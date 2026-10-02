//! Compact generic workflow prose for catalogs owned by the default manager.
//! Explicit catalogs and runtime configuration remain authoritative elsewhere.

use super::ResolvedModelMessages;
use super::permissions::DEFAULT_CATALOG_ON_REQUEST_AUTO_REVIEW;
use codex_protocol::openai_models::ModelMessages;

const TOKEN_BUDGET_GUIDANCE: &str = concat!(
    "Before context reset: `notes` checkpoint of active request, decisions, progress, learnings, next steps, and known window and item IDs for important evidence. After reset: read hinted notes and resume; `history` only for missing details. No automatic conversation carryover. ",
    "After substantial work, save useful new findings, decisions, progress or resumable state in notes as your last tool calls before replying. Skip completion notes for brief clarifications, routine lookups, acknowledgements and unchanged state. Explicit checkpoints and context reminders still apply. ",
    "Include useful deferred ideas and tasks, even unrelated ones, when checkpointing. Recording is not permission to implement. Include note paths in agent handoffs. Bookkeeping out of user-facing replies"
);
const TOKEN_BUDGET_REMINDER: &str = "<context_window_reminder>Remaining tokens: {n_remaining}. `notes` checkpoint: goal, decisions, progress, next steps, relevant window and item IDs. Then `functions.new_context`. No automatic conversation carryover</context_window_reminder>";
const TOKEN_BUDGET_FALLBACK: &str = "<context_window_reminder>Context exhausted. No task continuation or answer here. Exactly one `notes` write or append: goal, decisions, progress, learnings, next steps, relevant window and item IDs. After its result: `functions.new_context`. No other tools. No automatic conversation carryover</context_window_reminder>";

/// Call only for the default catalog. Replace named prose fields, not feature settings,
/// tool contracts, permission policies, or explicit empty-string suppression.
pub fn apply_default_catalog_workflow(messages: &mut ModelMessages) {
    let bundled = ResolvedModelMessages::bundled();
    if let Some(approvals) = messages.approvals.as_mut() {
        replace_nonempty(
            &mut approvals.on_request_auto_review,
            DEFAULT_CATALOG_ON_REQUEST_AUTO_REVIEW,
        );
    }
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
