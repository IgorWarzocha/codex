use codex_extension_api::FunctionCallError;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolCallSource;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolPayload;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseInputItem;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolExposure;
use serde_json::Value;
use serde_json::json;

use crate::backend::HistoryNotesBackend;

const HISTORY_NAMESPACE: &str = "history";
const NOTES_NAMESPACE: &str = "notes";
const HISTORY_DESCRIPTION: &str = "Private conversation recovery; returned window/item IDs unchanged; new items may lag; no private state in user-facing output";
const NOTES_DESCRIPTION: &str = "Private remote notes across windows; relative paths use current agent, absolute <agent>/notes/<path>; no shell expansion or empty, '.' or '..' components; 1,000,000 UTF-8 bytes/file; reads reflect completed writes, search/list may lag; await dependent same-path writes; no private state in user-facing output";
const HISTORY_AGENT_NAME_DESCRIPTION: &str =
    "Omit for current agent; absolute or relative agent name";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryNotesAction {
    HistoryListWindows,
    HistoryListItems,
    HistoryReadItem,
    HistorySearchContents,
    NotesListFilesByPrefix,
    NotesReadFile,
    NotesSearchContents,
    NotesAppendToFile,
    NotesWriteFile,
}

impl HistoryNotesAction {
    pub(crate) const ALL: [Self; 9] = [
        Self::HistoryListWindows,
        Self::HistoryListItems,
        Self::HistoryReadItem,
        Self::HistorySearchContents,
        Self::NotesListFilesByPrefix,
        Self::NotesReadFile,
        Self::NotesSearchContents,
        Self::NotesAppendToFile,
        Self::NotesWriteFile,
    ];

    fn namespace(self) -> &'static str {
        match self {
            Self::HistoryListWindows
            | Self::HistoryListItems
            | Self::HistoryReadItem
            | Self::HistorySearchContents => HISTORY_NAMESPACE,
            Self::NotesListFilesByPrefix
            | Self::NotesReadFile
            | Self::NotesSearchContents
            | Self::NotesAppendToFile
            | Self::NotesWriteFile => NOTES_NAMESPACE,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::HistoryListWindows => "list_windows",
            Self::HistoryListItems => "list_items",
            Self::HistoryReadItem => "read_item",
            Self::HistorySearchContents => "search_contents",
            Self::NotesListFilesByPrefix => "list_files_by_prefix",
            Self::NotesReadFile => "read_file",
            Self::NotesSearchContents => "search_contents",
            Self::NotesAppendToFile => "append_to_file",
            Self::NotesWriteFile => "write_file",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::HistoryListWindows => "alpha/history/v2/list_windows",
            Self::HistoryListItems => "alpha/history/v2/list_items",
            Self::HistoryReadItem => "alpha/history/v2/read_item",
            Self::HistorySearchContents => "alpha/history/v2/search_contents",
            Self::NotesListFilesByPrefix => "alpha/notes/v2/list_files_by_prefix",
            Self::NotesReadFile => "alpha/notes/v2/read_file",
            Self::NotesSearchContents => "alpha/notes/v2/search_contents",
            Self::NotesAppendToFile => "alpha/notes/v2/append_to_file",
            Self::NotesWriteFile => "alpha/notes/v2/write_file",
        }
    }

    fn namespace_description(self) -> &'static str {
        match self.namespace() {
            HISTORY_NAMESPACE => HISTORY_DESCRIPTION,
            NOTES_NAMESPACE => NOTES_DESCRIPTION,
            _ => unreachable!("History actions use a known namespace"),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::HistoryListWindows => "Context window IDs and item counts",
            Self::HistoryListItems => "Filtered history items",
            Self::HistoryReadItem => "Read a history item range",
            Self::HistorySearchContents => "Search history by literal substring",
            Self::NotesListFilesByPrefix => "List note paths",
            Self::NotesReadFile => "Read note lines",
            Self::NotesSearchContents => "Search note lines by literal substring",
            Self::NotesAppendToFile => "Append text exactly",
            Self::NotesWriteFile => "Create or replace a note",
        }
    }

    fn parameters(self) -> Value {
        match self {
            Self::HistoryListWindows => json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1},
                    "agent_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": HISTORY_AGENT_NAME_DESCRIPTION},
                    "recent_first": {"type": "boolean"}
                }
            }),
            Self::HistoryListItems => json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1},
                    "recent_first": {"type": "boolean"},
                    "tool_namespace": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Excludes non-tool messages"},
                    "role": {"anyOf": [{"type": "string", "enum": ["user", "assistant", "tool", "system", "developer"]}, {"type": "null"}]},
                    "agent_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": HISTORY_AGENT_NAME_DESCRIPTION},
                    "tool_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Excludes non-tool messages"},
                    "window_id": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Full returned ID; omit for all windows"},
                    "max_chars_per_item": {"type": "integer", "minimum": 1}
                }
            }),
            Self::HistoryReadItem => json!({
                "type": "object",
                "properties": {
                    "agent_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": HISTORY_AGENT_NAME_DESCRIPTION},
                    "item_id": {"type": "string", "description": "Suffix from the [id: …] marker"},
                    "offset_chars": {"type": "integer", "minimum": 0, "description": "Zero-based"},
                    "limit_chars": {"type": "integer", "minimum": 1},
                    "window_id": {"type": "string", "description": "Full returned window ID"}
                },
                "required": ["item_id", "window_id"]
            }),
            Self::HistorySearchContents => json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1},
                    "query": {"type": "string", "encrypted": true, "description": "Case-sensitive literal substring"},
                    "recent_first": {"type": "boolean"},
                    "tool_namespace": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Excludes non-tool messages"},
                    "role": {"anyOf": [{"type": "string", "enum": ["user", "assistant", "tool", "system", "developer"]}, {"type": "null"}]},
                    "agent_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": HISTORY_AGENT_NAME_DESCRIPTION},
                    "tool_name": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Excludes non-tool messages"},
                    "window_id": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Full returned ID; omit for all windows"}
                },
                "required": ["query"]
            }),
            Self::NotesListFilesByPrefix => json!({
                "type": "object",
                "properties": {
                    "prefix": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "max_results": {"type": "integer", "minimum": 1},
                    "file_order_by": {"type": "string", "enum": ["name", "created_at", "updated_at"]},
                    "file_order": {"type": "string", "enum": ["ascending", "descending"]}
                }
            }),
            Self::NotesReadFile => json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "start_line": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Inclusive, 1-based; negative counts from end"},
                    "stop_line": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Inclusive, 1-based; negative counts from end"}
                },
                "required": ["path"]
            }),
            Self::NotesSearchContents => json!({
                "type": "object",
                "properties": {
                    "max_matches_per_file": {"type": "integer", "minimum": 1},
                    "query": {"type": "string", "encrypted": true, "description": "Case-sensitive literal substring"},
                    "recent_file_first": {"type": "boolean", "description": "By creation time"},
                    "max_files": {"type": "integer", "minimum": 1},
                    "path_prefix": {"anyOf": [{"type": "string"}, {"type": "null"}]}
                },
                "required": ["query"]
            }),
            Self::NotesAppendToFile => json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "encrypted": true},
                    "path": {"type": "string"}
                },
                "required": ["text", "path"]
            }),
            Self::NotesWriteFile => json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "encrypted": true},
                    "path": {"type": "string"}
                },
                "required": ["text", "path"]
            }),
        }
    }
}

