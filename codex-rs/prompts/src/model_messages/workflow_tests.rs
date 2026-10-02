use super::*;
use codex_protocol::openai_models::CollaborationModeMessages;
use codex_protocol::openai_models::MultiAgentMessages;
use codex_protocol::openai_models::MultiAgentModeMessages;
use codex_protocol::openai_models::MultiAgentRoleMessages;

#[test]
fn default_workflow_preserves_absence_suppression_and_custom_mode_hints() {
    for text in [None, Some(String::new()), Some(" \n".to_string())] {
        let mut messages = ModelMessages {
            persistent_instructions: text.clone(),
            collaboration_modes: Some(CollaborationModeMessages {
                default: text.clone(),
                plan: text.clone(),
            }),
            multi_agent: Some(MultiAgentMessages {
                role: Some(MultiAgentRoleMessages {
                    root: text.clone(),
                    subagent: text.clone(),
                }),
                mode: Some(MultiAgentModeMessages {
                    explicit: text.clone(),
                    proactive: text.clone(),
                    hint_text: Some("specific mode hint".to_string()),
                }),
            }),
            ..Default::default()
        };
        let original = messages.clone();
        apply_default_catalog_workflow(&mut messages);
        assert_eq!(messages, original);
    }
    let mut absent = ModelMessages::default();
    apply_default_catalog_workflow(&mut absent);
    assert_eq!(absent, ModelMessages::default());
}
