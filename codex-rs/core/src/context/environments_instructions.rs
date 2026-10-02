use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_OPEN_TAG;

use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

pub(crate) struct EnvironmentsInstructions;

impl ContextualUserFragment for EnvironmentsInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("environments.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            ENVIRONMENTS_INSTRUCTIONS_OPEN_TAG,
            ENVIRONMENTS_INSTRUCTIONS_CLOSE_TAG,
        )
    }

    fn body(&self) -> String {
        "\n## Execution environments\n\
Separate machines or workspaces, each with its own files, shell, and installed capabilities. Task selection: `<environment_context>`\n\
\n\
`starting`: not yet usable. Files, commands, AGENTS.md instructions, skills, plugins, and MCP tools may become available after startup\n\
\n\
Wait only for environments needed by the current task. Available tools for unrelated work meanwhile\n"
            .to_string()
    }
}
