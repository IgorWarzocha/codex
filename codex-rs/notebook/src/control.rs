use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Host-side controls. Mutations are never dispatched through the nested tool bridge.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotebookRequest {
    Status {
        query: Option<String>,
    },
    List {
        query: Option<String>,
    },
    Diagnostics,
    Checkpoint,
    Restart,
    Reset,
    Save {
        name: String,
    },
    Load {
        name: String,
    },
    Pin {
        names: Vec<String>,
        hook: Option<NotebookHook>,
    },
    Unpin {
        names: Vec<String>,
    },
    Release {
        names: Vec<String>,
    },
    Prune {
        query: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotebookHook {
    Startup,
    ToolResult,
    Removed,
}

impl Serialize for NotebookHook {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Startup => serializer.serialize_str("startup"),
            Self::ToolResult => serializer.serialize_str("tool_result"),
            Self::Removed => serializer.serialize_bool(false),
        }
    }
}

impl<'de> Deserialize<'de> for NotebookHook {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(value) if value == "startup" => Ok(Self::Startup),
            Value::String(value) if value == "tool_result" => Ok(Self::ToolResult),
            Value::Bool(false) => Ok(Self::Removed),
            _ => Err(serde::de::Error::custom(
                "hook must be startup, tool_result, or false",
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NotebookControlResult {
    /// Complete bounded model-facing text. `details` is for structured consumers.
    pub message: String,
    pub details: Value,
}

impl NotebookControlResult {
    pub(crate) fn with_details(label: &str, details: Value) -> Self {
        Self {
            message: format!("{label}\n{}", format_details(&details, 12 * 1024)),
            details,
        }
    }
}

/// Bound each field independently so large inventories cannot hide failures,
/// disposal results or continuation handles. Raw details stay unchanged.
pub(crate) fn format_details(details: &Value, budget: usize) -> String {
    use crate::journal::bound_text;

    if let Some(fields) = details.as_object().filter(|fields| !fields.is_empty()) {
        let overhead = fields.keys().map(|key| key.len() + 4).sum::<usize>() + 2;
        let field_budget = budget.saturating_sub(overhead) / fields.len();
        let text = fields
            .iter()
            .map(|(key, value)| format!("{key}: {}", format_details(value, field_budget)))
            .collect::<Vec<_>>()
            .join(", ");
        return bound_text(&format!("{{{text}}}"), budget);
    }
    let text = details.to_string();
    if text.len() > budget
        && let Some(items) = details.as_array()
    {
        let count = format!("{} items ", items.len());
        return bound_text(
            &format!(
                "{count}{}",
                bound_text(&text, budget.saturating_sub(count.len()))
            ),
            budget,
        );
    }
    bound_text(&text, budget)
}

pub(crate) fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

pub(crate) fn matches(query: &str, name: &str) -> bool {
    // Iterative wildcard matching avoids recursion on untrusted patterns.
    let query = query.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let (pattern, text) = (query.as_bytes(), name.as_bytes());
    let (mut p, mut t, mut star, mut retry) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = t;
        } else if let Some(s) = star {
            p = s + 1;
            retry += 1;
            t = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mutation_message_bounds_inventory_without_hiding_other_fields() {
        let details = json!({"released":["x".repeat(64 * 1024)],"failures":[{"name":"shared","reason":"concurrent pin"}],"warnings":["partial release"],"memory":{"rssBytes":99},"disposal":{"failures":[{"name":"handle","reason":"dispose failed"}]},"checkpoint":{"skipped":[{"name":"socket","reason":"runtime-only"}]},"continuation":{"cellId":"next-cell"}});
        let result = NotebookControlResult::with_details("Notebook release", details.clone());
        assert!(result.message.len() < 13 * 1024);
        for text in [
            "concurrent pin",
            "partial release",
            "rssBytes",
            "dispose failed",
            "runtime-only",
            "next-cell",
            "truncated",
        ] {
            assert!(
                result.message.contains(text),
                "missing {text}: {}",
                result.message
            );
        }
        assert_eq!(result.message.matches("concurrent pin").count(), 1);
        assert_eq!(result.details, details);
    }
}
