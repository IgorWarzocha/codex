//! Compact facade spec and on-demand action contracts. The host supplies the namespace.

use super::AGENT_BOARD_TOOL_NAME;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use codex_tools::parse_tool_input_schema;
use serde_json::Value;
use serde_json::json;

pub(super) const ACTIONS: [&str; 10] = [
    "create_channel",
    "get_channels",
    "list_threads",
    "search_posts",
    "read_thread",
    "read_post",
    "subscribe",
    "unsubscribe",
    "post",
    "help",
];

pub(super) fn tool(namespace: Option<&str>, namespace_description: &str) -> ToolSpec {
    let parameters = json!({"type":"object","properties":{"action":{"type":"string"}},"required":["action"],"additionalProperties":true});
    let tool = ResponsesApiTool {
        name: AGENT_BOARD_TOOL_NAME.into(),
        description: "Shared agent discussions. action=help lists actions; add topic for arguments"
            .into(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&parameters)
            .unwrap_or_else(|error| panic!("message-board schema must parse: {error}")),
        output_schema: None,
    };
    match namespace {
        Some(namespace) => ToolSpec::Namespace(ResponsesApiNamespace {
            name: namespace.into(),
            description: namespace_description.into(),
            tools: vec![ResponsesApiNamespaceTool::Function(tool)],
        }),
        None => ToolSpec::Function(tool),
    }
}

pub(super) fn action_help(name: &str, description_override: Option<&str>) -> Option<Value> {
    let (description, fields, required): (&str, &[&str], &[&str]) = match name {
        "create_channel" => (
            "Create a channel with collaboration-wide read/post access",
            &["channel_name", "subscribe"],
            &["channel_name"],
        ),
        "get_channels" => (
            "List channels by activity; creation and posts count; case-insensitive name substring search",
            &["query", "recent_first", "limit", "cursor"],
            &[],
        ),
        "list_threads" => (
            "First-post and latest-reply previews; activity sort uses recent replies",
            &[
                "channel_name",
                "sort",
                "recent_first",
                "limit",
                "cursor",
                "max_chars_per_post",
            ],
            &["channel_name"],
        ),
        "search_posts" => (
            "Top-level posts and replies, newest first; case-insensitive substring; omitted query shows recent activity; full text via read_post",
            &[
                "channel_name",
                "query",
                "after_message_id",
                "author",
                "limit",
                "cursor",
                "max_chars_per_post",
            ],
            &[],
        ),
        "read_thread" => (
            "thread_id = first post message ID; each page includes first-post and newest-reply previews; cursor advances through replies",
            &["thread_id", "limit", "cursor", "max_chars_per_post"],
            &["thread_id"],
        ),
        "read_post" => (
            "Post/reply text; Unicode character offsets; continue at next_offset_chars while below n_chars",
            &["message_id", "offset_chars", "limit_chars"],
            &["message_id"],
        ),
        "subscribe" => (
            "Channel top-level posts or thread replies; exactly one of channel_name/thread_id; notifications only during running turns, missed notifications not saved",
            &["channel_name", "thread_id", "target_agent"],
            &[],
        ),
        "unsubscribe" => (
            "Exactly one of channel_name/thread_id; thread unsubscribe survives posting; explicit post notifications still delivered",
            &["channel_name", "thread_id", "target_agent"],
            &[],
        ),
        "post" => (
            "New thread in channel or reply with thread_id = first post message ID; exactly one destination; subscribes author unless previously unsubscribed; agents_to_notify notifies once, without subscribing or starting idle agents; metadata result, no post text",
            &[
                "text",
                "channel_name",
                "new_channel_name",
                "thread_id",
                "agents_to_notify",
            ],
            &["text"],
        ),
        "help" => (
            "Omit topic for the action index; supply one action name for its full argument contract",
            &["topic"],
            &[],
        ),
        _ => return None,
    };
    let mut properties = serde_json::Map::new();
    properties.insert("action".into(), json!({"type":"string","const":name}));
    for field in fields {
        let schema = match *field {
            "new_channel_name" => {
                json!({"type":"string","description":"Subscribe to created channel"})
            }
            "subscribe" => {
                json!({"type":"boolean","default":true,"description":"Subscribe creator to new top-level posts"})
            }
            "author" => {
                json!({"type":"string","description":"Absolute agent path or relative to caller"})
            }
            "target_agent" => {
                json!({"type":"string","description":"Absolute agent path or relative to caller; omitted = caller"})
            }
            "recent_first" => json!({"type":"boolean","default":true}),
            "limit" => {
                json!({"type":"integer","minimum":1,"default":20,"description":"Capped at 50 and output budget; continue with next_cursor"})
            }
            "offset_chars" => json!({"type":"integer","minimum":0,"default":0}),
            "limit_chars" => {
                json!({"type":"integer","minimum":1,"default":20000,"description":"Capped at 20000 and output budget"})
            }
            "max_chars_per_post" => {
                json!({"type":"integer","minimum":1,"default":1000,"description":"Capped at 20000 and output budget"})
            }
            "sort" => json!({"type":"string","enum":["created","activity"],"default":"created"}),
            "agents_to_notify" => {
                json!({"type":"array","items":{"type":"string"},"maxItems":256,"description":"Absolute agent paths or relative to caller"})
            }
            "topic" => json!({"type":"string","enum":ACTIONS}),
            "cursor" => {
                json!({"type":"string","description":"Returned next_cursor; same filters/sort; concurrent posts may shift pages; omit to refresh"})
            }
            _ => json!({"type":"string"}),
        };
        properties.insert((*field).into(), schema);
    }
    let required: Vec<_> = std::iter::once("action")
        .chain(required.iter().copied())
        .collect();
    Some(json!({
        "action":name,
        "description":description_override.unwrap_or(description),
        "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
    }))
}
