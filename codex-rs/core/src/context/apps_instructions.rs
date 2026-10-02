use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_protocol::protocol::APPS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::APPS_INSTRUCTIONS_OPEN_TAG;

use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AppsInstructions;

impl ContextualUserFragment for AppsInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("apps.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (APPS_INSTRUCTIONS_OPEN_TAG, APPS_INSTRUCTIONS_CLOSE_TAG)
    }

    fn body(&self) -> String {
        format!(
            "\nApps use `{CODEX_APPS_MCP_SERVER_NAME}` tools. A user can select an app with `[$app-name](app://{{connector_id}})`. Otherwise use relevant available apps. Discover unloaded tools with `tool_search` when available. Do not call `list_mcp_resources` or `list_mcp_resource_templates` for apps.\n"
        )
    }
}
