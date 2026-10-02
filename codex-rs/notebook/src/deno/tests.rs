use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use sha2::Digest;
use sha2::Sha256;
use zip::write::SimpleFileOptions;

use super::Selection;
use super::assets;
use super::assets::Asset;
use super::install;

fn digest(bytes: &[u8]) -> &'static str {
    Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str())
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default().unix_permissions(0o755))
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn fixture(bytes: &[u8], binary: &[u8]) -> Asset {
    Asset {
        target: "test-target",
        archive: "test.zip",
        archive_sha256: digest(bytes),
        archive_bytes: bytes.len() as u64,
        executable: "deno",
        binary_sha256: digest(binary),
        binary_bytes: binary.len() as u64,
    }
}

fn executable(path: &Path) {
    fs::write(path, "not a runtime, must never be executed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn selection_precedence_is_authoritative_and_cold() {
    let cwd = tempfile::tempdir().unwrap();
    let home = cwd.path().join("home-not-created");
    let bin = cwd.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let program = bin.join(if cfg!(windows) { "deno.exe" } else { "deno" });
    executable(&program);
    let search = std::env::join_paths([&bin]).unwrap();
    assert!(matches!(
        super::select(None, cwd.path(), None, Some(search.clone())).unwrap(),
        Selection::Existing(path) if path == program
    ));
    assert!(
        super::select(
            Some(Path::new("./missing")),
            cwd.path(),
            Some(&home),
            Some(search)
        )
        .is_err()
    );
    assert!(matches!(
        super::select(Some(&program), cwd.path(), Some(&home), Some("".into())).unwrap(),
        Selection::Existing(path) if path == program
    ));
    if assets::current().is_ok() {
        assert!(matches!(
            super::select(None, cwd.path(), Some(&home), Some("".into())).unwrap(),
            Selection::Managed
        ));
    }
    assert!(super::select(None, cwd.path(), None, Some("".into())).is_err());
    assert!(
        !home.exists(),
        "cold selection must not write managed state"
    );
    assert!(super::availability(Some(&program), cwd.path(), Some(&home)).is_ok());
    assert!(!home.exists());
}

#[tokio::test]
async fn prewarm_cache_probe_never_installs_or_repairs() {
    let home = tempfile::tempdir().unwrap();
    let bytes = archive(&[("deno", b"binary")]);
    let asset = fixture(&bytes, b"binary");
    let absent = home.path().join("absent");
    assert!(
        install::resolve_cached(&absent, &asset)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!absent.exists());
    assert!(
        install::resolve_cached(home.path(), &asset)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
    let directory = home
        .path()
        .join("notebook/runtime")
        .join(assets::VERSION)
        .join(asset.target);
    fs::create_dir_all(&directory).unwrap();
    let executable = directory.join(asset.executable);
    assert!(
        install::resolve_cached(home.path(), &asset)
            .await
            .unwrap()
            .is_none()
    );
    install::publish(&directory, &asset, &bytes).await.unwrap();
    assert_eq!(
        install::resolve_cached(home.path(), &asset).await.unwrap(),
        Some(executable.clone())
    );
    fs::write(&executable, b"broken").unwrap();
    assert!(
        install::resolve_cached(home.path(), &asset)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&executable).unwrap(), b"broken");
    assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
}

#[tokio::test]
async fn verified_publication_and_corrupt_cache_repair() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = archive(&[("deno", b"binary")]);
    let asset = fixture(&bytes, b"binary");
    let path = directory.path().join("deno");
    assert!(!install::cached(&path, &asset).await.unwrap());
    install::publish(directory.path(), &asset, &bytes)
        .await
        .unwrap();
    assert!(install::cached(&path, &asset).await.unwrap());
    for corrupt in [b"trunc".as_slice(), b"badbad".as_slice()] {
        fs::write(&path, corrupt).unwrap();
        assert!(!install::cached(&path, &asset).await.unwrap());
        install::publish(directory.path(), &asset, &bytes)
            .await
            .unwrap();
        assert!(install::cached(&path, &asset).await.unwrap());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!install::cached(&path, &asset).await.unwrap());
    }
}

