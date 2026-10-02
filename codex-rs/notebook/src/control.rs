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
    pub message: String,
    pub details: Value,
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
