use codex_tools::JsonSchema;
use codex_tools::LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME;
use codex_tools::REQUEST_PLUGIN_INSTALL_TOOL_NAME;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

use crate::tools::router::ToolSuggestPresentation;

pub(crate) fn create_request_plugin_install_tool(
    presentation: ToolSuggestPresentation,
) -> ToolSpec {
    let (properties, required, description) = match presentation {
        ToolSuggestPresentation::ListTool => (
            BTreeMap::from([
                (
                    "tool_type".to_string(),
                    JsonSchema::string(Some("connector or plugin".to_string())),
                ),
                (
                    "action_type".to_string(),
                    JsonSchema::string(Some("install".to_string())),
                ),
                (
                    "tool_id".to_string(),
                    JsonSchema::string(Some("Returned candidate ID".to_string())),
                ),
                (
                    "suggest_reason".to_string(),
                    JsonSchema::string(Some("One-line user-facing reason".to_string())),
                ),
            ]),
            vec![
                "tool_type".to_string(),
                "action_type".to_string(),
                "tool_id".to_string(),
                "suggest_reason".to_string(),
            ],
            format!(
                "Request installation only for an exact match to the user's explicit request returned by `{LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME}`. Pass its tool_type and id as tool_type and tool_id. Not for adjacent capabilities or general recommendations. Do not call in parallel with other tools."
            ),
        ),
        ToolSuggestPresentation::RecommendationContext => (
            BTreeMap::from([
                (
                    "plugin_id".to_string(),
                    JsonSchema::string(Some(
                        "Parenthesized ID from <recommended_plugins>"
                            .to_string(),
                    )),
                ),
                (
                    "suggest_reason".to_string(),
                    JsonSchema::string(Some("One-line user-facing reason".to_string())),
                ),
            ]),
            vec!["plugin_id".to_string(), "suggest_reason".to_string()],
            "Suggest installation only for a specific unavailable plugin the user explicitly requested, after exhausting tool search, and only from <recommended_plugins>. Not for adjacent capabilities or general recommendations. Do not call in parallel with other tools.".to_string(),
        ),
    };

    ToolSpec::Function(ResponsesApiTool {
        name: REQUEST_PLUGIN_INSTALL_TOOL_NAME.to_string(),
        description,
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_tools::JsonSchema;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

    #[test]
    fn create_request_plugin_install_tool_uses_expected_legacy_wire_shape() {
        let expected_description = "Request installation only for an exact match to the user's explicit request returned by `list_available_plugins_to_install`. Pass its tool_type and id as tool_type and tool_id. Not for adjacent capabilities or general recommendations. Do not call in parallel with other tools.";

        assert_eq!(
            create_request_plugin_install_tool(ToolSuggestPresentation::ListTool),
            ToolSpec::Function(ResponsesApiTool {
                name: "request_plugin_install".to_string(),
                description: expected_description.to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([
                        (
                            "action_type".to_string(),
                            JsonSchema::string(Some("install".to_string(),),),
                        ),
                        (
                            "suggest_reason".to_string(),
                            JsonSchema::string(Some("One-line user-facing reason".to_string(),),),
                        ),
                        (
                            "tool_id".to_string(),
                            JsonSchema::string(Some("Returned candidate ID".to_string(),),),
                        ),
                        (
                            "tool_type".to_string(),
                            JsonSchema::string(Some("connector or plugin".to_string(),),),
                        ),
                    ]),
                    Some(vec![
                        "tool_type".to_string(),
                        "action_type".to_string(),
                        "tool_id".to_string(),
                        "suggest_reason".to_string(),
                    ]),
                    Some(false.into())
                ),
                output_schema: None,
            })
        );
    }

    #[test]
    fn recommendation_context_uses_simplified_plugin_wire_shape() {
        let expected_description = "Suggest installation only for a specific unavailable plugin the user explicitly requested, after exhausting tool search, and only from <recommended_plugins>. Not for adjacent capabilities or general recommendations. Do not call in parallel with other tools.";

        assert_eq!(
            create_request_plugin_install_tool(ToolSuggestPresentation::RecommendationContext),
            ToolSpec::Function(ResponsesApiTool {
                name: "request_plugin_install".to_string(),
                description: expected_description.to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([
                        (
                            "plugin_id".to_string(),
                            JsonSchema::string(Some(
                                "Parenthesized ID from <recommended_plugins>".to_string(),
                            )),
                        ),
                        (
                            "suggest_reason".to_string(),
                            JsonSchema::string(Some("One-line user-facing reason".to_string())),
                        ),
                    ]),
                    Some(vec!["plugin_id".to_string(), "suggest_reason".to_string()]),
                    Some(false.into()),
                ),
                output_schema: None,
            })
        );
    }
}
