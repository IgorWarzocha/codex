use super::*;
use codex_history::CodexHarnessMetadata;

#[test]
fn rewritten_output_preserves_harness_metadata() {
    let envelope = ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-1".to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text("large output".repeat(100)),
                success: Some(true),
            },
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: Some(CodexHarnessMetadata::default()),
    };

    let rewritten = rewritten_output_for_context_window(&envelope)
        .expect("function output should be rewritten");

    assert_eq!(rewritten.metadata, envelope.metadata);
    assert_ne!(rewritten.item, envelope.item);
}

fn text_output(text: String) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some("plain".to_string()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_text(text),
        internal_chat_message_metadata_passthrough: None,
    })
}

#[test]
fn compaction_budget_boundary_keeps_full_large_output_until_exceeded() {
    // This output is much larger than ordinary configured active windows.
    let output = text_output("x".repeat(800_000 * 4));
    let output_tokens = estimate_item_token_count(&output.item);
    for offset in [-1, 0, 1] {
        let instructions = BaseInstructions {
            text: "i"
                .repeat(((COMPACTION_INPUT_TOKEN_BUDGET - output_tokens + offset) * 4) as usize),
            ..Default::default()
        };
        let mut history = ContextManager::default();
        history.replace_annotated(vec![output.clone()]);
        let (rewritten, deleted) =
            trim_function_call_history_to_fit_compaction_budget(&mut history, &instructions);
        if offset <= 0 {
            assert_eq!((rewritten, deleted), (0, 0));
            assert_eq!(history.annotated_items(), &[output.clone()]);
        } else {
            assert_eq!(rewritten, 1);
            assert!(deleted > 0);
            assert_ne!(history.annotated_items(), &[output.clone()]);
        }
    }
}

#[test]
fn over_budget_skips_encrypted_history_and_notes_and_shrinks_preceding_output() {
    let plain = text_output("p".repeat(900_000 * 4));
    let encrypted_body = FunctionCallOutputPayload::from_content_items(vec![
        FunctionCallOutputContentItem::InputText {
            text: "remote receipt".to_string(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "opaque history or notes".repeat(100),
        },
    ]);
    let encrypted = [
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("history".to_string()),
            name: None,
            namespace: None,
            output: encrypted_body.clone(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::CustomToolCallOutput {
            id: None,
            call_id: "notes".to_string(),
            name: None,
            output: encrypted_body,
            internal_chat_message_metadata_passthrough: None,
        },
    ]
    .map(|item| ResponseItemEnvelope {
        item,
        metadata: Some(CodexHarnessMetadata::default()),
    });
    let mut history = ContextManager::default();
    history.replace_annotated(vec![
        plain.clone(),
        encrypted[0].clone(),
        encrypted[1].clone(),
    ]);
    let (rewritten, deleted) = trim_function_call_history_to_fit_compaction_budget(
        &mut history,
        &BaseInstructions {
            text: String::new(),
            ..Default::default()
        },
    );
    assert_eq!(rewritten, 1);
    assert!(deleted > 0);
    assert_eq!(&history.annotated_items()[1..], &encrypted);
    assert_ne!(history.annotated_items()[0], plain);
}

#[test]
fn encrypted_only_overflow_is_preserved_not_laundered_into_plaintext() {
    let mut item = text_output(String::new());
    if let ResponseItem::FunctionCallOutput { output, .. } = &mut item.item {
        *output = FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "e".repeat(8_000_000),
            },
        ]);
    }
    let mut history = ContextManager::default();
    history.replace_annotated(vec![item.clone()]);
    // Even if there are no eligible outputs, return the original opaque input.
    let result = trim_function_call_history_to_fit_compaction_budget(
        &mut history,
        &BaseInstructions {
            text: "b".repeat(900_000 * 4),
            ..Default::default()
        },
    );
    assert_eq!(result, (0, 0));
    assert_eq!(history.annotated_items(), &[item]);
}
