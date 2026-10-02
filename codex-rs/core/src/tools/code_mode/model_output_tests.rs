//! Local native-output fixtures exercise delivery, not provider encryption.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::delegate::CodeModeCellDelegate;
use super::delegate::CodeModeDispatchBroker;
use crate::context_manager::ContextManager;
use crate::session::TurnInput;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use crate::state::ActiveTurn;
use crate::tools::context::ToolInvocation;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::turn_diff_tracker::TurnDiffTracker;
use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::CodeModeToolKind;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::InputModality;
use codex_tools::ToolName;
use codex_tools::ToolOutput;
use codex_tools::ToolPayload;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct ModelOnlyOutput {
    nested_call_id: String,
    encrypted: bool,
    attachments: bool,
}

impl ToolOutput for ModelOnlyOutput {
    fn log_output(&self) -> String {
        "model-only fixture".to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: self.code_mode_model_output(payload).unwrap(),
        }
    }

    fn code_mode_model_output(&self, _payload: &ToolPayload) -> Option<FunctionCallOutputPayload> {
        let text = format!("native result {}", self.nested_call_id);
        let body = if !self.encrypted && !self.attachments {
            FunctionCallOutputBody::Text(text)
        } else {
            let mut items = vec![FunctionCallOutputContentItem::InputText { text }];
            if self.encrypted {
                items.push(FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: format!("opaque-{}", self.nested_call_id),
                });
            }
            if self.attachments {
                items.push(FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline { image_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==".to_string() },
                    detail: None,
                });
            }
            FunctionCallOutputBody::ContentItems(items)
        };
        Some(FunctionCallOutputPayload {
            body,
            success: Some(true),
        })
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        json!({"delivered_to": "model", "call_id": self.nested_call_id})
    }
}

#[derive(Clone, Copy)]
struct ModelOnlyHandler {
    encrypted: bool,
    attachments: bool,
}

impl ToolExecutor<ToolInvocation> for ModelOnlyHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("relay_probe").with_default_namespace()
    }

    fn spec(&self) -> codex_tools::ToolSpec {
        codex_tools::ToolSpec::Function(codex_tools::ResponsesApiTool {
            name: "relay_probe".to_string(),
            description: "Native model-only output fixture".to_string(),
            strict: false,
            defer_loading: None,
            parameters: codex_tools::JsonSchema::default(),
            output_schema: None,
        })
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            Ok(Box::new(ModelOnlyOutput {
                nested_call_id: invocation.call_id,
                encrypted: self.encrypted,
                attachments: self.attachments,
            }) as Box<dyn ToolOutput>)
        })
    }
}

impl CoreToolRuntime for ModelOnlyHandler {}

