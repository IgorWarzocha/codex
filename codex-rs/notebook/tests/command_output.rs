//! Command presentation follows the original tool identity without changing JS results.

use std::sync::Arc;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeToolKind;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::ToolDefinition;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_notebook::DenoNotebookSessionProvider;
use codex_protocol::ToolName;
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct Echo;

impl CodeModeSessionDelegate for Echo {
    fn invoke_tool<'a>(
        &'a self,
        call: CodeModeNestedToolCall,
        _cancellation: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move { Ok(call.input.unwrap_or(Value::Null)) })
    }

    fn notify<'a>(
        &'a self,
        _call: String,
        _cell: CellId,
        _text: String,
        _cancellation: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell: &CellId) {}
}

#[allow(clippy::unwrap_used)]
async fn execute(session: &Arc<dyn CodeModeSession>, source: &str) -> Vec<String> {
    let enabled_tools = [
        ("renamed", ToolName::plain("exec_command")),
        (
            "write_stdin",
            ToolName::namespaced("functions", "write_stdin"),
        ),
        ("external", ToolName::namespaced("server", "exec_command")),
    ]
    .into_iter()
    .map(|(name, tool_name)| ToolDefinition {
        name: name.into(),
        tool_name,
        description: "Return the supplied command result".into(),
        kind: CodeModeToolKind::Function,
        input_schema: None,
        input_schema_max_bytes: None,
        output_schema: None,
    })
    .collect();
    let response = session
        .execute(
            ExecuteRequest {
                tool_call_id: "command-presentation".into(),
                source: source.into(),
                enabled_tools,
                yield_time_ms: Some(10_000),
                max_output_tokens: None,
            },
            Arc::new(Echo),
            None,
        )
        .await
        .unwrap()
        .initial_response()
        .await
        .unwrap();
    let RuntimeResponse::Result {
        content_items,
        error_text: None,
        ..
    } = response
    else {
        panic!("unexpected response: {response:?}");
    };
    content_items
        .into_iter()
        .map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => text,
            other => panic!("unexpected content: {other:?}"),
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires a local Deno Jupyter executable"]
async fn command_projection_preserves_truncation_continuations_and_unrelated_objects() {
    let cwd = tempfile::tempdir().unwrap();
    let deno = std::env::var_os("DENO_PROGRAM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "deno".into());
    for plain in [false, true] {
        let provider = DenoNotebookSessionProvider::new(deno.clone(), cwd.path().into())
            .with_plain_command_output(plain);
        let session = provider.create_session().await.unwrap();
        let output = execute(&session, r#"
var savedCommand = await tools.renamed({output:"head\n[truncated]", exit_code:7, truncated:true, original_token_count:5000, chunk_id:"hidden", wall_time_seconds:1.25});
text(savedCommand);
text({...savedCommand});
text(await tools.external({...savedCommand}));
text(await tools.write_stdin({output:"tail", session_id:42, chunk_id:"hidden", wall_time_seconds:0.25, original_token_count:1}));
text(savedCommand.output);
text(42n);
var cyclic = {}; cyclic.self = cyclic; text(cyclic);
text(undefined);
"#).await;
        assert_eq!(output.len(), 8);
        let projected: Value = serde_json::from_str(if plain {
            let (metadata, text) = output[0].split_once("\nOutput:\n").unwrap();
            assert_eq!(text, "head\n[truncated]");
            metadata
        } else {
            &output[0]
        })
        .unwrap();
        assert_eq!(projected["truncated"], true);
        assert_eq!(projected["original_token_count"], 5000);
        assert_eq!(projected["exit_code"], 7);
        assert!(projected.get("chunk_id").is_none());
        assert!(projected.get("wall_time_seconds").is_none());
        for raw in [&output[1], &output[2]] {
            let raw: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(raw["chunk_id"], "hidden");
            assert_eq!(raw["wall_time_seconds"], 1.25);
        }
        if plain {
            assert_eq!(output[3], "{\"session_id\":42}\nOutput:\ntail");
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&output[3]).unwrap(),
                json!({"session_id":42,"output":"tail"})
            );
        }
        assert_eq!(output[4], "head\n[truncated]");
        assert_eq!(output[5..], ["42", "[object Object]", "undefined"]);
        // The original result remains recognizable in later cells of this kernel.
        assert_eq!(
            execute(&session, "text(savedCommand);").await,
            vec![output[0].clone()]
        );
        session.shutdown().await.unwrap();
    }
}