#[tokio::test]
async fn archive_integrity_and_member_validation_precede_publication() {
    let directory = tempfile::tempdir().unwrap();
    let valid = archive(&[("deno", b"binary")]);
    let pin = fixture(&valid, b"binary");
    let mut corrupt = valid.clone();
    corrupt[0] ^= 1;
    for bytes in [corrupt, valid[..valid.len() - 1].to_vec()] {
        assert!(
            install::publish(directory.path(), &pin, &bytes)
                .await
                .is_err()
        );
    }
    let mut symlink = zip::ZipWriter::new(Cursor::new(Vec::new()));
    symlink
        .add_symlink("deno", "binary", SimpleFileOptions::default())
        .unwrap();
    let symlink = symlink.finish().unwrap().into_inner();
    let mut broken_zip = valid.clone();
    broken_zip.truncate(8);
    for bytes in [
        archive(&[("../deno", b"binary")]),
        archive(&[("/deno", b"binary")]),
        archive(&[("deno", b"binary"), ("extra", b"unexpected")]),
        archive(&[("deno", b"too large")]),
        archive(&[("deno", b"badbad")]),
        symlink,
        broken_zip,
    ] {
        let pin = fixture(&bytes, b"binary");
        assert!(
            install::publish(directory.path(), &pin, &bytes)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            0,
            "no executable or staging files may survive failed verification"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn linked_cache_and_lock_paths_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let external = tempfile::NamedTempFile::new().unwrap();
    let bytes = archive(&[("deno", b"binary")]);
    let asset = fixture(&bytes, b"binary");
    std::os::unix::fs::symlink(external.path(), directory.path().join("deno")).unwrap();
    assert!(
        install::cached(&directory.path().join("deno"), &asset)
            .await
            .is_err()
    );
    assert!(
        install::publish(directory.path(), &asset, &bytes)
            .await
            .is_err()
    );
    std::os::unix::fs::symlink(external.path(), directory.path().join("install.lock")).unwrap();
    assert!(install::lock(directory.path()).await.is_err());
    assert_eq!(fs::read(external.path()).unwrap(), b"");
}

#[tokio::test]
async fn cancellation_removes_staging_and_releases_lock() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let binary = vec![7; 256 * 1024];
    let bytes = archive(&[("deno", &binary)]);
    let asset = fixture(&bytes, &binary);
    let lock = install::lock(directory.path()).await.unwrap();
    let mut future = Box::pin(install::publish(directory.path(), &asset, &bytes));
    // Start staging, but do not let this owning future reach publication.
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(future);
    drop(lock);
    // A worker already holding a ZIP chunk must observe cancellation and clean
    // its owned temporary. It has no publication capability.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let reacquired = tokio::time::timeout(Duration::from_secs(1), install::lock(directory.path()))
        .await
        .unwrap()
        .unwrap();
    drop(reacquired);
    assert!(!directory.path().join("deno").exists());
}

#[tokio::test]
async fn concurrent_publication() {
    const CHILD: &str = "CODEX_DENO_PUBLICATION_TEST_HOME";
    if let Some(directory) = std::env::var_os(CHILD) {
        let directory = Path::new(&directory);
        let bytes = archive(&[("deno", b"binary")]);
        let asset = fixture(&bytes, b"binary");
        let _lock = install::lock(directory).await.unwrap();
        if !install::cached(&directory.join("deno"), &asset)
            .await
            .unwrap()
        {
            // Ensure the other process actually has a chance to contend.
            tokio::time::sleep(Duration::from_millis(100)).await;
            install::publish(directory, &asset, &bytes).await.unwrap();
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join("publications"))
                .unwrap()
                .write_all(b"published\n")
                .unwrap();
        }
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let launch = || {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "deno::tests::concurrent_publication",
                "--nocapture",
            ])
            .env(CHILD, directory.path())
            .kill_on_drop(true);
        command.output()
    };
    let (a, b) = tokio::join!(launch(), launch());
    for output in [a.unwrap(), b.unwrap()] {
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read(directory.path().join("publications")).unwrap(),
        b"published\n"
    );
    assert_eq!(fs::read(directory.path().join("deno")).unwrap(), b"binary");
}

