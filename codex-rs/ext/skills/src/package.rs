//! Package traversal stays on the filesystem that supplied the skill. Canonical
//! containment is checked both during inventory and immediately before a read.

use std::collections::HashSet;
use std::io;

use codex_exec_server::EnvironmentAccess;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemReadStream;
use codex_exec_server::ReadDirectoryEntry;
use codex_utils_path_uri::PathUri;
use futures::StreamExt;

use crate::provider::MAX_SKILL_RESOURCE_CONTENT_BYTES;

pub(crate) enum PackageAccess<'a> {
    Host(&'a dyn ExecutorFileSystem),
    Executor(&'a dyn EnvironmentAccess),
}

impl PackageAccess<'_> {
    async fn canonicalize(&self, path: &PathUri) -> io::Result<PathUri> {
        match self {
            Self::Host(fs) => fs.canonicalize(path, None).await,
            Self::Executor(fs) => fs.canonicalize(path).await,
        }
    }

    async fn read_directory(&self, path: &PathUri) -> io::Result<Vec<ReadDirectoryEntry>> {
        match self {
            Self::Host(fs) => fs.read_directory(path, None).await,
            Self::Executor(fs) => fs.read_directory(path).await,
        }
    }

    async fn read_stream(&self, path: &PathUri) -> io::Result<FileSystemReadStream> {
        match self {
            Self::Host(fs) => fs.read_file_stream(path, None).await,
            Self::Executor(fs) => fs.read_file_stream(path).await,
        }
    }

    pub(crate) async fn read_text(&self, root: &PathUri, path: &PathUri) -> io::Result<String> {
        let root = self.canonicalize(root).await?;
        let path = self.canonicalize(path).await?;
        if !path.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "skill resource escapes its package",
            ));
        }
        let mut stream = self.read_stream(&path).await?;
        let mut contents = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if contents.len().saturating_add(chunk.len()) > MAX_SKILL_RESOURCE_CONTENT_BYTES {
                return Err(io::Error::other("skill resource exceeds content limit"));
            }
            contents.extend_from_slice(&chunk);
        }
        String::from_utf8(contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    pub(crate) async fn files(&self, root: &PathUri) -> io::Result<Vec<(String, PathUri)>> {
        let canonical_root = self.canonicalize(root).await?;
        let mut pending = vec![(String::new(), root.clone(), false)];
        let mut visited = HashSet::new();
        let mut files = Vec::new();
        let mut examined = 0usize;
        while let Some((relative, directory, shallow)) = pending.pop() {
            let canonical = self.canonicalize(&directory).await?;
            if !canonical.starts_with(&canonical_root) || !visited.insert(canonical) {
                continue;
            }
            let mut entries = self.read_directory(&directory).await?;
            entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));
            for entry in entries {
                examined += 1;
                if examined > 8192 {
                    return Err(io::Error::other(
                        "skill package inventory exceeds 8192 entries",
                    ));
                }
                let name = &entry.file_name;
                if name.starts_with('.') || name == "node_modules" || name.contains(['/', '\\']) {
                    continue;
                }
                let path = directory.join(name).map_err(io::Error::other)?;
                let canonical = match self.canonicalize(&path).await {
                    Ok(path) => path,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(err),
                };
                if !canonical.starts_with(&canonical_root) {
                    continue;
                }
                let child = if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                };
                if entry.is_directory {
                    if shallow {
                        files.push((child, path));
                    } else {
                        let assets = relative.is_empty() && name == "assets";
                        pending.push((child, path, assets));
                    }
                } else if entry.is_file {
                    files.push((child, path));
                }
            }
        }
        files.sort_by(|left, right| {
            (left.0 != "SKILL.md", &left.0).cmp(&(right.0 != "SKILL.md", &right.0))
        });
        Ok(files)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use codex_exec_server::FileSystemEnvironmentAccessor;
    use codex_exec_server::LOCAL_FS;

    #[tokio::test]
    async fn package_inventory_and_reads_reject_symlink_escapes_and_cycles() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("references"))?;
        std::fs::write(root.path().join("references/inside.md"), "inside")?;
        std::fs::write(outside.path().join("secret.md"), "outside")?;
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            root.path().join("references/escape.md"),
        )?;
        std::os::unix::fs::symlink(root.path(), root.path().join("cycle"))?;
        let root_path = PathUri::from_host_native_path(root.path()).map_err(io::Error::other)?;
        let escape = root_path
            .join("references/escape.md")
            .map_err(io::Error::other)?;
        let accessor = FileSystemEnvironmentAccessor::unrestricted(&LOCAL_FS);
        for access in [
            PackageAccess::Host(LOCAL_FS.as_ref()),
            PackageAccess::Executor(&accessor),
        ] {
            let files = access.files(&root_path).await?;
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].0, "references/inside.md");
            assert_eq!(
                access
                    .read_text(&root_path, &escape)
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        Ok(())
    }
}
