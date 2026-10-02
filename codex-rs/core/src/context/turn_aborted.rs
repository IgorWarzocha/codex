use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnAborted {
    pub(crate) guidance: String,
}

impl TurnAborted {
    pub(crate) const INTERRUPTED_GUIDANCE: &'static str = "Previous turn deliberately interrupted by the user. Unified exec processes may still be running. Aborted tools or commands may have partially executed";
    pub(crate) const INTERRUPTED_DEVELOPER_GUIDANCE: &'static str = "Previous turn deliberately interrupted. Unified exec processes may still be running. Aborted tools or commands may have partially executed";

    pub(crate) fn new(guidance: impl Into<String>) -> Self {
        Self {
            guidance: guidance.into(),
        }
    }
}

impl ContextualUserFragment for TurnAborted {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("generic.turn_aborted".to_string())
    }

    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<turn_aborted>", "</turn_aborted>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.guidance)
    }
}
