use std::borrow::Borrow;

use crate::context::ContextualUserFragment;
use crate::context::ImageResizeNotice;
use crate::context_manager::ContextManager;
use crate::context_manager::estimate_item_token_count;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_utils_output_truncation::approx_token_count;

const CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE: &str =
    "Output exceeded the available model context and was truncated";

// The compaction backend accepts this independently of the active user context window.
pub(crate) const COMPACTION_INPUT_TOKEN_BUDGET: i64 = 872_000;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HistoryItemGroup<T> {
    pub(crate) source: T,
    pub(crate) attached_notice: Option<T>,
}

impl<T: Borrow<ResponseItem>> HistoryItemGroup<T> {
    pub(crate) fn into_items(self) -> impl Iterator<Item = T> {
        std::iter::once(self.source).chain(self.attached_notice)
    }

    pub(crate) fn estimated_token_count(&self) -> i128 {
        let source_tokens = i128::from(estimate_item_token_count(self.source.borrow()));
        let notice_tokens = self.attached_notice.as_ref().map_or(0, |notice| {
            i128::from(estimate_item_token_count(notice.borrow()))
        });
        source_tokens.saturating_add(notice_tokens)
    }
}

pub(crate) fn history_item_groups<I>(items: I) -> impl Iterator<Item = HistoryItemGroup<I::Item>>
where
    I: IntoIterator,
    I::Item: Borrow<ResponseItem>,
{
    let mut items = items.into_iter().peekable();
    std::iter::from_fn(move || {
        let source = items.next()?;
        let attached_notice = items.next_if(|notice| is_attached_notice(notice.borrow()));
        Some(HistoryItemGroup {
            source,
            attached_notice,
        })
    })
}

fn is_attached_notice(notice: &ResponseItem) -> bool {
    matches!(
        notice,
        ResponseItem::Message { role, content, .. }
            if role == "developer"
                && matches!(
                    content.as_slice(),
                    [ContentItem::InputText { text }]
                        if ImageResizeNotice::matches_text(text)
                )
    )
}

pub(crate) fn trim_function_call_history_to_fit_compaction_budget(
    history: &mut ContextManager,
    base_instructions: &BaseInstructions,
) -> (usize, i64) {
    // Keep the unclamped total so replacing an item cannot lose an overflow hidden by i64
    // saturation in the normal history estimator.
    let base_tokens =
        i128::try_from(approx_token_count(&base_instructions.text)).unwrap_or(i128::MAX);
    let original_items = history.annotated_items();
    let mut estimated_tokens = history_item_groups(original_items.iter().map(|item| &item.item))
        .map(|group| group.estimated_token_count())
        .fold(base_tokens, i128::saturating_add);
    let initial_estimated_tokens = i64::try_from(estimated_tokens).unwrap_or(i64::MAX);
    if estimated_tokens <= i128::from(COMPACTION_INPUT_TOKEN_BUDGET) {
        return (0, 0);
    }
    let mut rewritten_items = original_items.to_vec();
    let mut rewritten_outputs = 0;
    let mut consumed_items: usize = 0;

    for group in history_item_groups(original_items.iter().map(|item| &item.item))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        if estimated_tokens <= i128::from(COMPACTION_INPUT_TOKEN_BUDGET) {
            break;
        }
        let group_item_count = 1 + usize::from(group.attached_notice.is_some());
        let source_index = original_items
            .len()
            .saturating_sub(consumed_items.saturating_add(group_item_count));
        // Only shrink the trailing output lane. Encrypted history/notes are part of
        // that lane, but must be kept intact while scanning preceding outputs.
        if !matches!(
            group.source,
            ResponseItem::FunctionCallOutput { .. }
                | ResponseItem::CustomToolCallOutput { .. }
                | ResponseItem::ToolSearchOutput { .. }
        ) {
            break;
        }
        consumed_items += group_item_count;
        let Some(rewritten_item) = original_items
            .get(source_index)
            .and_then(rewritten_output_for_context_window)
        else {
            continue;
        };
        let replacement_tokens = i128::from(estimate_item_token_count(&rewritten_item.item));
        if replacement_tokens >= group.estimated_token_count() {
            continue;
        }
        estimated_tokens = estimated_tokens
            .saturating_sub(group.estimated_token_count())
            .saturating_add(replacement_tokens);
        rewritten_items.splice(
            source_index..source_index + group_item_count,
            [rewritten_item],
        );
        rewritten_outputs += 1;
    }

    if rewritten_outputs > 0 {
        history.replace_annotated(rewritten_items);
    }

    let final_estimated_tokens = i64::try_from(estimated_tokens).unwrap_or(i64::MAX);
    let estimated_deleted_tokens = initial_estimated_tokens.saturating_sub(final_estimated_tokens);
    (rewritten_outputs, estimated_deleted_tokens)
}

fn rewritten_output_for_context_window(
    envelope: &ResponseItemEnvelope,
) -> Option<ResponseItemEnvelope> {
    let item = match &envelope.item {
        ResponseItem::FunctionCallOutput {
            id,
            call_id,
            name,
            namespace,
            output,
            internal_chat_message_metadata_passthrough: metadata,
        } if !has_encrypted_content(output) => ResponseItem::FunctionCallOutput {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
            namespace: namespace.clone(),
            output: truncated_output_payload(output),
            internal_chat_message_metadata_passthrough: metadata.clone(),
        },
        ResponseItem::CustomToolCallOutput {
            id,
            call_id,
            name,
            output,
            internal_chat_message_metadata_passthrough: metadata,
        } if !has_encrypted_content(output) => ResponseItem::CustomToolCallOutput {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
            output: truncated_output_payload(output),
            internal_chat_message_metadata_passthrough: metadata.clone(),
        },
        ResponseItem::ToolSearchOutput {
            id,
            call_id,
            status,
            execution,
            internal_chat_message_metadata_passthrough: metadata,
            ..
        } => ResponseItem::ToolSearchOutput {
            id: id.clone(),
            call_id: call_id.clone(),
            status: status.clone(),
            execution: execution.clone(),
            tools: Vec::new(),
            internal_chat_message_metadata_passthrough: metadata.clone(),
        },
        _ => return None,
    };
    Some(ResponseItemEnvelope {
        item,
        metadata: envelope.metadata.clone(),
    })
}

fn has_encrypted_content(output: &FunctionCallOutputPayload) -> bool {
    output.content_items().is_some_and(|items| {
        items
            .iter()
            .any(|item| matches!(item, FunctionCallOutputContentItem::EncryptedContent { .. }))
    })
}

fn truncated_output_payload(output: &FunctionCallOutputPayload) -> FunctionCallOutputPayload {
    FunctionCallOutputPayload {
        body: FunctionCallOutputBody::Text(CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE.to_string()),
        success: output.success,
    }
}

#[cfg(test)]
#[path = "compact_remote_history_tests.rs"]
mod metadata_tests;
