use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub(crate) fn create_wait_tool(
    description_override: Option<&str>,
    parameters_override: Option<&str>,
) -> ToolSpec {
    let properties = BTreeMap::from([
        ("cell_id".to_string(), JsonSchema::string(None)),
        (
            "yield_time_ms".to_string(),
            JsonSchema::number(Some("Wait ms, default 10000".to_string())),
        ),
        (
            "max_tokens".to_string(),
            JsonSchema::number(Some("Output tokens, default 10000".to_string())),
        ),
        (
            "terminate".to_string(),
            JsonSchema::boolean(Some(
                "Stop cell, not wait. Notebook: terminates the kernel".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: codex_code_mode::WAIT_TOOL_NAME.to_string(),
        description: description_override
            .map(str::to_owned)
            .unwrap_or_else(|| codex_code_mode::build_wait_tool_description().to_string()),
        strict: false,
        parameters: parameters_override
            .and_then(
                |parameters| match crate::tools::catalog_parameters::parse(parameters) {
                    Ok(parameters) => Some(parameters),
                    Err(reason) => {
                        tracing::warn!(
                            tool = codex_code_mode::WAIT_TOOL_NAME,
                            reason,
                            "Invalid catalog tool parameters; using bundled parameters"
                        );
                        None
                    }
                },
            )
            .unwrap_or_else(|| {
                JsonSchema::object(
                    properties,
                    Some(vec!["cell_id".to_string()]),
                    Some(false.into()),
                )
            }),
        output_schema: None,
        defer_loading: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn create_wait_tool_matches_expected_spec() {
        assert_eq!(
            create_wait_tool(
                /*description_override*/ None, /*parameters_override*/ None
            ),
            ToolSpec::Function(ResponsesApiTool {
                name: codex_code_mode::WAIT_TOOL_NAME.to_string(),
                description: codex_code_mode::build_wait_tool_description().to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([
                        ("cell_id".to_string(), JsonSchema::string(None),),
                        (
                            "max_tokens".to_string(),
                            JsonSchema::number(Some("Output tokens, default 10000".to_string(),)),
                        ),
                        (
                            "terminate".to_string(),
                            JsonSchema::boolean(Some(
                                "Stop cell, not wait. Notebook: terminates the kernel".to_string(),
                            )),
                        ),
                        (
                            "yield_time_ms".to_string(),
                            JsonSchema::number(Some("Wait ms, default 10000".to_string(),)),
                        ),
                    ]),
                    Some(vec!["cell_id".to_string()]),
                    Some(false.into()),
                ),
                output_schema: None,
            })
        );
    }
}
