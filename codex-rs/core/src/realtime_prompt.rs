use crate::communication_preferences::append_communication_preferences;
use crate::communication_preferences::read_communication_preferences;
use codex_prompts::BACKEND_PROMPT;
use codex_protocol::error::Result as CodexResult;
use codex_utils_absolute_path::AbsolutePathBuf;
const DEFAULT_USER_FIRST_NAME: &str = "there";
const USER_FIRST_NAME_PLACEHOLDER: &str = "{{ user_first_name }}";

/// Codex owns the base; user communication preferences append without replacing it.
/// Reread preferences for each call, independent of the normal-text config snapshot.
pub(crate) async fn load_realtime_backend_prompt(
    prompt: Option<Option<String>>,
    config_prompt: Option<String>,
    personality_file: &AbsolutePathBuf,
    personality_file_required: bool,
) -> CodexResult<String> {
    let base = prepare_realtime_backend_prompt(prompt, config_prompt);
    let preferences =
        read_communication_preferences(personality_file, personality_file_required).await?;
    Ok(append_communication_preferences(
        &base,
        preferences.as_deref(),
    ))
}

pub(crate) fn prepare_realtime_backend_prompt(
    prompt: Option<Option<String>>,
    config_prompt: Option<String>,
) -> String {
    if let Some(config_prompt) = config_prompt
        && !config_prompt.trim().is_empty()
    {
        return config_prompt;
    }

    match prompt {
        Some(Some(prompt)) => return prompt,
        Some(None) => return String::new(),
        None => {}
    }

    BACKEND_PROMPT
        .trim_end()
        .replace(USER_FIRST_NAME_PLACEHOLDER, &current_user_first_name())
}

fn current_user_first_name() -> String {
    [whoami::realname(), whoami::username()]
        .into_iter()
        .filter_map(|name| name.split_whitespace().next().map(str::to_string))
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_FIRST_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::load_realtime_backend_prompt;
    use super::prepare_realtime_backend_prompt;
    use codex_utils_absolute_path::AbsolutePathBuf;

    #[tokio::test]
    async fn preferences_append_to_native_or_overridden_base_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::try_from(dir.path().join("voice.md")).unwrap();
        for contents in [
            "My preferred communication style",
            "Updated preferred style",
            "",
        ] {
            tokio::fs::write(path.as_path(), contents).await.unwrap();
            for (request, inline, expected_base) in [
                (
                    Some(Some("request override".into())),
                    None,
                    "request override",
                ),
                (
                    Some(Some("request override".into())),
                    Some("inline override".into()),
                    "inline override",
                ),
                (Some(None), None, ""),
            ] {
                let prompt = load_realtime_backend_prompt(request, inline, &path, true)
                    .await
                    .unwrap();
                assert!(prompt.starts_with(expected_base));
                if contents.is_empty() {
                    assert_eq!(prompt, expected_base);
                } else {
                    assert!(prompt.contains(contents));
                    assert!(prompt.contains("user's preferred communication styles"));
                }
            }
        }
    }

    #[tokio::test]
    async fn configured_file_errors_do_not_fall_back_to_request_or_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::try_from(dir.path().join("voice.md")).unwrap();
        let error = load_realtime_backend_prompt(None, None, &path, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Cannot read personality file"));
        for contents in [vec![0xff], vec![b'x'; 64 * 1024 + 1]] {
            tokio::fs::write(path.as_path(), contents).await.unwrap();
            assert!(
                load_realtime_backend_prompt(None, None, &path, true)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn prepare_realtime_backend_prompt_prefers_config_override() {
        assert_eq!(
            prepare_realtime_backend_prompt(
                Some(Some("prompt from request".to_string())),
                Some("prompt from config".to_string()),
            ),
            "prompt from config"
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_uses_request_prompt() {
        assert_eq!(
            prepare_realtime_backend_prompt(
                Some(Some("prompt from request".to_string())),
                /*config_prompt*/ None,
            ),
            "prompt from request"
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_preserves_empty_request_prompt() {
        assert_eq!(
            prepare_realtime_backend_prompt(Some(Some(String::new())), /*config_prompt*/ None),
            ""
        );
        assert_eq!(
            prepare_realtime_backend_prompt(Some(None), /*config_prompt*/ None),
            ""
        );
    }

    #[test]
    fn prepare_realtime_backend_prompt_renders_default() {
        let prompt =
            prepare_realtime_backend_prompt(/*prompt*/ None, /*config_prompt*/ None);

        assert!(prompt.starts_with("## Identity, tone, and role"));
        assert!(prompt.contains("You are Codex, an OpenAI general-purpose agentic assistant"));
        assert!(prompt.contains("The user's name is "));
        assert!(!prompt.contains("{{ user_first_name }}"));
    }
}
