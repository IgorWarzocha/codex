use codex_protocol::models::VIEW_IMAGE_TOOL_NAME;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewImageToolOptions {
    pub can_request_original_image_detail: bool,
    pub unified_image_budget: bool,
    pub include_environment_id: bool,
}

pub fn create_view_image_tool(options: ViewImageToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([("path".to_string(), JsonSchema::string(None))]);
    if options.can_request_original_image_detail && !options.unified_image_budget {
        properties.insert(
            "detail".to_string(),
            JsonSchema::string_enum(
                vec![json!("high"), json!("original")],
                Some("Default high; original preserves exact resolution".to_string()),
            ),
        );
    }
    if options.include_environment_id {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "ID from <environment_context>, defaults to primary environment".to_string(),
            )),
        );
    }

    ToolSpec::Function(ResponsesApiTool {
        name: VIEW_IMAGE_TOOL_NAME.to_string(),
        description: "Inspect a local image".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(view_image_output_schema(options).into()),
    })
}

fn view_image_output_schema(options: ViewImageToolOptions) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "image_url": {
                "type": "string",
                "description": "Image data URL"
            }
        },
        "required": ["image_url"],
        "additionalProperties": false
    });
    if !options.unified_image_budget {
        schema["properties"]["detail"] = json!({
            "type": "string",
            "enum": ["high", "original"],
            "description": "high is resized; original preserves resolution"
        });
        schema["required"] = json!(["image_url", "detail"]);
    }
    schema
}
