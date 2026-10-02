use codex_notebook::NotebookControlResult;
use codex_notebook::NotebookRequest;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::formatted_truncate_text;
use serde_json::Value;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub(crate) struct NotebookHandler;

impl ToolExecutor<ToolInvocation> for NotebookHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("notebook")
    }

    fn spec(&self) -> ToolSpec {
        super::notebook_spec::create_notebook_tool()
    }

    fn exposure(&self) -> ToolExposure {
        // Lifecycle mutations must stay available outside exec in CodeModeOnly.
        ToolExposure::DirectModelOnly
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "notebook expects JSON arguments".to_string(),
                ));
            };
            let request: NotebookRequest = serde_json::from_str(arguments).map_err(|error| {
                FunctionCallError::RespondToModel(format!("Invalid notebook request: {error}"))
            })?;
            if matches!(invocation.source, ToolCallSource::CodeMode { .. }) {
                validate_nested_request(&request).map_err(FunctionCallError::RespondToModel)?;
            }
            let response = invocation
                .session
                .services
                .code_mode_service
                .control_notebook(request, &invocation.step_context)
                .await
                .map_err(FunctionCallError::RespondToModel)?;
            Ok(boxed_tool_output(NotebookOutput(response)))
        })
    }
}

impl CoreToolRuntime for NotebookHandler {}

fn validate_nested_request(request: &NotebookRequest) -> Result<(), String> {
    if matches!(
        request,
        NotebookRequest::Status { query: None }
            | NotebookRequest::List { .. }
            | NotebookRequest::Diagnostics
    ) {
        return Ok(());
    }
    // Never enqueue a kernel mutation while the active cell awaits its tool bridge.
    Err("This notebook action requires the active exec cell to finish. Call the top-level notebook tool after exec returns. Inside exec use status without query, list, or diagnostics".to_string())
}

struct NotebookOutput(NotebookControlResult);

impl NotebookOutput {
    fn render(&self) -> String {
        formatted_truncate_text(&self.0.message, TruncationPolicy::Bytes(32 * 1024))
    }
}

impl ToolOutput for NotebookOutput {
    fn log_output(&self) -> String {
        self.render()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(self.render(), Some(true)).to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        serde_json::json!({ "message": self.0.message, "details": self.0.details })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notebook_output_forwards_authoritative_message_and_keeps_structured_js_details() {
        let output = NotebookOutput(NotebookControlResult {
            message: "Notebook result from its producer".to_string(),
            details: json!({"internalMarker":"not-appended"}),
        });
        let response = output.to_response_item(
            "status",
            &ToolPayload::Function {
                arguments: "{}".to_string(),
            },
        );
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("Notebook result from its producer"));
        assert!(!serialized.contains("not-appended"));
        assert_eq!(
            output.code_mode_result(&ToolPayload::Function {
                arguments: "{}".into()
            })["details"]["internalMarker"],
            "not-appended"
        );
        let large = NotebookOutput(NotebookControlResult {
            message: "x".repeat(64 * 1024),
            details: json!({}),
        });
        let rendered = large.render();
        assert!(rendered.len() < 34 * 1024);
        assert!(rendered.contains("truncated"));
    }

    #[test]
    fn nested_control_rejects_queue_dependent_actions() {
        for input in [
            json!({"action": "status", "query": "*"}),
            json!({"action": "checkpoint"}),
            json!({"action": "restart"}),
            json!({"action": "reset"}),
            json!({"action": "save", "name": "profile"}),
            json!({"action": "load", "name": "profile"}),
            json!({"action": "pin", "names": ["binding"]}),
            json!({"action": "unpin", "names": ["binding"]}),
            json!({"action": "release", "names": ["binding"]}),
            json!({"action": "prune", "query": "*"}),
        ] {
            let request = serde_json::from_value(input).expect("valid request");
            assert!(validate_nested_request(&request).is_err());
        }
        for input in [
            json!({"action": "status"}),
            json!({"action": "list", "query": "*"}),
            json!({"action": "diagnostics"}),
        ] {
            let request = serde_json::from_value(input).expect("valid request");
            assert!(validate_nested_request(&request).is_ok());
        }
    }
}
