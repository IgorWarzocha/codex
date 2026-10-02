//! Responses API tool definitions for persisted thread goals.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const GET_GOAL_TOOL_NAME: &str = "get_goal";
pub const CREATE_GOAL_TOOL_NAME: &str = "create_goal";
pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

pub fn create_get_goal_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: GET_GOAL_TOOL_NAME.to_string(),
        description: "Thread goal, status, budgets, usage, remaining tokens".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}

pub fn create_create_goal_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        ("objective".to_string(), JsonSchema::string(None)),
        (
            "token_budget".to_string(),
            JsonSchema::integer(Some(
                "Positive; omit unless explicitly requested".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: CREATE_GOAL_TOOL_NAME.to_string(),
        description: format!(
            r#"Explicit user or system/developer request only; no inferred goals from ordinary tasks
New active goal or replacement for a completed goal; fails while unfinished
Status changes via {UPDATE_GOAL_TOOL_NAME}"#
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["objective".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_update_goal_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "status".to_string(),
        JsonSchema::string_enum(
            vec![json!("complete"), json!("blocked"), json!("paused")],
            None,
        ),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: UPDATE_GOAL_TOOL_NAME.to_string(),
        description: r#"Existing goal status only; resume, budget-limit and usage-limit controlled by user/system
paused: explicit user request only; ask if unclear; resume revokes permission; report returned status and stop goal work; budget limits take precedence
complete: objective achieved, no required work remaining; stopping or low budget insufficient; report final token usage for budgeted goals
blocked: same blocker for at least three consecutive goal turns, including the original/user turn and automatic continuations; no meaningful progress without user input or external-state change
Fresh blocked audit after resume; mark blocked once threshold met, not merely for difficulty, slowness, uncertainty, incompleteness or useful clarification"#
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["status".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
