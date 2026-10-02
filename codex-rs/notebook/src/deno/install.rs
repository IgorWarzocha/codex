use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use sha2::Digest;
use sha2::Sha256;
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

use super::assets::Asset;
use super::assets::VERSION;
use crate::storage::files;

// Bounded local-file chunks cooperate with cancellation. ZIP decoding runs in a
// cancellable staging-only worker because ZipFile is not Send. Only the owning
// async future may publish. A dropped caller can never install an executable.
const CHUNK_BYTES: usize = 64 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const LOCK_TIMEOUT: Duration = Duration::from_secs(240);

/// A read-only probe for startup warmup. No directories, install lock or staging are created.
pub(super) async fn resolve_cached(home: &Path, asset: &Asset) -> Result<Option<PathBuf>, String> {
    let home = match fs::canonicalize(home) {
        Ok(home) => home,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let directory = home
        .join("notebook")
        .join("runtime")
        .join(VERSION)
        .join(asset.target);
    if !directory.exists() {
        return Ok(None);
    }
    let executable = directory.join(asset.executable);
    Ok(cached(&executable, asset).await?.then_some(executable))
}

pub(super) async fn resolve(home: &Path, asset: &Asset) -> Result<PathBuf, String> {
    let directory = runtime_directory(home, asset)?;
    let _lock = lock(&directory).await?;
    let executable = directory.join(asset.executable);
    if cached(&executable, asset).await? {
        tracing::debug!(path = %executable.display(), version = VERSION, "Using verified notebook Deno cache");
        return Ok(executable);
    }
    let url = format!(
        "https://github.com/denoland/deno/releases/download/v{VERSION}/{}",
        asset.archive
    );
    tracing::info!(%url, path = %executable.display(), "Installing notebook Deno privately in CODEX_HOME");
    // Deliberately keep reqwest's ordinary environment/system proxy discovery.
    // The shared helper also honors CODEX_CA_CERTIFICATE and SSL_CERT_FILE.
    let client = codex_http_client::build_reqwest_client_with_custom_ca(
        reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(30))
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent("codex-notebook")
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd(),
    )
    .map_err(|error| error.to_string())?;
    let response = client
        .get(&url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| format!("Download {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Download {url}: {error}"))?;
    let archive = receive(response, asset.archive_bytes).await?;
    publish(&directory, asset, &archive).await?;
    tracing::info!(path = %executable.display(), version = VERSION, "Installed verified notebook Deno");
    Ok(executable)
}

fn runtime_directory(home: &Path, asset: &Asset) -> Result<PathBuf, String> {
    // CODEX_HOME itself may deliberately be linked, like notebook storage.
    fs::create_dir_all(home).map_err(|error| error.to_string())?;
    let mut directory = fs::canonicalize(home).map_err(|error| error.to_string())?;
    for component in ["notebook", "runtime", VERSION, asset.target] {
        directory.push(component);
        files::directory(&directory)?;
    }
    Ok(directory)
}

pub(super) async fn lock(directory: &Path) -> Result<File, String> {
    let path = directory.join("install.lock");
    files::regular_file(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Never unlink this persistent inode. OS ownership ends when the future
    // drops its File, including cancellation, failure, and process death.
    let file = options.open(&path).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => {
                return Err(format!(
                    "Cannot lock Deno cache {}: {error}",
                    path.display()
                ));
            }
        }
    }
}

pub(super) async fn receive(
    mut response: reqwest::Response,
    expected_bytes: u64,
) -> Result<Vec<u8>, String> {
    if let Some(length) = response.content_length()
        && length != expected_bytes
    {
        return Err(format!(
            "Deno archive size mismatch: expected {expected_bytes}, got {length}"
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() as u64 + chunk.len() as u64 > expected_bytes {
            return Err("Deno archive exceeds pinned size".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != expected_bytes {
        return Err("Deno archive is truncated".into());
    }
    Ok(bytes)
}

pub(super) async fn cached(path: &Path, asset: &Asset) -> Result<bool, String> {
    if !files::regular_file(path)? {
        return Ok(false);
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() != asset.binary_bytes {
        tracing::warn!(path = %path.display(), "Replacing notebook Deno cache with invalid size");
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut remaining = asset.binary_bytes;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        let length = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if length == 0 {
            break;
        }
        if length as u64 > remaining {
            return Ok(false);
        }
        remaining -= length as u64;
        hash.update(&buffer[..length]);
        tokio::task::yield_now().await;
    }
    let valid = remaining == 0
        && format!("{:x}", hash.finalize()) == asset.binary_sha256
        && is_executable(path)?;
    if !valid {
        tracing::warn!(path = %path.display(), "Replacing corrupt or non-executable notebook Deno cache");
    }
    Ok(valid)
}

fn is_executable(path: &Path) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions()
            .mode()
            & 0o100
            != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(true)
    }
}

pub(super) async fn publish(directory: &Path, asset: &Asset, bytes: &[u8]) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let directory_owned = directory.to_path_buf();
    let bytes = bytes.to_vec();
    let pin = Asset {
        target: asset.target,
        archive: asset.archive,
        archive_sha256: asset.archive_sha256,
        archive_bytes: asset.archive_bytes,
        executable: asset.executable,
        binary_sha256: asset.binary_sha256,
        binary_bytes: asset.binary_bytes,
    };
    let temporary =
        tokio::task::spawn_blocking(move || extract(&directory_owned, &pin, &bytes, &cancellation))
            .await
            .map_err(|error| format!("Deno archive verification worker failed: {error}"))??;
    let path = directory.join(asset.executable);
    files::regular_file(&path)?;
    // Keep ownership on persist failure so NamedTempFile removes the staging file.
    // No await between this point and publication, so cancellation cannot detach it.
    temporary
        .persist(&path)
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn extract(
    directory: &Path,
    asset: &Asset,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<NamedTempFile, String> {
    let check_cancelled = || {
        if cancellation.is_cancelled() {
            Err("Deno archive verification cancelled".to_string())
        } else {
            Ok(())
        }
    };
    check_cancelled()?;
    if bytes.len() as u64 != asset.archive_bytes {
        return Err("Deno archive size mismatch".into());
    }
    let mut hash = Sha256::new();
    for chunk in bytes.chunks(CHUNK_BYTES) {
        check_cancelled()?;
        hash.update(chunk);
    }
    if format!("{:x}", hash.finalize()) != asset.archive_sha256 {
        return Err("Deno archive SHA-256 mismatch".into());
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    if archive.len() != 1 {
        return Err("Deno archive must contain exactly one executable".into());
    }
    let mut binary = archive.by_index(0).map_err(|error| error.to_string())?;
    let file_type = binary.unix_mode().unwrap_or(0) & 0o170000;
    if binary.name() != asset.executable
        || !binary.is_file()
        || (file_type != 0 && file_type != 0o100000)
        || binary.size() != asset.binary_bytes
    {
        return Err("Deno archive executable name, type, or size mismatch".into());
    }
    // Extract only the exact pinned member, never archive-controlled paths.
    let mut temporary = NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut remaining = asset.binary_bytes;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_cancelled()?;
        let length = binary
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if length == 0 {
            break;
        }
        if length as u64 > remaining {
            return Err("Deno executable exceeds pinned size".into());
        }
        remaining -= length as u64;
        hash.update(&buffer[..length]);
        temporary
            .write_all(&buffer[..length])
            .map_err(|error| error.to_string())?;
    }
    if remaining != 0 || format!("{:x}", hash.finalize()) != asset.binary_sha256 {
        return Err("Deno executable size or SHA-256 mismatch".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    check_cancelled()?;
    Ok(temporary)
}
