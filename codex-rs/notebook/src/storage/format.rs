use std::collections::BTreeMap;
use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

pub(super) const MAX_FILE_BYTES: u64 = 192 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub deno: String,
    pub v8: String,
    pub entries: Vec<Entry>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub name: String,
    pub kind: Kind,
    pub data: String,
    pub length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<Hook>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Value,
    Function,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Hook {
    Startup,
    ToolResult,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Skipped {
    pub name: String,
    pub reason: String,
}

impl Snapshot {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut snapshot: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("Invalid notebook snapshot: {error}"))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&mut self) -> Result<(), String> {
        if self.deno.is_empty()
            || self.deno.len() > 256
            || self.v8.is_empty()
            || self.v8.len() > 256
        {
            return Err("Invalid notebook runtime provenance".into());
        }
        if self.entries.len() > MAX_ENTRIES || self.skipped.len() > MAX_ENTRIES {
            return Err("Notebook snapshot exceeds 10000 bindings".into());
        }
        let mut names = BTreeSet::new();
        let mut length = 0u64;
        for entry in &mut self.entries {
            if !identifier(&entry.name) || !names.insert(entry.name.clone()) {
                return Err(format!(
                    "Invalid or duplicate notebook binding: {}",
                    entry.name
                ));
            }
            length = length
                .checked_add(entry.length)
                .ok_or("Notebook snapshot payload length overflow")?;
            if length > MAX_PAYLOAD_BYTES || entry.data.len() as u64 > MAX_FILE_BYTES {
                return Err("Notebook snapshot exceeds 64 MiB serialized payload".into());
            }
            let bytes = STANDARD
                .decode(&entry.data)
                .map_err(|_| format!("Invalid base64 for notebook binding: {}", entry.name))?;
            if bytes.len() as u64 != entry.length || STANDARD.encode(&bytes) != entry.data {
                return Err(format!(
                    "Notebook binding length or base64 mismatch: {}",
                    entry.name
                ));
            }
            validate_metadata(entry.description.as_deref(), 256, 1)?;
            validate_metadata(entry.usage.as_deref(), 512, 4)?;
            if entry.pinned == Some(false) {
                entry.pinned = None;
            }
            if entry.hook.is_some() && (entry.pinned != Some(true) || entry.kind != Kind::Function)
            {
                return Err(format!(
                    "Notebook hooks require pinned functions: {}",
                    entry.name
                ));
            }
        }
        for skipped in &self.skipped {
            if !identifier(&skipped.name)
                || !names.insert(skipped.name.clone())
                || skipped.reason.is_empty()
                || skipped.reason.len() > 4096
            {
                return Err(format!(
                    "Invalid or duplicate skipped binding: {}",
                    skipped.name
                ));
            }
        }
        self.entries.sort_by(|a, b| a.name.cmp(&b.name));
        self.skipped.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    pub fn value(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|error| error.to_string())
    }

    pub fn unpin(&mut self, names: &BTreeSet<String>) -> Vec<String> {
        let mut changed = Vec::new();
        for entry in &mut self.entries {
            if names.contains(&entry.name) {
                entry.pinned = None;
                entry.hook = None;
                changed.push(entry.name.clone());
            }
        }
        changed
    }
}

pub(super) fn identifier(name: &str) -> bool {
    let mut chars = name.bytes();
    name.len() <= 4096
        && chars
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == b'_' || ch == b'$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'$')
}

pub(super) fn profile_name(name: &str) -> Result<(), String> {
    if name.len() <= 64
        && name
            .bytes()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err("Notebook profile name must be 1-64 letters, numbers, dots, underscores, or hyphens and start with a letter or number".into())
    }
}

fn validate_metadata(
    value: Option<&str>,
    max_bytes: usize,
    max_lines: usize,
) -> Result<(), String> {
    if value.is_some_and(|text| {
        text.is_empty()
            || text.len() > max_bytes
            || text.split('\n').count() > max_lines
            || text.chars().any(|ch| ch.is_control() && ch != '\n')
    }) {
        return Err("Invalid notebook binding description or usage".into());
    }
    Ok(())
}

pub(super) struct Merge {
    pub snapshot: Snapshot,
    pub baseline: Snapshot,
    pub conflicts: Vec<String>,
    pub applied: Vec<String>,
}

// The next baseline is the candidate, not the merged project. A private fork that
// loses a conflict must not overwrite the winner on its next unchanged checkpoint.
pub(super) fn merge(
    base: Option<&Snapshot>,
    current: Option<&Snapshot>,
    candidate: &Snapshot,
) -> Result<Merge, String> {
    let entries = |snapshot: Option<&Snapshot>| -> BTreeMap<String, Entry> {
        snapshot
            .into_iter()
            .flat_map(|snapshot| &snapshot.entries)
            .map(|entry| (entry.name.clone(), entry.clone()))
            .collect()
    };
    let base = entries(base);
    let current = entries(current);
    let proposed = entries(Some(candidate));
    let skipped: BTreeSet<_> = candidate
        .skipped
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    let names: BTreeSet<_> = base
        .keys()
        .chain(current.keys())
        .chain(proposed.keys())
        .chain(skipped.iter())
        .cloned()
        .collect();
    let mut snapshot = Snapshot {
        entries: Vec::new(),
        ..candidate.clone()
    };
    // Project skips describe capture failures, not deletions of retained values.
    snapshot.skipped.clear();
    let mut baseline = candidate.clone();
    baseline.skipped.clear();
    let mut conflicts = Vec::new();
    let mut applied = Vec::new();
    for name in names {
        let previous = base.get(&name);
        let existing = current.get(&name);
        let next = if skipped.contains(&name) {
            previous
        } else {
            proposed.get(&name)
        };
        let changed = next != previous;
        let protected_deletion =
            changed && next.is_none() && existing.is_some_and(|entry| entry.pinned == Some(true));
        let conflict = protected_deletion || (changed && existing != previous && next != existing);
        let selected = if conflict {
            conflicts.push(name.clone());
            existing
        } else if changed {
            applied.push(name.clone());
            next
        } else {
            existing
        };
        if let Some(entry) = selected {
            snapshot.entries.push(entry.clone());
        }
        if skipped.contains(&name)
            && let Some(entry) = previous
        {
            baseline.entries.push(entry.clone());
        }
    }
    snapshot.validate()?;
    baseline.validate()?;
    Ok(Merge {
        snapshot,
        baseline,
        conflicts,
        applied,
    })
}