#[test_case::test_case(true, true, true; "encrypted results with attachments")]
#[test_case::test_case(true, false, false; "legacy pure plaintext")]
#[test_case::test_case(true, false, true; "legacy plaintext with attachments")]
#[test_case::test_case(false, true, true; "inactive encrypted relay rejects receipts")]
#[test_case::test_case(false, false, true; "inactive plaintext relay rejects receipts")]
#[tokio::test]
async fn native_relay_delivers_before_receipts(active: bool, encrypted: bool, attachments: bool) {
    let (session, turn) = make_session_and_context().await;
    let session = Arc::new(session);
    if active {
        *session.active_turn.lock().await = Some(ActiveTurn::default());
    }
    let turn = Arc::new(turn);
    let handler = ModelOnlyHandler {
        encrypted,
        attachments,
    };
    let router = Arc::new(ToolRouter::from_registry(
        &turn,
        turn.model_info(),
        ToolRegistry::with_handler_for_test(Arc::new(handler)),
        Vec::new(),
        &Default::default(),
    ));
    let step = StepContext::for_test(turn).with_tool_router_for_test(router);
    let broker = Arc::new(CodeModeDispatchBroker::new(
        session.thread_id,
        Default::default(),
    ));
    let cell_id = CellId::new("relay-cell".to_string());
    broker.mark_cell_ready_for_dispatch(&cell_id, None);
    let delegate = CodeModeCellDelegate {
        broker: Arc::clone(&broker),
        step_context: Arc::clone(&step),
        outer_call_id: "original-exec-call".to_string(),
    };
    let _worker = broker.start_turn_worker(
        Arc::clone(&session),
        step,
        Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
    );
    let invocation = |id: &str| CodeModeNestedToolCall {
        cell_id: cell_id.clone(),
        runtime_tool_call_id: id.to_string(),
        tool_name: ToolName::plain("relay_probe"),
        tool_kind: CodeModeToolKind::Function,
        input: Some(json!({})),
    };
    let (first, second) = tokio::join!(
        delegate.invoke_tool(invocation("one"), CancellationToken::new()),
        delegate.invoke_tool(invocation("two"), CancellationToken::new()),
    );
    let (pending, _) = session
        .input_queue
        .get_pending_input(&session.active_turn)
        .await;
    if !active {
        for response in [first, second] {
            assert_eq!(
                response,
                Err("failed to inject exec output for cell relay-cell: no active turn".to_string())
            );
        }
        assert!(pending.is_empty());
        return;
    }
    let ids = [first.unwrap(), second.unwrap()]
        .map(|receipt| {
            let id = receipt["call_id"].as_str().unwrap().to_string();
            assert_eq!(receipt, json!({"delivered_to": "model", "call_id": id}));
            id
        })
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    assert_eq!(pending.len(), 2);
    let mut relayed_ids = BTreeSet::new();
    let mut relayed_items = Vec::new();
    for item in pending {
        let TurnInput::ResponseItem(envelope) = item else {
            panic!("expected native output")
        };
        relayed_items.push(envelope.item.clone());
        let output = match envelope.item {
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } if encrypted => {
                assert_eq!(call_id.as_deref(), Some("original-exec-call"));
                output
            }
            ResponseItem::CustomToolCallOutput {
                call_id,
                name,
                output,
                ..
            } if !encrypted => {
                assert_eq!(call_id, "original-exec-call");
                assert_eq!(name.as_deref(), Some("exec"));
                output
            }
            _ => panic!(
                "encrypted relays require function output; plaintext relays require custom output"
            ),
        };
        assert_eq!(output.success, Some(true));
        let FunctionCallOutputBody::ContentItems(items) = output.body else {
            panic!("expected content items")
        };
        let Some(FunctionCallOutputContentItem::InputText { text }) = items.first() else {
            panic!("expected attribution")
        };
        let id = text
            .strip_prefix(&format!("Nested tool {}, call_id ", handler.tool_name()))
            .and_then(|text| text.strip_suffix(": model-only output"))
            .unwrap();
        let expected = ModelOnlyOutput {
            nested_call_id: id.to_string(),
            encrypted,
            attachments,
        }
        .code_mode_model_output(&ToolPayload::Function {
            arguments: "{}".to_string(),
        })
        .unwrap();
        let expected_items = match expected.body {
            FunctionCallOutputBody::Text(text) => {
                vec![FunctionCallOutputContentItem::InputText { text }]
            }
            FunctionCallOutputBody::ContentItems(items) => items,
        };
        assert_eq!(&items[1..], expected_items.as_slice());
        relayed_ids.insert(id.to_string());
        assert!(
            envelope
                .metadata
                .unwrap()
                .history_truncation_token_limit
                .is_some()
        );
    }
    assert_eq!(relayed_ids, ids);
    // Exercise production prompt preparation, not just successful queue insertion.
    // Both output kinds share the outer exec call and its ordinary JS receipt.
    let mut prepared = vec![ResponseItem::CustomToolCall {
        id: None,
        status: None,
        call_id: "original-exec-call".to_string(),
        name: "exec".to_string(),
        namespace: None,
        input: "await Promise.all([tools.relay_probe({}), tools.relay_probe({})])".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }];
    prepared.extend(relayed_items);
    prepared.push(
        ResponseInputItem::CustomToolCallOutput {
            call_id: "original-exec-call".to_string(),
            name: Some("exec".to_string()),
            output: FunctionCallOutputPayload::from_text(json!({"receipts": ids}).to_string()),
        }
        .into(),
    );
    let mut history = ContextManager::new();
    history.record_items(prepared.iter(), TruncationPolicy::Tokens(10_000));
    assert_eq!(
        history.for_prompt(&[InputModality::Text, InputModality::Image]),
        prepared
    );
}
