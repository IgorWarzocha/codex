//! Exec's model-only relay uses function outputs alongside custom receipts.

use super::ContextManager;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::default_input_modalities;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;

fn call(id: &str, name: &str, namespace: Option<&str>) -> ResponseItem {
    ResponseItem::CustomToolCall {
        id: None,
        status: None,
        call_id: id.to_string(),
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        input: "await tools.notes__read_file({path: 'note'})".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn relay(id: &str, nested_id: &str, encrypted: bool) -> ResponseItem {
    let mut items = vec![FunctionCallOutputContentItem::InputText {
        text: format!("Nested tool notes.read_file, call_id {nested_id}: model-only output"),
    }];
    if encrypted {
        items.push(FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: format!("opaque-{nested_id}"),
        });
    }
    ResponseInputItem::FunctionCallOutput {
        call_id: id.to_string(),
        output: FunctionCallOutputPayload::from_content_items(items),
    }
    .into()
}

fn receipt(id: &str, text: &str) -> ResponseItem {
    ResponseInputItem::CustomToolCallOutput {
        call_id: id.to_string(),
        name: Some("exec".to_string()),
        output: FunctionCallOutputPayload::from_text(text.to_string()),
    }
    .into()
}

fn history(items: &[ResponseItem]) -> ContextManager {
    let mut history = ContextManager::new();
    history.record_items(items.iter(), TruncationPolicy::Tokens(10_000));
    history
}

#[test_case::test_case(None; "unnamespaced exec")]
#[test_case::test_case(Some("functions"); "default namespace")]
#[test_case::test_case(Some(""); "legacy empty namespace")]
fn prompt_normalization_keeps_parallel_encrypted_relays_and_receipt(namespace: Option<&str>) {
    let items = vec![
        call("outer-exec", "exec", namespace),
        relay("outer-exec", "nested-one", true),
        relay("outer-exec", "nested-two", true),
        receipt("outer-exec", r#"{"delivered_to_model":true}"#),
    ];
    let h = history(&items);
    assert_eq!(h.clone().for_prompt(&default_input_modalities()), items);
    // Prompt normalization is repeatable and does not consume relay state.
    assert_eq!(h.for_prompt(&default_input_modalities()), items);
}

#[test_case::test_case("other", None, true; "not exec")]
#[test_case::test_case("exec", Some("editor"), true; "not default namespace")]
#[test_case::test_case("exec", None, false; "not encrypted")]
fn normalization_rejects_unproven_custom_function_output_pairs(
    name: &str,
    namespace: Option<&str>,
    encrypted: bool,
) {
    let items = vec![
        call("custom-call", name, namespace),
        relay("custom-call", "nested", encrypted),
        receipt("custom-call", "done"),
    ];
    let h = history(&items);
    let normalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.for_prompt(&default_input_modalities())
    }));
    if cfg!(debug_assertions) {
        let panic = normalized.expect_err("invalid cross-kind pairing must panic in debug");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied());
        assert_eq!(
            message,
            Some("Orphan function call output for call id: custom-call")
        );
    } else {
        assert_eq!(
            normalized.unwrap(),
            vec![items[0].clone(), items[2].clone()]
        );
    }
}

#[test]
fn trimming_any_exec_group_member_removes_all_outputs_but_keeps_other_exec() {
    let group = vec![
        call("outer-exec", "exec", None),
        relay("outer-exec", "nested-one", true),
        receipt("outer-exec", "notify"),
        relay("outer-exec", "nested-two", true),
        receipt("outer-exec", "receipt"),
    ];
    let other = vec![
        call("other-exec", "exec", None),
        relay("other-exec", "other-nested", true),
        receipt("other-exec", "receipt"),
    ];
    for index in 0..group.len() {
        let mut items = group.clone();
        let first = items.remove(index);
        items.insert(0, first);
        items.extend(other.clone());
        let mut h = history(&items);
        h.remove_first_item();
        assert_eq!(
            h.raw_items().cloned().collect::<Vec<_>>(),
            other,
            "removed group member {index}"
        );
        assert_eq!(h.for_prompt(&default_input_modalities()), other);
    }
}
