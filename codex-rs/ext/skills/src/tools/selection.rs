use std::collections::HashSet;

use codex_extension_api::FunctionCallError;

use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;

#[derive(Clone)]
pub(super) struct PackageFile {
    pub relative: String,
    pub resource: SkillResourceId,
    pub source: String,
}

pub(super) struct SkillPackage {
    pub entry: SkillCatalogEntry,
    pub files: Vec<PackageFile>,
}

#[derive(Clone)]
pub(super) struct Selection {
    pub skill: usize,
    pub resource: SkillResourceId,
    pub reference: Option<String>,
}

fn error(message: String) -> FunctionCallError {
    FunctionCallError::RespondToModel(message)
}

fn markdown_stem(value: &str) -> &str {
    if value
        .get(value.len().saturating_sub(3)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
    {
        &value[..value.len() - 3]
    } else {
        value
    }
}

fn main_selection(skill: usize, package: &SkillPackage) -> Selection {
    Selection {
        skill,
        resource: package.entry.main_prompt.clone(),
        reference: None,
    }
}

fn find_reference(skill: usize, package: &SkillPackage, name: &str) -> Option<Selection> {
    let normalized = name.replace('\\', "/");
    let mut candidate = normalized.as_str();
    for prefix in [
        "./references/".to_string(),
        "references/".to_string(),
        format!("{}/references/", package.entry.name),
    ] {
        if candidate
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        {
            candidate = &candidate[prefix.len()..];
        }
    }
    package.files.iter().find_map(|file| {
        let reference = file.relative.strip_prefix("references/")?;
        if !reference.to_ascii_lowercase().ends_with(".md") {
            return None;
        }
        (markdown_stem(reference) == markdown_stem(candidate)
            || name == file.source
            || name == file.resource.as_str())
        .then(|| Selection {
            skill,
            resource: file.resource.clone(),
            reference: Some(markdown_stem(reference).to_string()),
        })
    })
}

fn explicit(packages: &[SkillPackage], name: &str) -> Result<Option<Selection>, FunctionCallError> {
    let native_matches = packages
        .iter()
        .enumerate()
        .flat_map(|(index, package)| {
            package.files.iter().filter_map(move |file| {
                file.resource
                    .environment_path()
                    .filter(|(_, path)| path.inferred_native_path_string() == name)
                    .map(|_| (index, file))
            })
        })
        .collect::<Vec<_>>();
    if native_matches.len() > 1 {
        return Err(error(format!(
            "Ambiguous source path \"{name}\". Use one of: {}",
            native_matches
                .iter()
                .map(|(_, file)| file.resource.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if let Some((skill, file)) = native_matches.first() {
        return Ok(Some(Selection {
            skill: *skill,
            resource: file.resource.clone(),
            reference: (file.resource != packages[*skill].entry.main_prompt).then(|| {
                file.relative
                    .strip_prefix("references/")
                    .unwrap_or(&file.relative)
                    .to_string()
            }),
        }));
    }
    let normalized = name.replace('\\', "/");
    for (index, package) in packages.iter().enumerate() {
        if normalized.eq_ignore_ascii_case(&format!("{}/SKILL.md", package.entry.name))
            || name == package.entry.id.0
            || name == package.entry.main_prompt.as_str()
            || name == package.entry.rendered_path()
        {
            return Ok(Some(main_selection(index, package)));
        }
        let prefix = format!("{}/references/", package.entry.name);
        if normalized
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        {
            if let Some(reference) = find_reference(index, package, &normalized) {
                return Ok(Some(reference));
            }
            // Remote packages may not expose an inventory. Reads still go through
            // their provider and its package validation, never through a local path.
            if package.entry.authority.kind == SkillSourceKind::Cloud {
                let relative = format!("references/{}", &normalized[prefix.len()..]);
                return remote_selection(index, package, &relative).map(Some);
            }
            return Err(error(format!(
                "Unknown reference \"{name}\" for skill \"{}\". Use \"read {}\" to inspect reference paths",
                package.entry.name, package.entry.name
            )));
        }
        if let Some(reference) = find_reference(index, package, name)
            && (name == reference.resource.as_str()
                || package.files.iter().any(|file| file.source == name))
        {
            return Ok(Some(reference));
        }
        if let Some(file) = package
            .files
            .iter()
            .find(|file| file.resource.as_str() == name || file.source == name)
        {
            return Ok(Some(Selection {
                skill: index,
                resource: file.resource.clone(),
                reference: Some(
                    file.relative
                        .strip_prefix("references/")
                        .unwrap_or(&file.relative)
                        .to_string(),
                ),
            }));
        }
        if package.entry.authority.kind == SkillSourceKind::Executor
            && package.entry.id.relative_resource_path(name).is_some()
        {
            let resource = package
                .entry
                .main_prompt
                .bind_environment_package_resource(&package.entry.id, name)
                .ok_or_else(|| error("Resource must be inside the skill package".to_string()))?;
            let reference = package
                .entry
                .id
                .relative_resource_path(name)
                .unwrap_or_default()
                .to_string();
            return Ok(Some(Selection {
                skill: index,
                resource,
                reference: Some(reference),
            }));
        }
        if package.entry.authority.kind == SkillSourceKind::Cloud
            && let Some(relative) = package.entry.id.relative_resource_path(name)
        {
            return remote_selection(index, package, relative).map(Some);
        }
    }
    Ok(None)
}

fn remote_selection(
    skill: usize,
    package: &SkillPackage,
    relative: &str,
) -> Result<Selection, FunctionCallError> {
    let resource = format!("{}/{relative}", package.entry.id.0.trim_end_matches('/'));
    if package.entry.id.relative_resource_path(&resource).is_none() {
        return Err(error(
            "Resource must be inside the skill package".to_string(),
        ));
    }
    Ok(Selection {
        skill,
        resource: SkillResourceId::new(resource),
        reference: Some(relative.to_string()),
    })
}

fn additional(packages: &[SkillPackage], name: &str) -> Result<Selection, FunctionCallError> {
    if let Some((index, package)) = packages
        .iter()
        .enumerate()
        .find(|(_, package)| package.entry.name == name || package.entry.id.0 == name)
    {
        return Ok(main_selection(index, package));
    }
    if let Some(selection) = explicit(packages, name)? {
        return Ok(selection);
    }
    let references = packages
        .iter()
        .enumerate()
        .filter_map(|(index, package)| find_reference(index, package, name))
        .collect::<Vec<_>>();
    match references.len() {
        1 => Ok(references[0].clone()),
        0 => Err(error(format!(
            "Unknown skill or reference \"{name}\". Use \"list\" for skill names or \"read <skill>\" to inspect reference paths"
        ))),
        _ => Err(error(format!(
            "Ambiguous reference \"{name}\". Use one of: {}",
            references
                .iter()
                .map(|reference| format!(
                    "{}/references/{}",
                    packages[reference.skill].entry.name,
                    reference.reference.as_deref().unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(super) fn select(
    packages: &[SkillPackage],
    names: &[&str],
) -> Result<Vec<Selection>, FunctionCallError> {
    let primary_name = names[0];
    let primary = if let Some((index, package)) =
        packages.iter().enumerate().find(|(_, package)| {
            package.entry.name == primary_name || package.entry.name == markdown_stem(primary_name)
        }) {
        main_selection(index, package)
    } else {
        explicit(packages, primary_name)?.ok_or_else(|| {
            error(format!(
                "Unknown skill \"{primary_name}\". Available: {}",
                packages
                    .iter()
                    .map(|package| package.entry.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?
    };
    let package = &packages[primary.skill];
    let mut selections = names[1..]
        .iter()
        .filter(|name| {
            !name.eq_ignore_ascii_case("SKILL.md")
                && !name.eq_ignore_ascii_case(&format!("{}/SKILL.md", package.entry.name))
                && **name != package.entry.main_prompt.as_str()
        })
        .map(|name| additional(packages, name))
        .collect::<Result<Vec<_>, _>>()?;
    // References-only reads deliberately omit the primary body and its inventory.
    if primary.reference.is_some()
        || selections.is_empty()
        || selections
            .iter()
            .any(|selection| selection.reference.is_none())
    {
        selections.insert(0, primary);
    }
    let mut seen = HashSet::new();
    selections.retain(|selection| {
        seen.insert((
            packages[selection.skill].entry.authority.clone(),
            selection.resource.clone(),
        ))
    });
    Ok(selections)
}
