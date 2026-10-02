//! Schemas for shared discussion tools. The host supplies their namespace.

use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use codex_tools::parse_tool_input_schema;
use serde_json::json;

pub(super) const NAMES: [&str; 9] = [
    "create_channel",
    "get_channels",
    "list_threads",
    "search_posts",
    "read_thread",
    "read_post",
    "subscribe",
    "unsubscribe",
    "post",
];

pub(super) fn tool(
    name: &str,
    namespace: Option<&str>,
    namespace_description: &str,
    description_override: Option<&str>,
) -> ToolSpec {
    let (description, fields, required): (&str, &[&str], &[&str]) = match name {
        "create_channel" => (
            "Collaboration-wide read/post access; creator subscribed to new top-level posts by default",
            &["channel_name", "subscribe"],
            &["channel_name"],
        ),
        "get_channels" => (
            "Most recently active first; creation and posts count as activity; case-insensitive name substring search",
            &["query", "recent_first", "limit", "cursor"],
            &[],
        ),
        "list_threads" => (
            "First-post and latest-reply previews; newest threads first by default; activity sort uses recent replies",
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
            "Top-level posts and replies, newest first; case-insensitive substring; omitted query shows recent activity; previews, full text via read_post",
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
        _ => unreachable!("only registered message-board tools have schemas"),
    };
    let mut properties = serde_json::Map::new();
    for field in fields {
        let schema = match *field {
            "new_channel_name" => {
                json!({"type":"string","description":"Subscribe to created channel"})
            }
            "subscribe" => {
                json!({"type":"boolean","description":"New top-level posts; default true"})
            }
            "author" => {
                json!({"type":"string","description":"Absolute agent path or relative to caller"})
            }
            "target_agent" => {
                json!({"type":"string","description":"Absolute agent path or relative to caller; omitted = caller"})
            }
            "recent_first" => json!({"type":"boolean","description":"Default true"}),
            "limit" => {
                json!({"type":"integer","minimum":1,"description":"Default 20; capped at 50 and output budget; continue with next_cursor"})
            }
            "offset_chars" => json!({"type":"integer","minimum":0,"description":"Default 0"}),
            "limit_chars" | "max_chars_per_post" => {
                json!({"type":"integer","minimum":1,"description":"Output-budget capped; defaults: limit_chars 20000, max_chars_per_post 1000"})
            }
            "sort" => json!({"type":"string","enum":["created","activity"]}),
            "agents_to_notify" => {
                json!({"type":"array","items":{"type":"string"},"description":"Absolute agent paths or relative to caller; maximum 256"})
            }
            "cursor" => {
                json!({"type":"string","description":"Returned next_cursor; same filters/sort; concurrent posts may shift pages; omit to refresh"})
            }
            _ => json!({"type":"string"}),
        };
        properties.insert((*field).into(), schema);
    }
    let parameters = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let tool = ResponsesApiTool {
        name: name.into(),
        description: description_override.unwrap_or(description).into(),
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
