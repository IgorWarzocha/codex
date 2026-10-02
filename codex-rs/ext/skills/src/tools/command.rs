use std::collections::HashSet;

use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::ToolPayload;
use serde_json::Value;

use crate::catalog::SkillCatalog;
use crate::warnings::bounded_warnings;

use super::SkillToolAuthoritySelector;
use super::SkillToolContext;
use super::list::format_list;

pub(super) const MAX_OUTPUT_BYTES: usize = 48 * 1024;

#[derive(Debug, PartialEq)]
enum Request<'a> {
    List(Vec<&'a str>),
    Read(Vec<&'a str>),
}

fn parse_request(command: &str) -> Result<Request<'_>, FunctionCallError> {
    if command.len() > 8192 {
        return Err(FunctionCallError::RespondToModel(
            "Skills command exceeds 8192 bytes".to_string(),
        ));
    }
    let mut parts = command.split_whitespace();
    let action = parts.next().unwrap_or("list");
    let mut arguments = parts.collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let start = usize::from(action == "read");
    if start <= arguments.len() {
        let additional = arguments
            .drain(start..)
            .filter(|value| seen.insert(*value))
            .collect::<Vec<_>>();
        arguments.extend(additional);
    }
    match action {
        "list" => Ok(Request::List(arguments)),
        "read" if !arguments.is_empty() => Ok(Request::Read(arguments)),
        _ => Err(FunctionCallError::RespondToModel(
            "Expected list [category...] or read <skill> [skill-or-reference...]".to_string(),
        )),
    }
}

pub(super) struct SkillsTool {
    pub(super) context: SkillToolContext,
}

impl SkillToolContext {
    async fn visible_catalog(&self, turn_id: &str) -> SkillCatalog {
        let mut catalog = SkillCatalog::default();
        // Name reads prefer remote/executor copies. Keep every enabled package
        // so a root-qualified locator still reaches its own source.
        for selector in [
            SkillToolAuthoritySelector::Cloud,
            SkillToolAuthoritySelector::Executor,
            SkillToolAuthoritySelector::Host,
        ] {
            catalog.extend(self.catalog(turn_id, selector).await);
        }
        catalog.entries.retain(|entry| entry.enabled);
        catalog
    }
}

impl<'call> ToolExecutor<ToolCall<'call>> for SkillsTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("skills")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Freeform(FreeformTool {
            name: "skills".to_string(),
            description: "Load skill instructions and references. list [category...] | read <skill> [skill-or-reference...]".to_string(),
            defer_loading: None,
            // The transport requires a format. This catchall leaves command
            // validation to the same parser used by Code Mode and Notebook.
            format: FreeformToolFormat { r#type: "grammar".to_string(), syntax: "lark".to_string(), definition: "start: command?\ncommand: /[\\s\\S]+/".to_string() },
        })
    }

    fn handle<'a>(&'a self, call: ToolCall<'call>) -> ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Custom { input } = &call.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "Skills expects a string command".to_string(),
                ));
            };
            let request = parse_request(input)?;
            let catalog = self.context.visible_catalog(&call.turn_id).await;
            let warnings = bounded_warnings(&catalog.warnings);
            let (mut text, external) = match request {
                Request::List(categories) => {
                    let mut names = HashSet::new();
                    let entries = catalog
                        .entries
                        .iter()
                        .filter(|entry| {
                            entry.is_model_visible() && names.insert(entry.name.clone())
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    (
                        format_list(&entries, &categories)?,
                        self.context.cloud_available,
                    )
                }
                Request::Read(names) => {
                    let entries = catalog
                        .entries
                        .into_iter()
                        .filter(|entry| {
                            entry.is_model_visible()
                                || entry.id.0 == names[0]
                                || entry.main_prompt.as_str() == names[0]
                                || entry.id.relative_resource_path(names[0]).is_some()
                        })
                        .collect();
                    self.context.read(entries, &names, &call).await?
                }
            };
            if !warnings.is_empty() {
                text.push_str(&format!("\n\nWarnings:\n{}", warnings.join("\n")));
            }
            let budget = MAX_OUTPUT_BYTES;
            if text.len() > budget {
                return Err(FunctionCallError::RespondToModel(format!(
                    "Skills output is {} bytes; maximum is {budget} bytes. List selected categories or read fewer skills or selected references",
                    text.len()
                )));
            }
            Ok(Box::new(SkillsOutput {
                text,
                external: external || self.context.cloud_available && !warnings.is_empty(),
            }) as Box<dyn ToolOutput>)
        })
    }
}

struct SkillsOutput {
    text: String,
    external: bool,
}

impl ToolOutput for SkillsOutput {
    fn log_output(&self) -> String {
        self.text.clone()
    }
    fn success_for_logging(&self) -> bool {
        true
    }
    fn contains_external_context(&self) -> bool {
        self.external
    }
    fn fallback_token_limit_override(&self) -> Option<usize> {
        // Selected instructions are atomic. The tool's own size check replaces
        // the general-purpose history truncation policy for this result.
        Some(self.text.len())
    }
    fn code_mode_result(&self, _payload: &ToolPayload) -> Value {
        Value::String(self.text.clone())
    }
    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        let output = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(self.text.clone()),
            success: Some(true),
        };
        if matches!(payload, ToolPayload::Custom { .. }) {
            ResponseInputItem::CustomToolCallOutput {
                call_id: call_id.to_string(),
                name: None,
                output,
            }
        } else {
            ResponseInputItem::FunctionCallOutput {
                call_id: call_id.to_string(),
                output,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_command_parser() {
        assert_eq!(parse_request("").unwrap(), Request::List(vec![]));
        assert_eq!(
            parse_request("list code session code").unwrap(),
            Request::List(vec!["code", "session"])
        );
        assert_eq!(
            parse_request("read communication testing code-review testing").unwrap(),
            Request::Read(vec!["communication", "testing", "code-review"])
        );
        assert!(parse_request("read").is_err());
        assert!(parse_request("unknown skill").is_err());
    }
}
