use std::sync::Arc;

use codex_extension_api::NotesCheckpointTracker;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_login::AuthHeaders;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolOutput;
use codex_tools::ToolPayload;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::HistoryNotesAction;
use super::HistoryNotesTool;
use super::HistoryNotesToolOutput;
use crate::backend::HistoryNotesBackend;

#[test]
fn preserves_encrypted_history_output() {
    let result = HistoryNotesToolOutput::new(
        json!({"encrypted_output": "enc_payload"}),
        "call-1".to_string(),
    )
    .expect("valid output")
    .to_response_item(
        "call-1",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );

    let ResponseInputItem::FunctionCallOutput { output, .. } = result else {
        panic!("expected function-call output");
    };
    assert_eq!(
        output.content_items(),
        Some(
            [FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "enc_payload".to_string(),
            }]
            .as_slice()
        )
    );
}

#[test]
fn preserves_images_as_separate_output_items_without_logging_bytes() {
    let output = HistoryNotesToolOutput::new(
        json!({
            "encrypted_output": "enc_payload",
            "images": [
                {"data": "cG5n", "mime_type": "image/png", "detail": "original"},
                {"data": "anBlZw==", "mime_type": "image/jpeg", "detail": "low"},
                {"data": "Z2lm", "mime_type": "image/gif"},
                {"data": "d2VicA==", "mime_type": "image/webp", "detail": null}
            ]
        }),
        "call-1".to_string(),
    )
    .expect("valid image output");
    assert_eq!(
        output.log_output(),
        json!({"encrypted_output": "enc_payload"}).to_string()
    );
    assert_eq!(
        output.post_tool_use_response(
            "call-1",
            &ToolPayload::Function {
                arguments: "{}".to_string()
            }
        ),
        Some(json!({"encrypted_output": "enc_payload"}))
    );
    assert_eq!(
        output.to_response_item(
            "call-1",
            &ToolPayload::Function {
                arguments: "{}".to_string()
            }
        ),
        ResponseInputItem::FunctionCallOutput {
            call_id: "call-1".to_string(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "enc_payload".to_string()
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,cG5n".to_string()
                    },
                    detail: Some(ImageDetail::Original)
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/jpeg;base64,anBlZw==".to_string()
                    },
                    detail: Some(ImageDetail::Low)
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/gif;base64,Z2lm".to_string()
                    },
                    detail: None
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/webp;base64,d2VicA==".to_string()
                    },
                    detail: None
                },
            ])
        }
    );
}

#[test]
fn accepts_empty_attachments_and_legacy_plaintext_results() {
    for (result, expected) in [
        (
            json!({"encrypted_output": "enc_payload", "images": []}),
            FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "enc_payload".to_string(),
                },
            ]),
        ),
        (
            json!({"text": "legacy result"}),
            FunctionCallOutputPayload::from_text(json!({"text": "legacy result"}).to_string()),
        ),
    ] {
        let output =
            HistoryNotesToolOutput::new(result, "call-1".to_string()).expect("valid output");
        assert_eq!(
            output.to_response_item(
                "call-1",
                &ToolPayload::Function {
                    arguments: "{}".to_string()
                }
            ),
            ResponseInputItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: expected
            }
        );
    }
}

#[test]
fn rejects_malformed_attachments_instead_of_silently_dropping_them() {
    for images in [
        json!(null),
        json!({}),
        json!([null]),
        json!([{"mime_type": "image/png"}]),
        json!([{"data": "private-image-bytes"}]),
        json!([{"data": "private-image-bytes", "mime_type": "image/png", "detail": "invalid"}]),
    ] {
        let result = HistoryNotesToolOutput::new(
            json!({"encrypted_output": "enc_payload", "images": images}),
            "call-1".to_string(),
        );
        let Err(codex_extension_api::FunctionCallError::RespondToModel(message)) = result else {
            panic!("expected a model-facing image error");
        };
        assert_eq!(message, "History backend returned invalid image content.");
    }
}

#[tokio::test]
async fn executor_groups_cell_retries_and_credits_only_validated_protected_writes() {
    let server = MockServer::start().await;
    for (note_path, response) in [
        ("http-failure", ResponseTemplate::new(500)),
        (
            "saved",
            ResponseTemplate::new(200).set_body_json(json!({"encrypted_output": "receipt"})),
        ),
        (
            "plaintext",
            ResponseTemplate::new(200).set_body_json(json!({"success": true})),
        ),
        (
            "empty",
            ResponseTemplate::new(200).set_body_json(json!({"encrypted_output": ""})),
        ),
        (
            "protected-error",
            ResponseTemplate::new(200).set_body_json(json!({
                "encrypted_output": "receipt", "error": "not saved",
            })),
        ),
    ] {
        Mock::given(method("POST"))
            .and(path("/backend-api/codex/alpha/notes/v2/write_file"))
            .and(body_partial_json(json!({"path": note_path})))
            .respond_with(response)
            .mount(&server)
            .await;
    }
    let provider = create_model_provider(
        ModelProviderInfo::create_openai_provider(Some(format!(
            "{}/backend-api/codex",
            server.uri()
        ))),
        Some(AuthManager::from_auth_for_testing(CodexAuth::Headers(
            AuthHeaders::new(http::HeaderMap::new()),
        ))),
    );
    let tracker = Arc::new(NotesCheckpointTracker::default());
    tracker.begin_run("run");
    let tool = HistoryNotesTool::new(
        HistoryNotesAction::NotesWriteFile,
        HistoryNotesBackend::new(
            provider,
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        ),
        "session".to_string(),
        "/root".to_string(),
    )
    .with_checkpoint_tracker(Arc::clone(&tracker));
    for (index, (cell_id, note_path, expected)) in [
        ("failed-cell", "http-failure", false),
        ("failed-cell", "saved", false),
        ("retry-cell", "saved", true),
        ("plaintext-cell", "plaintext", false),
        ("empty-cell", "empty", false),
        ("error-cell", "protected-error", false),
        ("final-retry-cell", "saved", true),
    ]
    .into_iter()
    .enumerate()
    {
        // A wait or follow-up sampling request must not split writes in the same cell.
        tracker.begin_batch("run");
        tracker.register_cell(tracker.capture_batch("run").unwrap(), cell_id);
        let result = tool
            .handle(ToolCall {
                turn_id: "run".to_string(),
                call_id: format!("call-{index}"),
                tool_name: tool.tool_name(),
                model: "test-model".to_string(),
                codex_turn_metadata: None,
                truncation_policy: TruncationPolicy::Bytes(1024),
                source: ToolCallSource::CodeMode {
                    cell_id: cell_id.to_string(),
                    runtime_tool_call_id: index.to_string(),
                },
                conversation_history: Default::default(),
                turn_item_emitter: Arc::new(codex_extension_api::NoopTurnItemEmitter),
                environments: Vec::new(),
                payload: ToolPayload::Function {
                    arguments: json!({"path": note_path, "text": "checkpoint"}).to_string(),
                },
            })
            .await;
        assert_eq!(result.is_err(), note_path == "http-failure");
        assert_eq!(tracker.has_successful_notes("run"), expected, "{cell_id}");
    }
}
