use super::*;
use codex_protocol::openai_models::ApprovalMessages;
use codex_protocol::openai_models::AutoReviewMessages;
use codex_protocol::openai_models::CollaborationModeMessages;
use codex_protocol::openai_models::ConfirmationPolicies;
use codex_protocol::openai_models::GuardianV2ModelConfig;
use codex_protocol::openai_models::ModelTokenBudgetConfig;
use codex_protocol::openai_models::MultiAgentMessages;
use codex_protocol::openai_models::MultiAgentModeMessages;
use codex_protocol::openai_models::MultiAgentRoleMessages;

#[test]
fn default_workflow_preserves_absence_suppression_and_custom_mode_hints() {
    for text in [None, Some(String::new()), Some(" \n".to_string())] {
        let mut messages = ModelMessages {
            approvals: Some(ApprovalMessages {
                on_request: text.clone(),
                on_request_auto_review: text.clone(),
                never: text.clone(),
                unless_trusted: text.clone(),
            }),
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
            token_budget: text.as_ref().map(|text| ModelTokenBudgetConfig {
                enabled: true,
                use_history_notes_extension: true,
                reminder_threshold_tokens: 13_579,
                reminder_message_template: text.clone(),
                guidance_message: text.clone(),
                auto_compact_fallback_prompt: text.clone(),
                auto_compact_fallback_buffer_tokens: 321,
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

#[test]
fn default_workflow_replaces_budget_prose_without_changing_runtime_settings() {
    for enabled in [false, true] {
        let mut messages = ModelMessages {
            token_budget: Some(ModelTokenBudgetConfig {
                enabled,
                use_history_notes_extension: !enabled,
                reminder_threshold_tokens: 13_579,
                reminder_message_template: "Catalog reminder {n_remaining}.".to_string(),
                guidance_message: "Catalog checkpoint guidance.".to_string(),
                auto_compact_fallback_prompt: "Catalog fallback.".to_string(),
                auto_compact_fallback_buffer_tokens: 321,
            }),
            ..Default::default()
        };
        let mut expected = messages.clone();
        let budget = expected.token_budget.as_mut().unwrap();
        budget.guidance_message = TOKEN_BUDGET_GUIDANCE.to_string();
        budget.reminder_message_template = TOKEN_BUDGET_REMINDER.to_string();
        budget.auto_compact_fallback_prompt = TOKEN_BUDGET_FALLBACK.to_string();

        apply_default_catalog_workflow(&mut messages);
        assert_eq!(messages, expected);
        apply_default_catalog_workflow(&mut messages);
        assert_eq!(messages, expected);
    }
}

#[test]
fn default_workflow_shortens_auto_review_prose_without_replacing_policy_or_other_approvals() {
    let mut messages = ModelMessages {
        approvals: Some(ApprovalMessages {
            on_request: Some("User approvals.".to_string()),
            on_request_auto_review: Some("Default catalog auto-review prose.".to_string()),
            never: Some(String::new()),
            unless_trusted: Some("Trusted approvals.".to_string()),
        }),
        auto_review: Some(AutoReviewMessages {
            policy: Some("Security policy.".to_string()),
            policy_template: Some("Policy template.".to_string()),
            node_repl_policy: Some("REPL policy.".to_string()),
            rejection_instructions: Some("Rejection restrictions.".to_string()),
            timeout_instructions: Some("Timeout guidance.".to_string()),
        }),
        guardian_v2: Some(GuardianV2ModelConfig {
            classifier_instructions: Some("Classifier policy.".to_string()),
            ..Default::default()
        }),
        confirmation_policies: Some(ConfirmationPolicies {
            browser_use: Some("Browser confirmation policy.".to_string()),
            computer_use: Some("Computer confirmation policy.".to_string()),
        }),
        ..Default::default()
    };
    let mut expected = messages.clone();
    expected.approvals.as_mut().unwrap().on_request_auto_review =
        Some(DEFAULT_CATALOG_ON_REQUEST_AUTO_REVIEW.to_string());

    apply_default_catalog_workflow(&mut messages);
    assert_eq!(messages, expected);
    // Normalization must retain the catalog's recovery choices, not bundled restrictions.
    let prose = messages
        .approvals
        .as_ref()
        .unwrap()
        .on_request_auto_review
        .as_deref()
        .unwrap();
    assert!(prose.contains("evidence establishing authorization or low risk before retrying"));
    assert!(prose.contains("Unaffected work without confirmation"));
    assert!(prose.contains("explain the auto-review rejection and risk, and ask for approval"));

    apply_default_catalog_workflow(&mut messages);
    assert_eq!(messages, expected);
}
