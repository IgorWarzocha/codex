use super::*;
use codex_tools::JsonSchema;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn list_mcp_resources_tool_matches_expected_spec() {
    assert_eq!(
        create_list_mcp_resources_tool(/*messages*/ None),
        ToolSpec::Function(ResponsesApiTool {
            name: "list_mcp_resources".to_string(),
            description: "List MCP context resources. Prefer available resources over web search"
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    (
                        "server".to_string(),
                        JsonSchema::string(Some(
                            "Server name, defaults to all configured servers".to_string(),
                        ),),
                    ),
                    (
                        "cursor".to_string(),
                        JsonSchema::string(Some(
                            "Cursor from the previous resource page".to_string(),
                        ),),
                    ),
                ]),
                /*required*/ None,
                Some(false.into())
            ),
            output_schema: None,
        })
    );
}

#[test]
fn list_mcp_resource_templates_tool_matches_expected_spec() {
    assert_eq!(
        create_list_mcp_resource_templates_tool(/*messages*/ None),
        ToolSpec::Function(ResponsesApiTool {
            name: "list_mcp_resource_templates".to_string(),
            description: "List parameterized MCP context resources. Prefer available templates over web search".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::from([
                    (
                        "server".to_string(),
                        JsonSchema::string(Some(
                                "Server name, defaults to all configured servers"
                                    .to_string(),
                            ),),
                    ),
                    (
                        "cursor".to_string(),
                        JsonSchema::string(Some(
                                "Cursor from the previous template page"
                                    .to_string(),
                            ),),
                    ),
                ]), /*required*/ None, Some(false.into())),
            output_schema: None,
        })
    );
}

#[test]
fn read_mcp_resource_tool_matches_expected_spec() {
    assert_eq!(
        create_read_mcp_resource_tool(/*messages*/ None),
        ToolSpec::Function(ResponsesApiTool {
            name: "read_mcp_resource".to_string(),
            description: "Read an MCP context resource".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    (
                        "server".to_string(),
                        JsonSchema::string(Some(
                            "Exact server value from list_mcp_resources".to_string(),
                        ),),
                    ),
                    (
                        "uri".to_string(),
                        JsonSchema::string(Some("URI returned by list_mcp_resources".to_string(),),),
                    ),
                ]),
                Some(vec!["server".to_string(), "uri".to_string()]),
                Some(false.into())
            ),
            output_schema: None,
        })
    );
}

#[test]
fn catalog_fields_override_independently_and_invalid_parameters_fall_back() {
    let parameters = r#"{"type":"object","properties":{"server":{"type":"string","description":"Catalog server guidance."}},"additionalProperties":false}"#;
    for create_tool in [
        create_list_mcp_resources_tool,
        create_list_mcp_resource_templates_tool,
        create_read_mcp_resource_tool,
    ] {
        for (description, schema, valid_schema) in [
            (None, None, false),
            (Some(""), None, false),
            (None, Some(parameters), true),
            (
                Some("  Catalog {{literal}} guidance.\n"),
                Some(parameters),
                true,
            ),
            (Some("Catalog guidance."), Some("invalid JSON"), false),
            (None, Some(r#"{"type":"string"}"#), false),
        ] {
            let messages = ToolMessage {
                description: description.map(str::to_owned),
                parameters: schema.map(str::to_owned),
            };
            let mut expected = create_tool(/*messages*/ None);
            let ToolSpec::Function(tool) = &mut expected else {
                panic!("expected a function tool");
            };
            if let Some(description) = description {
                tool.description = description.to_owned();
            }
            if valid_schema {
                tool.parameters = serde_json::from_str(parameters).unwrap();
            }
            assert_eq!(create_tool(Some(&messages)), expected);
        }
    }
}
