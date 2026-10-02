use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub fn create_test_sync_tool() -> ToolSpec {
    let barrier_properties = BTreeMap::from([
        (
            "id".to_string(),
            JsonSchema::string(Some("Shared rendezvous ID".to_string())),
        ),
        (
            "participants".to_string(),
            JsonSchema::number(Some("Calls required to open barrier".to_string())),
        ),
        (
            "timeout_ms".to_string(),
            JsonSchema::number(Some("Max wait ms, default 1000".to_string())),
        ),
    ]);

    let properties = BTreeMap::from([
        (
            "sleep_before_ms".to_string(),
            JsonSchema::number(Some("Delay before actions, default 0".to_string())),
        ),
        (
            "sleep_after_ms".to_string(),
            JsonSchema::number(Some("Delay after barrier, default 0".to_string())),
        ),
        (
            "barrier".to_string(),
            JsonSchema::object(
                barrier_properties,
                Some(vec!["id".to_string(), "participants".to_string()]),
                Some(false.into()),
            ),
        ),
        (
            "wait_for_git_enrichment".to_string(),
            JsonSchema::boolean(Some(
                "Wait for current-turn Git enrichment, subject to timeout".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "test_sync_tool".to_string(),
        description: "Internal integration-test synchronization".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "test_sync_spec_tests.rs"]
mod tests;
