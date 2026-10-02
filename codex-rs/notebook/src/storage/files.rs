use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Paths {
    pub project: PathBuf,
    pub directory: PathBuf,
    pub session: PathBuf,
    pub profiles: PathBuf,
}

impl Paths {
    pub fn new(home: PathBuf, cwd: &Path, thread_id: &str) -> Result<Self, String> {
        Self::resolve(home, cwd, thread_id, true)
    }

    pub(crate) fn for_profile_reads(
        home: PathBuf,
        cwd: &Path,
        thread_id: &str,
    ) -> Result<Self, String> {
        Self::resolve(home, cwd, thread_id, false)
    }

    fn resolve(home: PathBuf, cwd: &Path, thread_id: &str, create: bool) -> Result<Self, String> {
        if thread_id.is_empty() || thread_id.len() > 4096 {
            return Err("Invalid notebook thread identifier".into());
        }
        let cwd = fs::canonicalize(cwd).map_err(|error| format!("Notebook cwd: {error}"))?;
        if !cwd.is_dir() {
            return Err("Notebook cwd must be a directory".into());
        }
        let project = cwd
            .ancestors()
            .find(|path| path.join(".git").exists())
            .unwrap_or(&cwd)
            .to_path_buf();
        // CODEX_HOME itself may intentionally be linked. All storage-owned paths
        // beneath its resolved target must be real directories and private files.
        if create {
            fs::create_dir_all(&home).map_err(|error| error.to_string())?;
        }
        let root = fs::canonicalize(home)
            .map_err(|error| error.to_string())?
            .join("notebook");
        let projects = root.join("projects");
        let directory_path = projects.join(key(project.as_os_str().as_encoded_bytes()));
        let sessions = directory_path.join("sessions");
        let profiles = root.join("profiles");
        if create {
            directory(&root)?;
            directory(&projects)?;
            directory(&directory_path)?;
            directory(&sessions)?;
            directory(&profiles)?;
        }
        Ok(Self {
            project,
            directory: directory_path,
            session: sessions.join(format!("{}.json", key(thread_id.as_bytes()))),
            profiles,
        })
    }
}

fn key(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        if let Err(error) = builder.create(path)
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(format!(
                "Cannot create notebook storage {}: {error}",
                path.display()
            ));
        }
    }
    verify_directory(path)
}

fn verify_directory(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!(
                "Notebook storage must be a real directory: {}",
                ancestor.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn regular_file(path: &Path) -> Result<bool, String> {
    if let Some(parent) = path.parent() {
        verify_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(format!(
            "Notebook storage must be a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Notebook storage {}: {error}", path.display())),
    }
}

pub(crate) fn read<T: DeserializeOwned>(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<T>, String> {
    if !regular_file(path)? {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("Notebook state is too large: {}", path.display()));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Corrupt notebook state {}: {error}", path.display()))
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(crate) fn atomic_write<T: Serialize>(
    path: &Path,
    value: &T,
    max_bytes: usize,
) -> Result<(), String> {
    atomic_write_bytes(path, &encode(value, max_bytes)?)
}

pub(crate) fn encode<T: Serialize>(value: &T, max_bytes: usize) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err("Notebook state file exceeds storage limit".into());
    }
    Ok(bytes)
}

pub(crate) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    regular_file(path)?;
    let parent = path
        .parent()
        .ok_or("Notebook state has no parent directory")?;
    directory(parent)?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = options()
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn remove(path: &Path) -> Result<(), String> {
    if regular_file(path)? {
        fs::remove_file(path).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

// A persistent inode carries the OS lock. Never unlink it: replacing a lock file
// would let two processes hold locks on different inodes for the same project.
pub(crate) fn lock(directory_path: &Path) -> Result<File, String> {
    directory(directory_path)?;
    let path = directory_path.join("write.lock");
    regular_file(&path)?;
    let file = options()
        .read(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(format!(
                    "Cannot lock notebook storage {}: {error}",
                    path.display()
                ));
            }
        }
    }
}
