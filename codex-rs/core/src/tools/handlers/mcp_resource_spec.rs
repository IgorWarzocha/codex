use codex_protocol::openai_models::ToolMessage;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub fn create_list_mcp_resources_tool(messages: Option<&ToolMessage>) -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "server".to_string(),
            JsonSchema::string(Some(
                "Server name, defaults to all configured servers".to_string(),
            )),
        ),
        (
            "cursor".to_string(),
            JsonSchema::string(Some("Cursor from the previous resource page".to_string())),
        ),
    ]);

    let tool = ResponsesApiTool {
        name: "list_mcp_resources".to_string(),
        description: "List MCP context resources. Prefer available resources over web search"
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    };
    with_model_messages(tool, messages)
}

pub fn create_list_mcp_resource_templates_tool(messages: Option<&ToolMessage>) -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "server".to_string(),
            JsonSchema::string(Some(
                "Server name, defaults to all configured servers".to_string(),
            )),
        ),
        (
            "cursor".to_string(),
            JsonSchema::string(Some("Cursor from the previous template page".to_string())),
        ),
    ]);

    let tool = ResponsesApiTool {
        name: "list_mcp_resource_templates".to_string(),
        description:
            "List parameterized MCP context resources. Prefer available templates over web search"
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    };
    with_model_messages(tool, messages)
}

pub fn create_read_mcp_resource_tool(messages: Option<&ToolMessage>) -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "server".to_string(),
            JsonSchema::string(Some(
                "Exact server value from list_mcp_resources".to_string(),
            )),
        ),
        (
            "uri".to_string(),
            JsonSchema::string(Some("URI returned by list_mcp_resources".to_string())),
        ),
    ]);

    let tool = ResponsesApiTool {
        name: "read_mcp_resource".to_string(),
        description: "Read an MCP context resource".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["server".to_string(), "uri".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    };
    with_model_messages(tool, messages)
}

fn with_model_messages(mut tool: ResponsesApiTool, messages: Option<&ToolMessage>) -> ToolSpec {
    if let Some(messages) = messages {
        if let Some(description) = &messages.description {
            tool.description.clone_from(description);
        }
        if let Some(parameters) = &messages.parameters {
            match crate::tools::catalog_parameters::parse(parameters) {
                Ok(parameters) => tool.parameters = parameters,
                Err(reason) => tracing::warn!(
                    tool = %tool.name,
                    reason,
                    "Invalid catalog tool parameters; using bundled parameters"
                ),
            }
        }
    }
    ToolSpec::Function(tool)
}

#[cfg(test)]
#[path = "mcp_resource_spec_tests.rs"]
mod tests;
