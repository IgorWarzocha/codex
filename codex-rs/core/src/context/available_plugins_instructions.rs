use codex_protocol::protocol::PLUGINS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::PLUGINS_INSTRUCTIONS_OPEN_TAG;

use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AvailablePluginsInstructions;

impl ContextualUserFragment for AvailablePluginsInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("plugins.usage_instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            PLUGINS_INSTRUCTIONS_OPEN_TAG,
            PLUGINS_INSTRUCTIONS_CLOSE_TAG,
        )
    }

    fn body(&self) -> String {
        "\nPlugins are not called directly. Use their exposed skills, MCP tools, and apps. Prefer a named plugin's capabilities when relevant. If unavailable, say so and use an available alternative. Plugin skills have a `plugin_name:` prefix. MCP provenance identifies plugin tools.\n".to_string()
    }
}
