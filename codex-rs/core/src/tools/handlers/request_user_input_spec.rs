use codex_protocol::config_types::ModeKind;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const REQUEST_USER_INPUT_TOOL_NAME: &str = "request_user_input";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct RequestUserInputToolArgs {
    pub questions: Vec<RequestUserInputQuestion>,
}

pub fn create_request_user_input_tool(description: String) -> ToolSpec {
    let option_props = BTreeMap::from([
        (
            "label".to_string(),
            JsonSchema::string(Some("1-5 words".to_string())),
        ),
        (
            "description".to_string(),
            JsonSchema::string(Some("Choice impact, one sentence".to_string())),
        ),
    ]);

    let options_schema = JsonSchema::array(
        JsonSchema::object(
            option_props,
            Some(vec!["label".to_string(), "description".to_string()]),
            Some(false.into()),
        ),
        Some("2-3 mutually exclusive choices; no Other, automatic free text".to_string()),
    );

    let question_props = BTreeMap::from([
        (
            "id".to_string(),
            JsonSchema::string(Some("Stable snake_case answer key".to_string())),
        ),
        (
            "header".to_string(),
            JsonSchema::string(Some("UI header, at most 12 characters".to_string())),
        ),
        (
            "question".to_string(),
            JsonSchema::string(Some("One sentence".to_string())),
        ),
        ("options".to_string(), options_schema),
    ]);

    let questions_schema = JsonSchema::array(
        JsonSchema::object(
            question_props,
            Some(vec![
                "id".to_string(),
                "header".to_string(),
                "question".to_string(),
                "options".to_string(),
            ]),
            Some(false.into()),
        ),
        Some("At most 3".to_string()),
    );

    let properties = BTreeMap::from([("questions".to_string(), questions_schema)]);

    ToolSpec::Function(ResponsesApiTool {
        name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
        description,
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["questions".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn request_user_input_unavailable_message(
    mode: ModeKind,
    available_modes: &[ModeKind],
) -> Option<String> {
    if available_modes.contains(&mode) {
        None
    } else {
        let mode_name = mode.display_name();
        Some(format!(
            "request_user_input is unavailable in {mode_name} mode"
        ))
    }
}

pub(crate) fn normalize_request_user_input_tool_args(
    mut args: RequestUserInputToolArgs,
) -> Result<RequestUserInputToolArgs, String> {
    let missing_options = args
        .questions
        .iter()
        .any(|question| question.options.as_ref().is_none_or(Vec::is_empty));
    if missing_options {
        return Err("request_user_input requires non-empty options for every question".to_string());
    }

    for question in &mut args.questions {
        question.is_other = true;
    }

    Ok(args)
}

pub fn request_user_input_tool_description(available_modes: &[ModeKind]) -> String {
    let allowed_modes = format_allowed_modes(available_modes);
    format!("Ask the user; wait for answers; {allowed_modes} only")
}

fn format_allowed_modes(available_modes: &[ModeKind]) -> String {
    let mode_names: Vec<&str> = available_modes
        .iter()
        .map(|mode| mode.display_name())
        .collect();

    match mode_names.as_slice() {
        [] => "no modes".to_string(),
        [mode] => format!("{mode} mode"),
        [first, second] => format!("{first} or {second} mode"),
        [..] => format!("modes: {}", mode_names.join(",")),
    }
}

#[cfg(test)]
#[path = "request_user_input_spec_tests.rs"]
mod tests;
