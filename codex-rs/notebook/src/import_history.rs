//! Project npm inventory is guidance, not an approval record or permission gate.
//! Like Pi, successful cells contribute exact-version static import literals.

mod source;
#[cfg(test)]
mod tests;

pub(crate) use source::extract;

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::storage::files;
use crate::storage::files::Paths;

pub(crate) const MAX_IMPORTS: usize = 1_000;
const MAX_SPECIFIER_BYTES: usize = 1_024;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_LIST_BYTES: usize = 12 * 1024;

#[derive(Clone)]
pub(crate) struct ImportHistory {
    paths: Paths,
}

#[derive(Deserialize, Serialize)]
struct Manifest {
    schema: u32,
    project: PathBuf,
    imports: BTreeSet<String>,
}

impl ImportHistory {
    pub(crate) fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub(crate) async fn read(&self) -> Result<Vec<String>, String> {
        let history = self.clone();
        tokio::task::spawn_blocking(move || history.read_manifest())
            .await
            .map_err(|error| format!("Notebook npm inventory task failed: {error}"))?
            .map(|imports| imports.into_iter().collect())
    }

    pub(crate) async fn record(&self, source: &str) -> Result<Option<Vec<String>>, String> {
        let source = source.to_owned();
        let history = self.clone();
        tokio::task::spawn_blocking(move || {
            let imports = source::extract(&source)?;
            if imports.is_empty() {
                return Ok(None);
            }
            let _lock = files::lock(&history.paths.directory)?;
            let mut combined = history.read_manifest()?;
            combined.extend(imports);
            if combined.len() > MAX_IMPORTS {
                return Err(format!(
                    "Notebook npm inventory exceeds {MAX_IMPORTS} imports"
                ));
            }
            let bytes = files::encode(
                &Manifest {
                    schema: 1,
                    project: history.paths.project.clone(),
                    imports: combined.clone(),
                },
                MAX_FILE_BYTES,
            )?;
            files::atomic_write_bytes(&history.paths.directory.join("npm-imports.json"), &bytes)?;
            Ok(Some(combined.into_iter().collect()))
        })
        .await
        .map_err(|error| format!("Notebook npm inventory task failed: {error}"))?
    }

    fn read_manifest(&self) -> Result<BTreeSet<String>, String> {
        let path = self.paths.directory.join("npm-imports.json");
        let Some(manifest) = files::read::<Manifest>(&path, MAX_FILE_BYTES)? else {
            return Ok(BTreeSet::new());
        };
        if manifest.schema != 1
            || manifest.project != self.paths.project
            || manifest.imports.len() > MAX_IMPORTS
        {
            return Err(
                "Notebook npm inventory schema, identity or imports are invalid".to_string(),
            );
        }
        for specifier in &manifest.imports {
            if !source::exact_specifier(specifier)? {
                return Err("Notebook npm inventory imports are invalid".to_string());
            }
        }
        Ok(manifest.imports)
    }
}

pub(crate) fn notice(imports: &[String]) -> String {
    let mut list = String::new();
    let mut included = 0;
    for specifier in imports {
        let separator = if list.is_empty() { "" } else { ", " };
        if list.len() + separator.len() + specifier.len() > MAX_LIST_BYTES {
            break;
        }
        list.push_str(separator);
        list.push_str(specifier);
        included += 1;
    }
    if list.is_empty() {
        list.push_str("none");
    }
    if included < imports.len() {
        list.push_str(&format!(", and {} more", imports.len() - included));
    }
    format!(
        "Available npm imports in this notebook: {list}. Ask before adding another, then use an exact-version npm: specifier"
    )
}
