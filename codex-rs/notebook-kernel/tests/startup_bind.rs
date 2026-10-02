#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use codex_notebook_kernel::ExecutionStatus;
use codex_notebook_kernel::Kernel;
use codex_notebook_kernel::KernelError;
use codex_notebook_kernel::KernelOptions;
use serde_json::Value;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct Launcher {
    directory: TempDir,
    options: KernelOptions,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl Launcher {
    fn new(mode: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("launch-deno");
        std::fs::write(&program, "#!/bin/sh\nexec \"$CODEX_TEST_REAL_DENO\" run --allow-all --no-config \"$CODEX_TEST_STARTUP_FIXTURE\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut options = KernelOptions {
            deno: program,
            cwd: Some(directory.path().into()),
            shutdown_timeout: Duration::from_millis(300),
            ..KernelOptions::default()
        };
        options.env.insert(
            "CODEX_TEST_REAL_DENO".into(),
            std::env::var_os("DENO_KERNEL_TEST_BIN")
                .or_else(|| std::env::var_os("DENO_PROGRAM"))
                .unwrap_or_else(|| "deno".into()),
        );
        options.env.insert(
            "CODEX_TEST_STARTUP_FIXTURE".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/startup-launcher.js")
                .into_os_string(),
        );
        options.env.insert(
            "CODEX_TEST_STARTUP_DIRECTORY".into(),
            directory.path().as_os_str().to_owned(),
        );
        options
            .env
            .insert("CODEX_TEST_STARTUP_MODE".into(), mode.into());
        Self { directory, options }
    }

    fn attempts(&self) -> Vec<Value> {
        match std::fs::read_to_string(self.directory.path().join("attempts.jsonl")) {
            Ok(log) => log
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read launcher log: {error}"),
        }
    }

    async fn wait_for_retry(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.attempts().len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual collision caused a startup retry");
    }

    async fn assert_clean(&self) {
        let attempts = self.attempts();
        for attempt in attempts {
            assert!(
                !PathBuf::from(attempt["path"].as_str().unwrap()).exists(),
                "connection credentials survived cleanup"
            );
            assert_eq!(
                attempt["previousConnectionExists"], false,
                "retry started before the previous owner cleaned up"
            );
            for key in ["leader", "child"] {
                let pid = attempt[key].as_u64().unwrap().to_string();
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        let status = tokio::process::Command::new("ps")
                            .args(["-p", &pid, "-o", "stat="])
                            .output()
                            .await
                            .unwrap();
                        let state = String::from_utf8_lossy(&status.stdout);
                        // An adopted zombie cannot perform work. Its final reap belongs to the OS parent.
                        if state.trim().is_empty() || state.trim().starts_with('Z') {
                            break;
                        }
                        assert!(status.status.success(), "process probe failed: {status:?}");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("owned startup process survived cleanup");
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires an actual Deno Jupyter executable"]
async fn actual_bind_collision_retries_with_fresh_credentials_then_executes_once() {
    let launcher = Launcher::new("once");
    let mut kernel = Kernel::start(launcher.options.clone()).await.unwrap();
    let attempts = launcher.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt["path"].to_string())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    assert_ne!(attempts[0]["key"], attempts[1]["key"]);
    assert_ne!(attempts[0]["ports"], attempts[1]["ports"]);
    let result = kernel
        .execute("Deno.writeTextFileSync('executed', 'x', {append:true})")
        .await
        .unwrap();
    assert_eq!(result.status, ExecutionStatus::Ok);
    kernel.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(launcher.directory.path().join("executed")).unwrap(),
        "x"
    );
    launcher.assert_clean().await;
}

#[tokio::test]
#[ignore = "requires an actual Deno Jupyter executable"]
async fn persistent_bind_collision_exhausts_only_the_bounded_startup_attempts() {
    let launcher = Launcher::new("persistent");
    let error = Kernel::start(launcher.options.clone())
        .await
        .err()
        .expect("persistent collision must fail");
    assert!(
        matches!(error, KernelError::Exited { ref stderr, .. } if stderr.contains("AddrInUse:")),
        "{error}"
    );
    assert_eq!(launcher.attempts().len(), 3);
    launcher.assert_clean().await;
}

#[tokio::test]
#[ignore = "requires an actual Deno Jupyter executable"]
async fn bind_retry_keeps_the_original_startup_deadline() {
    let mut launcher = Launcher::new("deadline");
    launcher.options.startup_timeout = Duration::from_secs(5);
    let started = Instant::now();
    let error = Kernel::start(launcher.options.clone())
        .await
        .err()
        .expect("second startup never becomes ready");
    assert!(matches!(error, KernelError::Timeout("startup")), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(6500),
        "retry reset the original deadline: {:?}",
        started.elapsed()
    );
    assert_eq!(launcher.attempts().len(), 2);
    launcher.assert_clean().await;
}

#[tokio::test]
#[ignore = "requires an actual Deno Jupyter executable"]
async fn cancellation_during_a_bind_retry_kills_every_owned_process() {
    let launcher = Launcher::new("cancel");
    let cancellation = CancellationToken::new();
    let options = launcher.options.clone();
    let token = cancellation.clone();
    let operation = tokio::spawn(async move { Kernel::start_cancellable(options, token).await });
    launcher.wait_for_retry().await;
    let started = Instant::now();
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), operation)
        .await
        .unwrap()
        .unwrap()
        .err()
        .expect("cancelled startup must fail");
    assert!(matches!(error, KernelError::Cancelled), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(launcher.attempts().len(), 2);
    launcher.assert_clean().await;
    let before = launcher.attempts().len();
    let error = Kernel::start_cancellable(launcher.options.clone(), cancellation)
        .await
        .err()
        .unwrap();
    assert!(matches!(error, KernelError::Cancelled));
    assert_eq!(
        launcher.attempts().len(),
        before,
        "pre-cancelled startup spawned a process"
    );
}

#[tokio::test]
#[ignore = "requires an actual Deno executable"]
async fn ordinary_exit_and_missing_executable_are_not_retried() {
    let launcher = Launcher::new("ordinary");
    let error = Kernel::start(launcher.options.clone())
        .await
        .err()
        .expect("bad CLI flag must fail");
    assert!(
        matches!(error, KernelError::Exited { ref stderr, .. } if stderr.contains("unexpected argument")),
        "{error}"
    );
    assert_eq!(launcher.attempts().len(), 1);
    launcher.assert_clean().await;
    let mut options = launcher.options.clone();
    options.deno = launcher.directory.path().join("nonexistent-deno");
    assert!(
        matches!(Kernel::start(options).await.err().unwrap(), KernelError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
    );
    assert_eq!(launcher.attempts().len(), 1);
}
