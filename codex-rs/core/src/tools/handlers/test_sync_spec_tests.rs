use super::*;
use codex_tools::JsonSchema;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn test_sync_tool_matches_expected_spec() {
    assert_eq!(
        create_test_sync_tool(),
        ToolSpec::Function(ResponsesApiTool {
            name: "test_sync_tool".to_string(),
            description: "Internal integration-test synchronization".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([
                    (
                        "barrier".to_string(),
                        JsonSchema::object(
                            BTreeMap::from([
                                (
                                    "id".to_string(),
                                    JsonSchema::string(Some("Shared rendezvous ID".to_string(),)),
                                ),
                                (
                                    "participants".to_string(),
                                    JsonSchema::number(Some(
                                        "Calls required to open barrier".to_string(),
                                    )),
                                ),
                                (
                                    "timeout_ms".to_string(),
                                    JsonSchema::number(Some(
                                        "Max wait ms, default 1000".to_string(),
                                    )),
                                ),
                            ]),
                            Some(vec!["id".to_string(), "participants".to_string()]),
                            Some(false.into()),
                        ),
                    ),
                    (
                        "sleep_after_ms".to_string(),
                        JsonSchema::number(Some("Delay after barrier, default 0".to_string(),)),
                    ),
                    (
                        "sleep_before_ms".to_string(),
                        JsonSchema::number(Some("Delay before actions, default 0".to_string(),)),
                    ),
                    (
                        "wait_for_git_enrichment".to_string(),
                        JsonSchema::boolean(Some(
                            "Wait for current-turn Git enrichment, subject to timeout".to_string(),
                        )),
                    ),
                ]),
                /*required*/ None,
                Some(false.into())
            ),
            output_schema: None,
        })
    );
}
