use super::session::Session;
use super::turn_context::TurnContext;
use crate::config::Config;
use crate::config::ContextStrategy;
use crate::config::TokenBudgetConfig;
use crate::config::resolve_token_budget_config;
use crate::context::ContextualUserFragment;
use codex_features::Feature;
use codex_login::CodexAuth;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;

/// Validate storage before enabling resets. Legacy flags and model experiments
/// cannot select continuity or silently downgrade notes to compaction.
pub(super) fn apply_context_strategy(
    config: &mut Config,
    auth: Option<&CodexAuth>,
) -> std::io::Result<()> {
    let notes = config.context_strategy == ContextStrategy::Notes;
    if notes && !config.notes_backend_is_available(auth) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "context_strategy = 'notes' requires remote notes storage through an OpenAI Codex backend provider and Codex backend authentication. Sign in to a supported backend or explicitly set context_strategy = 'compaction'.",
        ));
    }

    // Settings persistence shares this policy check without requiring session auth.
    let features = config
        .features
        .with_context_strategy(config.context_strategy)?;

    let mut token_budget = config.token_budget.clone();
    if notes && token_budget.is_none() {
        let config_toml = config
            .config_layer_stack
            .effective_config()
            .try_into()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        token_budget = resolve_token_budget_config(&config_toml)?;
    }
    if notes {
        token_budget
            .get_or_insert_default()
            .use_history_notes_extension = true;
    } else {
        token_budget = None;
    }

    config.features = features;
    config.token_budget = token_budget;
    Ok(())
}

/// Detects explicit preferences before model defaults are applied to the turn config.
pub(super) fn has_explicit_settings(config: &Config) -> bool {
    config
        .config_layer_stack
        .effective_config()
        .get("features")
        .and_then(|features| features.get("token_budget"))
        .and_then(|token_budget| token_budget.as_table())
        .is_some_and(|settings| {
            settings
                .keys()
                .any(|key| !matches!(key.as_str(), "enabled" | "use_history_notes_extension"))
        })
        || config.token_budget.as_ref().is_some_and(|token_budget| {
            let mut settings = token_budget.clone();
            settings.use_history_notes_extension = false;
            settings != TokenBudgetConfig::default()
        })
}

/// Resolves user-configured token-budget preferences against the current model's defaults.
pub(super) fn resolve_token_budget(
    configured_token_budget: Option<&TokenBudgetConfig>,
    use_model_defaults: bool,
    model_info: &ModelInfo,
) -> Option<TokenBudgetConfig> {
    if !use_model_defaults {
        return configured_token_budget.cloned();
    }

    let Some(model_defaults) = model_info
        .model_messages
        .as_ref()
        .and_then(|messages| messages.token_budget.as_ref())
    else {
        return configured_token_budget.cloned();
    };

    let token_budget = TokenBudgetConfig {
        use_history_notes_extension: configured_token_budget
            .is_some_and(|token_budget| token_budget.use_history_notes_extension),
        reminder_threshold_tokens: Some(model_defaults.reminder_threshold_tokens),
        reminder_message_template: model_defaults.reminder_message_template.clone(),
        guidance_message: Some(model_defaults.guidance_message.clone()),
        auto_compact_fallback_prompt: Some(model_defaults.auto_compact_fallback_prompt.clone()),
        auto_compact_fallback_buffer_tokens: Some(
            model_defaults.auto_compact_fallback_buffer_tokens,
        ),
    };

    if let Err(error) = token_budget.validate() {
        tracing::warn!(
            model = %model_info.slug,
            %error,
            "ignoring invalid model-owned token-budget defaults"
        );
        return configured_token_budget.cloned();
    }

    Some(token_budget)
}

