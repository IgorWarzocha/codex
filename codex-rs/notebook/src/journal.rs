//! Bounded historical notebooks. These documents are evidence, never executable state.
use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use codex_code_mode_protocol::FunctionCallOutputContentItem;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

pub(crate) const DEFAULT_MAX_BYTES: usize = 32 * 1024 * 1024;
const OUTPUT_RESERVE: usize = 4096;

#[derive(Clone)]
pub(crate) struct Journal {
    pub(crate) path: PathBuf,
    home: PathBuf,
    project: PathBuf,
    thread_id: String,
    max_bytes: usize,
}

pub(crate) struct CodeCell {
    pub(crate) id: String,
    pub(crate) index: usize,
    pub(crate) source: String,
}

impl Journal {
    pub(crate) fn new(codex_home: &Path, cwd: &Path, thread_id: &str) -> Result<Self, String> {
        Self::with_budget(codex_home, cwd, thread_id, DEFAULT_MAX_BYTES)
    }

    pub(crate) fn with_budget(
        codex_home: &Path,
        cwd: &Path,
        thread_id: &str,
        max_bytes: usize,
    ) -> Result<Self, String> {
        let project = cwd
            .canonicalize()
            .map_err(|e| format!("resolve journal project: {e}"))?;
        let project_key = digest(project.to_string_lossy().as_bytes());
        let thread_key = digest(thread_id.as_bytes());
        // CODEX_HOME itself may be a legitimate symlink. Resolve it once without
        // creating anything; lifecycle does not construct journals for ephemeral threads.
        let home = codex_home
            .canonicalize()
            .map_err(|e| format!("resolve journal home: {e}"))?;
        Ok(Self {
            path: home
                .join("notebook/journals")
                .join(project_key)
                .join(format!("{thread_key}.ipynb")),
            home,
            project,
            thread_id: thread_id.to_owned(),
            max_bytes,
        })
    }

    pub(crate) fn begin(&self, id: &str, source: &str) -> Result<(), String> {
        self.record(id, source, "running", None, &[])
    }

    /// Upserts completion even if the begin record was rotated out. Caller serializes writes
    /// per thread. Output must be collected independently of destructive exec/wait drains.
    pub(crate) fn append(
        &self,
        id: &str,
        source: &str,
        status: &str,
        error: Option<&str>,
        items: &[FunctionCallOutputContentItem],
    ) -> Result<(), String> {
        self.record(id, source, status, error, items)
    }

    fn empty(&self) -> Value {
        json!({
            "nbformat": 4, "nbformat_minor": 5, "cells": [],
            "metadata": {
                "kernelspec": {"display_name": "Deno", "language": "typescript", "name": "deno"},
                "language_info": {"name": "typescript"},
                "codex": {"project": self.project, "threadId": self.thread_id}
            }
        })
    }

    fn record(
        &self,
        id: &str,
        source: &str,
        status: &str,
        error: Option<&str>,
        items: &[FunctionCallOutputContentItem],
    ) -> Result<(), String> {
        if !valid_id(id) {
            return Err("invalid notebook cell id".into());
        }
        if !matches!(status, "running" | "ok" | "error" | "terminated") {
            return Err("invalid notebook cell status".into());
        }
        let mut document = self.read()?;
        // JSON escaping can expand each source/output byte by up to six bytes.
        let output_budget = self
            .max_bytes
            .saturating_sub(source.len().saturating_mul(6))
            .saturating_sub(OUTPUT_RESERVE)
            / 6;
        let outputs = journal_outputs(items, error, output_budget);
        let cells = document["cells"]
            .as_array_mut()
            .ok_or("invalid journal cells")?;
        let position = cells.iter().position(|cell| cell["id"] == id);
        if status == "running" && position.is_some() {
            return Ok(());
        }
        let count = position.map_or(cells.len() + 1, |i| i + 1);
        let cell = json!({
            "id": id, "cell_type": "code", "execution_count": count,
            "metadata": {"codex": {"cellId": id, "status": status}},
            "source": source, "outputs": outputs
        });
        if let Some(index) = position {
            cells[index] = cell.clone();
        } else {
            cells.push(cell.clone());
        }
        let mut bytes = encode(&document)?;
        if bytes.len() > self.max_bytes {
            document = self.empty();
            let mut cell = cell;
            cell["execution_count"] = json!(1);
            document["cells"] = json!([cell]);
            bytes = encode(&document)?;
            if bytes.len() > self.max_bytes {
                return Err("Notebook journal cell exceeds the persistence budget".into());
            }
            // Keep exactly one prior bounded notebook. Copy atomically before replacing current,
            // so failure never leaves current absent and interrupted rotation remains readable.
            if self.path.exists() {
                let previous = self.read()?;
                atomic_write(
                    &self.home,
                    &self.path.with_extension("previous.ipynb"),
                    &encode(&previous)?,
                )?;
            }
        }
        atomic_write(&self.home, &self.path, &bytes)
    }

