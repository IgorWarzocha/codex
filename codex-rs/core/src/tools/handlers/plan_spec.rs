use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub fn create_update_plan_tool() -> ToolSpec {
    let plan_item_properties = BTreeMap::from([
        (
            "step".to_string(),
            JsonSchema::string(Some("Task step".to_string())),
        ),
        (
            "status".to_string(),
            JsonSchema::string_enum(
                vec![json!("pending"), json!("in_progress"), json!("completed")],
                Some("Status".to_string()),
            ),
        ),
    ]);

    let properties = BTreeMap::from([
        (
            "explanation".to_string(),
            JsonSchema::string(Some("Reason for the update".to_string())),
        ),
        (
            "plan".to_string(),
            JsonSchema::array(
                JsonSchema::object(
                    plan_item_properties,
                    Some(vec!["step".to_string(), "status".to_string()]),
                    Some(false.into()),
                ),
                Some("Plan steps".to_string()),
            ),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "update_plan".to_string(),
        description: r#"Update the task plan. At most one step may be in_progress."#.to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["plan".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}