#[tokio::test]
async fn bounded_network_body() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    // Test the owned HTTP-body bound, not a simulated release service.
    for (headers, body, expected) in [
        ("Content-Length: 4\r\n", "data", 4),
        ("Content-Length: 4\r\n", "data", 3),
        ("Content-Length: 5\r\n", "data", 5),
        ("", "data", 3),
        ("", "data", 5),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(connection.read(&mut request).await.unwrap() > 0);
            connection
                .write_all(
                    format!("HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n{body}")
                        .as_bytes(),
                )
                .await
                .unwrap();
        };
        let client = async {
            let response = reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap();
            install::receive(response, expected).await
        };
        let ((), result) = tokio::join!(server, client);
        assert_eq!(
            result.is_ok(),
            headers == "Content-Length: 4\r\n" && expected == 4
        );
    }
}

/// `cargo test -p codex-notebook --lib deno::tests::managed_download_kernel -- --ignored --exact --nocapture`
#[tokio::test]
#[ignore = "downloads pinned official Deno and runs a native full-access kernel and LSP"]
async fn managed_download_kernel() {
    const CHILD: &str = "CODEX_DENO_MANAGED_TEST_HOME";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deno::tests::managed_download_kernel",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, home.path())
            .env("PATH", "")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let home = std::path::PathBuf::from(std::env::var_os(CHILD).unwrap());
    let cwd = tempfile::tempdir().unwrap();
    super::availability(None, cwd.path(), Some(&home)).unwrap();
    if std::env::var_os("CODEX_DENO_OFFLINE_TEST").is_none() {
        assert!(!home.join("notebook").exists());
    }
    let path = super::resolve(None, cwd.path(), Some(&home)).await.unwrap();
    assert!(path.starts_with(home.join("notebook/runtime")));
    // A new child with unusable proxies proves the second resolve is offline,
    // without mutating this process's environment or running a second download.
    if std::env::var_os("CODEX_DENO_OFFLINE_TEST").is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deno::tests::managed_download_kernel",
                "--ignored",
                "--nocapture",
            ])
            .env("CODEX_DENO_OFFLINE_TEST", "1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("NO_PROXY", "")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut kernel = codex_notebook_kernel::Kernel::start(codex_notebook_kernel::KernelOptions {
        deno: path.clone(),
        cwd: Some(cwd.path().into()),
        max_heap_mib: Some(128),
        execute_timeout: Duration::from_secs(10),
        ..Default::default()
    })
    .await
    .unwrap();
    let result = kernel
        .execute("let managedCounter: number = 40; console.log(managedCounter + 2)")
        .await
        .unwrap();
    assert_eq!(result.status, codex_notebook_kernel::ExecutionStatus::Ok);
    assert!(result.outputs.iter().any(|output| matches!(output, codex_notebook_kernel::Output::Stream { text, .. } if text.contains("42"))));
    kernel.shutdown().await.unwrap();
    crate::journal::Journal::new(&home, cwd.path(), "managed-test")
        .unwrap()
        .append(
            "managed-cell",
            "const invalid: number = 'wrong';",
            "ok",
            None,
            &[],
        )
        .unwrap();
    let diagnostics = crate::diagnostics::diagnostics_with_runtime(
        &path,
        cwd.path(),
        &home,
        "managed-test",
        "not_started",
        &[],
        crate::persistence::PersistenceBudget::from_heap_mib(Some(4096)),
    )
    .await
    .unwrap();
    assert!(
        diagnostics.details.to_string().contains("2322"),
        "{diagnostics:?}"
    );
}
