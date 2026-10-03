//! Covers role composition and attribution for captured model instructions.

use super::*;
use codex_context_fragments::AnnotatedContent;
use codex_context_fragments::RenderedFragment;
use pretty_assertions::assert_eq;

#[test]
fn role_segment_filters_base_and_appends_bundled_guidance() {
    let shared = DEFAULT_MULTI_AGENT_V2_SHARED_USAGE_HINT_TEXT;
    let wait = DEFAULT_MULTI_AGENT_V2_WAIT_AGENT_USAGE_HINT_TEXT;
    let model_override = DEFAULT_MULTI_AGENT_V2_MODEL_OVERRIDE_USAGE_HINT_TEXT;
    let expected_body = format!(
        "Role.\n## Work\nContinue.\n{shared}\n{wait}\n\nActive-agent limit, including you: 2\n\n{model_override}"
    );
    for marked in [false, true] {
        let instructions = MultiAgentRoleInstructions::Composed {
            base: "Role.\n## Plan tool\nOmit role checklist guidance.\n## Work\nContinue."
                .to_string(),
            marked,
            omit_update_plan_instructions: true,
            max_concurrency: 2,
            wait_agent_enabled: true,
            expose_model_overrides: true,
            agent_message_board: None,
        };
        let expected_text = if marked {
            format!("<multi_agent_role>{expected_body}</multi_agent_role>")
        } else {
            expected_body.clone()
        };
        assert_eq!(
            instructions.render_fragment(),
            RenderedFragment::new(
                "developer",
                AnnotatedContent::input_text(
                    expected_text,
                    ContentItemKind("multi_agent.role_instructions".to_string()),
                ),
            ),
        );
    }
}

#[test]
fn board_guidance_extends_composed_roles_only_when_available() {
    let role = MultiAgentRoleInstructions::Composed {
        base: "Role.".to_string(),
        marked: false,
        omit_update_plan_instructions: false,
        max_concurrency: 3,
        wait_agent_enabled: false,
        expose_model_overrides: false,
        agent_message_board: None,
    };
    let disabled = role.body();
    let guidance = Some(AgentMessageBoardGuidance::WithDirectMessaging);
    let enabled = role.clone().with_agent_message_board(guidance).body();
    assert_eq!(
        enabled,
        format!(
            "{disabled}\n\n{AGENT_MESSAGE_BOARD_USAGE_HINT_TEXT} {DIRECT_AGENT_COORDINATION_USAGE_HINT_TEXT}"
        )
    );
    assert_eq!(
        enabled.len() - disabled.len(),
        AGENT_MESSAGE_BOARD_USAGE_HINT_TEXT.len()
            + DIRECT_AGENT_COORDINATION_USAGE_HINT_TEXT.len()
            + 3
    );
    assert_eq!(
        role.clone()
            .with_agent_message_board(guidance)
            .with_agent_message_board(None)
            .body(),
        disabled
    );
    let configured = MultiAgentRoleInstructions::Configured("Custom role.".to_string());
    assert_eq!(
        configured.clone().with_agent_message_board(guidance),
        configured
    );
    let board_only = role
        .with_agent_message_board(Some(AgentMessageBoardGuidance::BoardOnly))
        .body();
    assert_eq!(
        board_only,
        format!("{disabled}\n\n{AGENT_MESSAGE_BOARD_USAGE_HINT_TEXT}")
    );
}