pub(super) async fn maybe_record(
    sess: &Session,
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    token_status: &super::context_window::ContextWindowTokenStatus,
    allow_auto_compact_fallback: bool,
) -> Vec<ResponseItem> {
    if !turn_context.config.features.enabled(Feature::TokenBudget) {
        return Vec::new();
    }
    let Some(config) = resolve_token_budget(
        turn_context.configured_token_budget.as_ref(),
        turn_context.use_model_token_budget_defaults,
        model_info,
    ) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    if token_status.notes_checkpoint_due {
        let checkpoint_due = sess.state.lock().await.claim_notes_checkpoint_reminder();
        if checkpoint_due {
            items.push(ContextualUserFragment::into(
                crate::context::TokenBudgetReminder::checkpoint_now(),
            ));
        }
    } else if let Some(base_window_tokens_remaining) = token_status.base_window_tokens_remaining
        && config
            .reminder_threshold_tokens
            .is_some_and(|threshold| base_window_tokens_remaining <= threshold)
    {
        let reminder_due = {
            let mut state = sess.state.lock().await;
            state.claim_token_budget_reminder()
        };
        if reminder_due {
            items.push(ContextualUserFragment::into(
                crate::context::TokenBudgetReminder::new(
                    &config.reminder_message_template,
                    base_window_tokens_remaining,
                ),
            ));
        }
    }

    // The urgent instruction supersedes softer prompts at this boundary.
    if !token_status.notes_checkpoint_due
        && allow_auto_compact_fallback
        && token_status.base_window_tokens_remaining == Some(0)
        && let Some(prompt) = config.auto_compact_fallback_prompt.as_deref()
        && sess.state.lock().await.claim_auto_compact_fallback()
    {
        items.push(ContextualUserFragment::into(
            crate::context::AutoCompactFallbackPrompt::new(prompt),
        ));
    }
    if !items.is_empty() {
        sess.record_conversation_items(turn_context, model_info, &items)
            .await;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;
    use crate::config::ManagedFeatures;
    use codex_config::FeatureRequirementsToml;
    use codex_config::RequirementSource;
    use codex_config::Sourced;
    use codex_models_manager::test_support::construct_model_info_offline_for_tests;
    use codex_protocol::openai_models::ModelTokenBudgetConfig;

    async fn config() -> Config {
        let home = tempfile::tempdir().unwrap();
        ConfigBuilder::without_managed_config_for_tests()
            .codex_home(home.path().to_path_buf())
            .build()
            .await
            .unwrap()
    }

    fn model(config: &Config) -> ModelInfo {
        let mut model = construct_model_info_offline_for_tests(
            "gpt-6-luna",
            &config.to_models_manager_config(),
        );
        model.supports_experimental_context = false;
        model
    }

    #[tokio::test]
    async fn notes_requires_usable_storage_without_partial_activation() {
        let mut config = config().await;
        let initial_features = config.features.clone();
        let initial_budget = config.token_budget.clone();
        let error = apply_context_strategy(&mut config, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("context_strategy = 'compaction'")
        );
        assert_eq!(config.features, initial_features);
        assert_eq!(config.token_budget, initial_budget);

        let manager = codex_login::test_support::auth_manager_with_agent_identity()
            .await
            .unwrap();
        let auth = manager.auth_cached();
        config.model_provider.env_key = Some("CUSTOM_API_KEY".to_string());
        assert!(apply_context_strategy(&mut config, auth.as_ref()).is_err());
        assert_eq!(config.features, initial_features);
    }

    #[tokio::test]
    async fn strategy_overrides_legacy_activation() {
        let mut config = config().await;
        let manager = codex_login::test_support::auth_manager_with_agent_identity()
            .await
            .unwrap();
        let auth = manager.auth_cached();
        config.features.disable(Feature::ContextManagement).unwrap();
        config.features.disable(Feature::TokenBudget).unwrap();
        config
            .token_budget
            .as_mut()
            .unwrap()
            .use_history_notes_extension = false;
        apply_context_strategy(&mut config, auth.as_ref()).unwrap();
        assert!(config.features.enabled(Feature::ContextManagement));
        assert!(config.features.enabled(Feature::TokenBudget));
        assert!(
            config
                .token_budget
                .as_ref()
                .unwrap()
                .use_history_notes_extension
        );

        // Post-load overrides must not be replaced by the default in raw TOML.
        config.context_strategy = ContextStrategy::Compaction;
        apply_context_strategy(&mut config, None).unwrap();
        assert!(!config.features.enabled(Feature::ContextManagement));
        assert!(!config.features.enabled(Feature::TokenBudget));
        assert!(config.token_budget.is_none());
    }

    #[tokio::test]
    async fn strategy_reports_managed_feature_conflicts() {
        let manager = codex_login::test_support::auth_manager_with_agent_identity()
            .await
            .unwrap();
        let auth = manager.auth_cached();
        for feature in [Feature::TokenBudget, Feature::ContextManagement] {
            for (strategy, pinned) in [
                (ContextStrategy::Notes, false),
                (ContextStrategy::Compaction, true),
            ] {
                let mut config = config().await;
                config.context_strategy = strategy;
                config.features = ManagedFeatures::from_configured(
                    config.features.get().clone(),
                    Some(Sourced {
                        value: FeatureRequirementsToml {
                            entries: [(feature.key().to_string(), pinned)].into(),
                        },
                        source: RequirementSource::Unknown,
                    }),
                )
                .unwrap();
                let features = config.features.clone();
                let error = apply_context_strategy(&mut config, auth.as_ref()).unwrap_err();
                assert!(error.to_string().contains("managed requirement"));
                assert!(error.to_string().contains(feature.key()));
                assert_eq!(config.features, features);
            }
        }
    }

    #[tokio::test]
    async fn fresh_child_restores_preferences_and_revalidates_backend() {
        let mut parent = config().await;
        let manager = codex_login::test_support::auth_manager_with_agent_identity()
            .await
            .unwrap();
        let auth = manager.auth_cached();
        parent.prepare_token_budget_for_startup().unwrap();
        apply_context_strategy(&mut parent, auth.as_ref()).unwrap();
        parent.token_budget.as_mut().unwrap().guidance_message =
            Some("Resolved parent guidance".to_string());

        let mut child = parent.clone();
        child.prepare_token_budget_for_startup().unwrap();
        assert!(
            child
                .token_budget
                .as_ref()
                .unwrap()
                .guidance_message
                .is_none()
        );
        apply_context_strategy(&mut child, auth.as_ref()).unwrap();
        assert!(child.features.enabled(Feature::TokenBudget));
        assert!(apply_context_strategy(&mut child, None).is_err());

        let mut compaction_child = parent;
        compaction_child.context_strategy = ContextStrategy::Compaction;
        compaction_child.prepare_token_budget_for_startup().unwrap();
        apply_context_strategy(&mut compaction_child, None).unwrap();
        assert!(compaction_child.token_budget.is_none());
        assert!(!compaction_child.features.enabled(Feature::TokenBudget));
    }

    #[tokio::test]
    async fn notes_keeps_model_owned_reminder_and_fallback_reserve() {
        let config = config().await;
        assert!(!has_explicit_settings(&config));
        let mut model = model(&config);
        model.model_messages.get_or_insert_default().token_budget = Some(ModelTokenBudgetConfig {
            enabled: false,
            use_history_notes_extension: false,
            reminder_threshold_tokens: 16_000,
            reminder_message_template: "Remaining: {n_remaining}".to_string(),
            guidance_message: "Preserve state.".to_string(),
            auto_compact_fallback_prompt: "Checkpoint notes.".to_string(),
            auto_compact_fallback_buffer_tokens: 8_000,
        });
        let resolved = resolve_token_budget(config.token_budget.as_ref(), true, &model).unwrap();
        assert!(resolved.use_history_notes_extension);
        assert_eq!(resolved.reminder_threshold_tokens, Some(16_000));
        assert_eq!(resolved.fallback_buffer_tokens(), 8_000);
        assert_eq!(
            resolved.auto_compact_fallback_prompt.as_deref(),
            Some("Checkpoint notes.")
        );
    }
}
