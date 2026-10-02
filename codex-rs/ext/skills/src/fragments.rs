use codex_extension_api::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

use crate::tools::SkillToolAuthority;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillInstructions {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) contents: String,
    pub(crate) resource_access: Option<SkillResourceAccess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillResourceAccess {
    pub(crate) authority: SkillToolAuthority,
    pub(crate) package: String,
    pub(crate) main_resource: String,
}

impl ContextualUserFragment for SkillInstructions {
    fn role(&self) -> &'static str {
        "user"
    }

    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("skills.selected_skill_instructions".to_string())
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<skill>", "</skill>")
    }

    fn body(&self) -> String {
        let name = &self.name;
        let path = &self.path;
        let contents = &self.contents;
        let resource_access = self
            .resource_access
            .as_ref()
            .map(|access| {
                let metadata = serde_json::json!({
                    "authority": access.authority,
                    "package": access.package,
                    "main_resource": access.main_resource,
                });
                format!("\n<resource_access>{metadata}</resource_access>")
            })
            .unwrap_or_default();
        format!("\n<name>{name}</name>\n<path>{path}</path>{resource_access}\n{contents}\n")
    }
}