    fn read(&self) -> Result<Value, String> {
        let directory = self.path.parent().ok_or("journal path has no parent")?;
        if !private_directories(&self.home, directory, false)? {
            return Ok(self.empty());
        }
        regular_file(&self.path.with_extension("previous.ipynb"))?;
        if !regular_file(&self.path)? {
            return Ok(self.empty());
        }
        let metadata = fs::symlink_metadata(&self.path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > self.max_bytes as u64 {
            return Err(format!(
                "Notebook journal is invalid or exceeds budget: {}",
                self.path.display()
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(&self.path)
            .map_err(|e| e.to_string())?
            .take(self.max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > self.max_bytes {
            return Err("Notebook journal exceeds budget".into());
        }
        let document: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Notebook journal is invalid: {e}"))?;
        validate(&document)?;
        Ok(document)
    }

    pub(crate) fn code_cells(&self) -> Result<Vec<CodeCell>, String> {
        let document = self.read()?;
        let cells = document["cells"]
            .as_array()
            .ok_or("invalid journal cells")?;
        cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell["cell_type"] == "code")
            .map(|(index, cell)| {
                Ok(CodeCell {
                    id: cell["id"].as_str().ok_or("invalid cell id")?.to_owned(),
                    index,
                    source: source_text(&cell["source"])?,
                })
            })
            .collect()
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate(document: &Value) -> Result<(), String> {
    if document["nbformat"] != 4
        || document["nbformat_minor"] != 5
        || !document["metadata"].is_object()
    {
        return Err("invalid notebook journal document".into());
    }
    let mut ids = HashSet::new();
    for cell in document["cells"]
        .as_array()
        .ok_or("invalid notebook journal cells")?
    {
        let id = cell["id"].as_str().ok_or("missing notebook cell id")?;
        if !valid_id(id)
            || !ids.insert(id)
            || !cell["metadata"].is_object()
            || !cell["cell_type"].is_string()
        {
            return Err("invalid or duplicate notebook cell".into());
        }
        source_text(&cell["source"])?;
        if cell["cell_type"] == "code" && !cell["outputs"].is_array() {
            return Err("invalid notebook outputs".into());
        }
    }
    Ok(())
}

fn source_text(source: &Value) -> Result<String, String> {
    if let Some(text) = source.as_str() {
        return Ok(text.to_owned());
    }
    source
        .as_array()
        .ok_or("invalid notebook source")?
        .iter()
        .map(|line| line.as_str().ok_or("invalid notebook source line"))
        .collect::<Result<Vec<_>, _>>()
        .map(|lines| lines.concat())
        .map_err(str::to_owned)
}

fn encode(document: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(document).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

// Check each storage-owned component rather than following links through create_dir_all.
// Reads are cold: absent directories are not created by diagnostics.
fn private_directories(home: &Path, directory: &Path, create: bool) -> Result<bool, String> {
    let relative = directory.strip_prefix(home).map_err(|e| e.to_string())?;
    let mut current = home.to_path_buf();
    let root = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err("invalid journal home directory".into());
    }
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err("invalid journal directory component".into());
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(format!("invalid journal directory: {}", current.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(false);
                }
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.to_string()),
                }
                let metadata = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("invalid journal directory".into());
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if create {
                fs::set_permissions(&current, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(true)
}

fn regular_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(format!("invalid journal file: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn atomic_write(home: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("journal path has no parent")?;
    private_directories(home, directory, true)?;
    regular_file(path)?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        private_directories(home, directory, false)?;
        regular_file(path)?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn bound_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let suffix = " [truncated]";
    let mut end = max_bytes.saturating_sub(suffix.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = text[..end].to_owned();
    result.push_str(&suffix[..suffix.len().min(max_bytes)]);
    result
}

fn journal_outputs(
    items: &[FunctionCallOutputContentItem],
    error: Option<&str>,
    mut remaining: usize,
) -> Vec<Value> {
    let mut outputs = Vec::new();
    let mut truncated = false;
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                let bounded = bound_text(text, remaining);
                truncated |= bounded != *text;
                remaining = remaining.saturating_sub(bounded.len());
                outputs.push(json!({"output_type":"stream", "name":"stdout", "text":bounded}));
            }
            FunctionCallOutputContentItem::InputImage { image_url, .. } => {
                if let Some((mime, data)) = image_url
                    .strip_prefix("data:")
                    .and_then(|url| url.split_once(";base64,"))
                {
                    if data.len() <= remaining {
                        remaining -= data.len();
                        outputs.push(json!({"output_type":"display_data", "data":{mime:data}, "metadata":{}}));
                    } else {
                        truncated = true;
                    }
                }
            }
            FunctionCallOutputContentItem::InputAudio { .. } => {}
        }
        if remaining == 0 {
            truncated = true;
            break;
        }
    }
    if truncated {
        outputs.push(json!({"output_type":"stream","name":"stderr","text":"[notebook journal output truncated]\n"}));
    }
    if let Some(error) = error {
        let error = bound_text(error, remaining / 2);
        outputs.push(json!({"output_type":"error","ename":"NotebookCellError","evalue":error,"traceback":error.lines().collect::<Vec<_>>() }));
    }
    outputs
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
