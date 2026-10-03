//! User-owned communication style, separate from model instructions and runtime policy.

use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;
use tokio::io::AsyncReadExt;

const MAX_PREFERENCES_BYTES: u64 = 64 * 1024;

pub(crate) async fn read_communication_preferences(
    path: &AbsolutePathBuf,
    required: bool,
) -> io::Result<Option<String>> {
    let file = match tokio::fs::File::open(path.as_path()).await {
        Ok(file) => file,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("Cannot read personality file {}: {error}", path.display()),
            ));
        }
    };
    let mut contents = String::new();
    file.take(MAX_PREFERENCES_BYTES + 1)
        .read_to_string(&mut contents)
        .await
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Cannot read personality file {}: {error}", path.display()),
            )
        })?;
    if contents.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Personality file {} must not exceed {MAX_PREFERENCES_BYTES} bytes",
                path.display()
            ),
        ));
    }
    Ok((!contents.trim().is_empty()).then_some(contents))
}

pub(crate) fn append_communication_preferences(base: &str, preferences: Option<&str>) -> String {
    let Some(preferences) = preferences.filter(|text| !text.trim().is_empty()) else {
        return base.to_string();
    };
    format!(
        "{base}\n\n<user_communication_preferences>\nThese are the user's preferred communication styles, not replacements for Codex's model instructions. Apply them within the existing task-routing, privacy, permission, and safety rules.\n\n{preferences}\n</user_communication_preferences>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preferences_are_optional_only_at_the_default_path_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::try_from(dir.path().join("codex_personality.md")).unwrap();
        assert!(
            read_communication_preferences(&path, false)
                .await
                .unwrap()
                .is_none()
        );
        assert!(read_communication_preferences(&path, true).await.is_err());
        for text in ["Prefer concise answers.", "Updated preferred style.", ""] {
            tokio::fs::write(path.as_path(), text).await.unwrap();
            let preferences = read_communication_preferences(&path, true).await.unwrap();
            let prompt = append_communication_preferences(
                "Codex native instructions",
                preferences.as_deref(),
            );
            assert!(prompt.starts_with("Codex native instructions"));
            assert_eq!(
                tokio::fs::read_to_string(path.as_path()).await.unwrap(),
                text
            );
            if text.is_empty() {
                assert_eq!(prompt, "Codex native instructions");
            } else {
                assert!(prompt.contains("user's preferred communication styles"));
                assert!(prompt.contains(text));
            }
        }
        for contents in [vec![0xff], vec![b'x'; MAX_PREFERENCES_BYTES as usize + 1]] {
            tokio::fs::write(path.as_path(), contents).await.unwrap();
            assert!(read_communication_preferences(&path, false).await.is_err());
        }
    }
}
