//! Description overrides belong to selected help, never the standing facade.

#![expect(
    clippy::unwrap_used,
    reason = "tests assert successful serialization and calls"
)]

use super::*;
use crate::InMemoryMessageBoards;
use crate::MessageBoardHost;
use crate::NotificationDelivery;
use crate::PostPreview;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::SessionId;
use codex_tools::ConversationHistory;
use codex_tools::NoopTurnItemEmitter;
use codex_tools::ToolPayload;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::json;

struct UnusedHost;

impl MessageBoardHost for UnusedHost {
    fn agent_path(
        &self,
        _caller: ThreadId,
    ) -> BoxFuture<'_, codex_protocol::error::Result<AgentPath>> {
        unreachable!("help must not access the host")
    }

    fn resolve_agent(
        &self,
        _path: AgentPath,
    ) -> BoxFuture<'_, codex_protocol::error::Result<ThreadId>> {
        unreachable!("help must not access the host")
    }

    fn current_time(
        &self,
        _caller: ThreadId,
    ) -> BoxFuture<'_, codex_protocol::error::Result<DateTime<Utc>>> {
        unreachable!("help must not access the host")
    }

    fn notify(
        &self,
        _recipient: ThreadId,
        _post: PostPreview,
    ) -> BoxFuture<'_, codex_protocol::error::Result<NotificationDelivery>> {
        unreachable!("help must not access the host")
    }
}

#[tokio::test]
async fn model_descriptions_only_override_selected_help_and_not_parameters() {
    let board = InMemoryMessageBoards::default()
        .open(SessionId::new(), Arc::new(UnusedHost))
        .await;
    let messages: MultiAgentToolMessages = serde_json::from_value(json!({
        "post":{"description":"Model-specific posting guidance", "parameters":"{\"type\":\"object\",\"properties\":{\"typo\":{\"type\":\"string\"}}}"},
        "read_post":{"description":""},
    })).unwrap();
    let tools = message_board_tools_with_descriptions(
        Arc::new(board),
        ThreadId::new(),
        AgentPath::root(),
        None,
        "",
        Some(&messages),
    );
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.tool_name(), ToolName::new(None, AGENT_BOARD_TOOL_NAME));
    let standing = serde_json::to_value(tool.spec()).unwrap();
    assert_eq!(standing["type"], "function");
    assert_eq!(standing["name"], AGENT_BOARD_TOOL_NAME);
    assert!(!standing.to_string().contains("Model-specific"));
    for (topic, expected_description) in [
        ("post", "Model-specific posting guidance"),
        ("read_post", ""),
    ] {
        let call = ToolCall {
            turn_id: "turn".into(),
            call_id: "call".into(),
            tool_name: tool.tool_name(),
            model: "test".into(),
            codex_turn_metadata: None,
            truncation_policy: TruncationPolicy::Bytes(8000),
            source: ToolCallSource::Direct,
            conversation_history: ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: vec![],
            payload: ToolPayload::Function {
                arguments: json!({"action":"help", "topic":topic}).to_string(),
            },
        };
        let output = tool.handle(call).await.unwrap();
        let help: Value = serde_json::from_str(&output.log_output()).unwrap();
        assert_eq!(help["description"], expected_description);
        assert_eq!(help["parameters"]["properties"]["action"]["const"], topic);
        assert!(help["parameters"]["properties"].get("typo").is_none());
        if topic == "post" {
            assert_eq!(help["parameters"]["required"], json!(["action", "text"]));
        } else {
            assert_eq!(
                help["parameters"]["required"],
                json!(["action", "message_id"])
            );
        }
    }
}
