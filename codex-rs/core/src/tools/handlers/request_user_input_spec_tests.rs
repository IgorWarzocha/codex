use super::*;
use codex_features::Feature;
use codex_features::Features;
use codex_protocol::config_types::ModeKind;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputQuestionOption;
use codex_tools::JsonSchema;
use codex_tools::request_user_input_available_modes;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

fn default_mode_enabled_available_modes() -> Vec<ModeKind> {
    let mut features = Features::with_defaults();
    features.enable(Feature::DefaultModeRequestUserInput);
    request_user_input_available_modes(&features)
}

fn default_available_modes() -> Vec<ModeKind> {
    request_user_input_available_modes(&Features::with_defaults())
}

#[test]
fn request_user_input_tool_includes_questions_schema() {
    assert_eq!(
        create_request_user_input_tool("Ask the user to choose.".to_string()),
        ToolSpec::Function(ResponsesApiTool {
            name: "request_user_input".to_string(),
            description: "Ask the user to choose.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::from([
                (
                    "questions".to_string(),
                    JsonSchema::array(
                        JsonSchema::object(
                            BTreeMap::from([
                                (
                                    "header".to_string(),
                                    JsonSchema::string(Some(
                                        "UI header, at most 12 characters"
                                            .to_string(),
                                    )),
                                ),
                                (
                                    "id".to_string(),
                                    JsonSchema::string(Some(
                                        "Stable snake_case answer key"
                                            .to_string(),
                                    )),
                                ),
                                (
                                    "options".to_string(),
                                    JsonSchema::array(
                                        JsonSchema::object(
                                            BTreeMap::from([
                                                (
                                                    "description".to_string(),
                                                    JsonSchema::string(Some(
                                                        "One sentence describing the choice's impact"
                                                            .to_string(),
                                                    )),
                                                ),
                                                (
                                                    "label".to_string(),
                                                    JsonSchema::string(Some(
                                                        "Label, 1-5 words"
                                                            .to_string(),
                                                    )),
                                                ),
                                            ]),
                                            Some(vec![
                                                "label".to_string(),
                                                "description".to_string(),
                                            ]),
                                            Some(false.into()),
                                        ),
                                        Some(
                                            "2-3 mutually exclusive choices. Omit Other because the client adds free text"
                                                .to_string(),
                                        ),
                                    ),
                                ),
                                (
                                    "question".to_string(),
                                    JsonSchema::string(Some("Single-sentence question".to_string())),
                                ),
                            ]),
                            Some(vec![
                                "id".to_string(),
                                "header".to_string(),
                                "question".to_string(),
                                "options".to_string(),
                            ]),
                            Some(false.into()),
                        ),
                        Some(
                            "At most 3".to_string(),
                        ),
                    ),
                ),
            ]),
            Some(vec!["questions".to_string()]),
            Some(false.into())),
            output_schema: None,
        })
    );
}

#[test]
fn normalize_request_user_input_tool_args_sets_other_on_every_question() {
    let args = RequestUserInputToolArgs {
        questions: vec![RequestUserInputQuestion {
            id: "confirm".to_string(),
            header: "Confirm".to_string(),
            question: "Proceed?".to_string(),
            is_other: false,
            is_secret: false,
            options: Some(vec![RequestUserInputQuestionOption {
                label: "Yes".to_string(),
                description: "Continue.".to_string(),
            }]),
        }],
    };

    assert_eq!(
        normalize_request_user_input_tool_args(args.clone()),
        Ok(RequestUserInputToolArgs {
            questions: vec![RequestUserInputQuestion {
                is_other: true,
                ..args.questions[0].clone()
            }],
        })
    );
}

#[test]
fn normalize_request_user_input_tool_args_rejects_missing_options() {
    let args = RequestUserInputToolArgs {
        questions: vec![RequestUserInputQuestion {
            id: "confirm".to_string(),
            header: "Confirm".to_string(),
            question: "Proceed?".to_string(),
            is_other: false,
            is_secret: false,
            options: None,
        }],
    };

    assert_eq!(
        normalize_request_user_input_tool_args(args),
        Err("request_user_input requires non-empty options for every question".to_string())
    );
}

#[test]
fn request_user_input_unavailable_messages_respect_default_mode_feature_flag() {
    assert_eq!(
        request_user_input_unavailable_message(ModeKind::Plan, &default_available_modes()),
        None
    );
    assert_eq!(
        request_user_input_unavailable_message(ModeKind::Default, &default_available_modes()),
        Some("request_user_input is unavailable in Default mode".to_string())
    );
    assert_eq!(
        request_user_input_unavailable_message(
            ModeKind::Default,
            &default_mode_enabled_available_modes()
        ),
        None
    );
}

#[test]
fn request_user_input_tool_description_mentions_available_modes() {
    assert_eq!(
        request_user_input_tool_description(&default_available_modes()),
        "Ask the user and wait for answers. Available only in Plan mode.".to_string()
    );
    assert_eq!(
        request_user_input_tool_description(&default_mode_enabled_available_modes()),
        "Ask the user and wait for answers. Available only in Default or Plan mode.".to_string()
    );
    assert_eq!(
        request_user_input_tool_description(&[ModeKind::Default]),
        "Ask the user and wait for answers. Available only in Default mode.".to_string()
    );
}
