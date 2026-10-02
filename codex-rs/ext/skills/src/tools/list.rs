use std::collections::BTreeSet;

use codex_extension_api::FunctionCallError;
use codex_protocol::protocol::SkillScope;

use crate::catalog::SkillCatalogEntry;

pub(super) fn category(entry: &SkillCatalogEntry) -> Option<String> {
    if entry.prompt_scope() == Some(SkillScope::Repo) {
        return Some("session".to_string());
    }
    let relative = entry
        .rendered_path()
        .strip_prefix(entry.alias_root()?.trim_end_matches('/'))?
        .strip_prefix('/')?;
    let parts = relative.split('/').collect::<Vec<_>>();
    (parts.len() > 2).then(|| parts[0].to_string())
}

pub(super) fn format_list(
    entries: &[SkillCatalogEntry],
    categories: &[&str],
) -> Result<String, FunctionCallError> {
    let available = entries.iter().filter_map(category).collect::<BTreeSet<_>>();
    let unknown = categories
        .iter()
        .filter(|value| !available.contains(**value))
        .copied()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "Unknown categories: {}. Available: {}",
            unknown.join(", "),
            available.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let mut skills = entries
        .iter()
        .filter_map(|entry| {
            let category = category(entry);
            (categories.is_empty()
                || category
                    .as_deref()
                    .is_some_and(|category| categories.contains(&category)))
            .then_some((category, entry))
        })
        .collect::<Vec<_>>();
    let rank = |category: &Option<String>| match category.as_deref() {
        None => 0,
        Some("session") => 1,
        Some(_) => 2,
    };
    skills.sort_by(|(left_category, left), (right_category, right)| {
        (rank(left_category), left_category, &left.name).cmp(&(
            rank(right_category),
            right_category,
            &right.name,
        ))
    });
    if skills.is_empty() {
        return Ok("No skills available.".to_string());
    }
    let mut lines = Vec::new();
    let mut current_category = None;
    for (category, entry) in skills {
        if category != current_category {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            if let Some(category) = &category {
                lines.push(format!("# {}", category.replace('-', " ").to_uppercase()));
            }
            current_category = category;
        }
        lines.push(format!(
            "- {}: {}",
            entry.name,
            entry
                .description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    Ok(lines.join("\n"))
}
