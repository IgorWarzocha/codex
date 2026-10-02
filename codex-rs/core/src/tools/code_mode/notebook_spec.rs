use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;

pub(super) fn create_notebook_tool() -> ToolSpec {
    let query = JsonSchema::string(None);
    let names = JsonSchema {
        min_items: Some(1),
        ..JsonSchema::array(JsonSchema::string(None), None)
    };
    let hook = JsonSchema::any_of(
        vec![
            JsonSchema::string_enum(vec![json!("startup"), json!("tool_result")], None),
            JsonSchema {
                enum_values: Some(vec![json!(false)]),
                ..JsonSchema::boolean(None)
            },
        ],
        Some("Await self-contained fn(event); tool_result gets {type,toolName,input,status,result?,error?}; hook tool calls do not retrigger; false removes hook".to_string()),
    );
    let variants = vec![
        action(&["status", "list"], vec![("query", query.clone())], &[]),
        action(
            &["checkpoint", "restart", "diagnostics", "reset"],
            vec![],
            &[],
        ),
        action(
            &["save", "load"],
            vec![("name", JsonSchema::string(None))],
            &["name"],
        ),
        action(
            &["pin"],
            vec![("names", names.clone()), ("hook", hook)],
            &["names"],
        ),
        action(&["unpin", "release"], vec![("names", names)], &["names"]),
        action(&["prune"], vec![("query", query)], &["query"]),
    ];
    ToolSpec::Function(ResponsesApiTool {
        name: "notebook".to_string(),
        description: "Control persistent notebook state; status queries memory/bindings by glob; prune removes unpinned matches; list/save/load manage profiles".to_string(),
        strict: false,
        parameters: JsonSchema {
            any_of: Some(variants),
            ..JsonSchema::object(BTreeMap::new(), None, None)
        },
        output_schema: None,
        defer_loading: None,
    })
}

fn action(actions: &[&str], fields: Vec<(&str, JsonSchema)>, required: &[&str]) -> JsonSchema {
    let mut properties = BTreeMap::from([(
        "action".to_string(),
        JsonSchema::string_enum(actions.iter().map(|action| json!(action)).collect(), None),
    )]);
    properties.extend(
        fields
            .into_iter()
            .map(|(name, schema)| (name.to_string(), schema)),
    );
    JsonSchema::object(
        properties,
        Some(
            std::iter::once("action")
                .chain(required.iter().copied())
                .map(str::to_string)
                .collect(),
        ),
        Some(false.into()),
    )
}
