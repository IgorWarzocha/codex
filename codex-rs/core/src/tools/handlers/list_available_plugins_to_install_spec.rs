use codex_tools::JsonSchema;
use codex_tools::LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME;
use codex_tools::REQUEST_PLUGIN_INSTALL_TOOL_NAME;
use codex_tools::ResponsesApiTool;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::ToolSpec;
pub(crate) fn create_list_available_plugins_to_install_tool() -> ToolSpec {
    let description = format!(
        "Install candidates only for explicit user-requested unavailable plugin or connector, after {TOOL_SEARCH_TOOL_NAME} fails to make it callable or is unavailable\nCandidates to {REQUEST_PLUGIN_INSTALL_TOOL_NAME}; matching plugin over connector unless already installed"
    );

    ToolSpec::Function(ResponsesApiTool {
        name: LIST_AVAILABLE_PLUGINS_TO_INSTALL_TOOL_NAME.to_string(),
        description,
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(Default::default(), Some(Vec::new()), Some(false.into())),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn create_list_available_plugins_to_install_tool_uses_expected_wire_shape() {
        assert_eq!(
            create_list_available_plugins_to_install_tool(),
            ToolSpec::Function(ResponsesApiTool {
                name: "list_available_plugins_to_install".to_string(),
                description: "Install candidates only for explicit user-requested unavailable plugin or connector, after tool_search fails to make it callable or is unavailable\nCandidates to request_plugin_install; matching plugin over connector unless already installed".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    Default::default(),
                    Some(Vec::new()),
                    Some(false.into()),
                ),
                output_schema: None,
            })
        );
    }
}