pub(crate) struct HistoryNotesTool {
    action: HistoryNotesAction,
    backend: HistoryNotesBackend,
    session_id: String,
    current_agent_name: String,
}

impl HistoryNotesTool {
    pub(crate) fn new(
        action: HistoryNotesAction,
        backend: HistoryNotesBackend,
        session_id: String,
        current_agent_name: String,
    ) -> Self {
        Self {
            action,
            backend,
            session_id,
            current_agent_name,
        }
    }

    async fn handle_call(
        &self,
        call: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let arguments = call.function_arguments()?;
        let arguments = if arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments)
                .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?
        };
        let result = self
            .backend
            .call(
                self.action.endpoint(),
                &self.session_id,
                &self.current_agent_name,
                arguments,
                call.truncation_policy,
                matches!(call.source, ToolCallSource::Direct),
            )
            .await
            .map_err(FunctionCallError::RespondToModel)?;

        Ok(Box::new(HistoryNotesToolOutput::new(result, call.call_id)?))
    }
}

impl<'call> ToolExecutor<ToolCall<'call>> for HistoryNotesTool {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(self.action.namespace(), self.action.name())
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: self.action.namespace().to_string(),
            description: self.action.namespace_description().to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: self.action.name().to_string(),
                description: self.action.description().to_string(),
                strict: false,
                parameters: parse_tool_input_schema(&self.action.parameters()).unwrap_or_else(
                    |error| panic!("History tool input schema should parse: {error}"),
                ),
                output_schema: None,
                defer_loading: None,
            })],
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, call: ToolCall<'call>) -> ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(self.handle_call(call))
    }
}

struct HistoryNotesToolOutput {
    result: Value,
    output: FunctionCallOutputPayload,
    call_id: String,
}

impl HistoryNotesToolOutput {
    fn new(mut result: Value, call_id: String) -> Result<Self, FunctionCallError> {
        // Separate attachments before serializing any text or retaining log output.
        let images = result.as_object_mut().and_then(|map| map.remove("images"));
        // The server applies the requested output budget before encryption.
        let mut output = match result.get("encrypted_output").and_then(Value::as_str) {
            Some(encrypted_content) => FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: encrypted_content.to_string(),
                },
            ]),
            None => FunctionCallOutputPayload::from_text(result.to_string()),
        };
        if let Some(images) = images {
            let invalid_image = || {
                FunctionCallError::RespondToModel(
                    "History backend returned invalid image content.".to_string(),
                )
            };
            let images = images.as_array().ok_or_else(invalid_image)?;
            let mut content = match output.body {
                FunctionCallOutputBody::Text(text) => {
                    vec![FunctionCallOutputContentItem::InputText { text }]
                }
                FunctionCallOutputBody::ContentItems(content) => content,
            };
            for image in images {
                let data = image
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_image)?;
                let mime_type = image
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_image)?;
                let detail =
                    serde_json::from_value(image.get("detail").cloned().unwrap_or(Value::Null))
                        .map_err(|_| invalid_image())?;
                content.push(FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: format!("data:{mime_type};base64,{data}"),
                    },
                    detail,
                });
            }
            output = FunctionCallOutputPayload::from_content_items(content);
        }
        Ok(Self {
            result,
            output,
            call_id,
        })
    }
}

impl ToolOutput for HistoryNotesToolOutput {
    fn log_output(&self) -> String {
        JsonToolOutput::new(self.result.clone()).log_output()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn post_tool_use_response(&self, _call_id: &str, _payload: &ToolPayload) -> Option<Value> {
        // Hooks must not receive model-only image attachments.
        Some(self.result.clone())
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: self.output.clone(),
        }
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        json!({"delivered_to_model": true, "call_id": self.call_id})
    }

    fn code_mode_model_output(&self, _payload: &ToolPayload) -> Option<FunctionCallOutputPayload> {
        Some(self.output.clone())
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
