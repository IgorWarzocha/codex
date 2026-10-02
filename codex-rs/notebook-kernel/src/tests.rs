use super::*;
#[cfg(unix)]
use crate::ExecutionStatus;

#[cfg(unix)]
fn stdout(result: &ExecutionResult) -> String {
    result
        .outputs
        .iter()
        .filter_map(|o| match o {
            Output::Stream {
                name: crate::StreamName::Stdout,
                text,
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(unix)]
fn child_source(directory: &std::path::Path, awaited: bool) -> Result<String, serde_json::Error> {
    let script = "while [ ! -e \"$1\" ]; do sleep 0.02; done; printf survived > \"$2\"";
    let args = serde_json::to_string(&[
        "-c".to_string(),
        script.to_string(),
        "sh".to_string(),
        directory.join("release").to_string_lossy().into_owned(),
        directory.join("late_write").to_string_lossy().into_owned(),
    ])?;
    Ok(format!(
        "{{ const child = new Deno.Command('/bin/sh', {{ args: {args}, stdout: 'null', stderr: 'null' }}).spawn(); console.log(child.pid); {} }}",
        if awaited { "await child.status;" } else { "" }
    ))
}

#[cfg(unix)]
async fn assert_child_stopped(
    pid: u32,
    directory: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let stopped = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = tokio::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .await?;
            if !status.status.success()
                && (status.status.code() != Some(1) || !status.stderr.is_empty())
            {
                return Err(std::io::Error::other(
                    String::from_utf8_lossy(&status.stderr).into_owned(),
                ));
            }
            let state = String::from_utf8_lossy(&status.stdout);
            // An orphan zombie cannot perform work. Its final reap belongs to the OS parent.
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                return Ok::<_, std::io::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // Release even on failure so a rejected implementation cannot leave an endless probe.
    std::fs::write(directory.join("release"), "")?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !directory.join("late_write").exists(),
        "Deno descendant survived cleanup"
    );
    stopped??;
    Ok(())
}

/// Real protocol and process lifecycle oracle. No Python, Jupyter installation, or model.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires a Deno executable (DENO_KERNEL_TEST_BIN or deno on PATH)"]
async fn real_deno_state_isolation_errors_and_bounded_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let options = KernelOptions {
        deno: std::env::var_os("DENO_KERNEL_TEST_BIN")
            .map(Into::into)
            .unwrap_or_else(|| "deno".into()),
        max_heap_mib: Some(128),
        execute_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(1),
        ..KernelOptions::default()
    };
    let mut a = Kernel::start(options.clone()).await?;
    let mut b = Kernel::start(options.clone()).await?;
    let private_directory =
        std::path::PathBuf::from(stdout(&a.execute("console.log(Deno.cwd())").await?).trim());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&private_directory)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(private_directory.join("connection.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        stdout(
            &a.execute("let counter: number = 40; console.log(counter)")
                .await?
        ),
        "40\n"
    );
    assert_eq!(
        stdout(&a.execute("counter += 2; console.log(counter)").await?),
        "42\n"
    );
    assert_eq!(
        stdout(&b.execute("console.log(typeof counter)").await?),
        "undefined\n"
    );

    let error = a.execute("throw new Error('controlled exception')").await?;
    assert_eq!(error.status, ExecutionStatus::Error);
    assert!(
        error
            .error
            .as_ref()
            .is_some_and(|e| e.value.contains("controlled exception"))
    );
    assert_eq!(stdout(&a.execute("console.log(counter)").await?), "42\n");

    let displayed = a.execute("await Deno.jupyter.broadcast('display_data', { data: { 'text/plain': 'displayed' }, metadata: {} })").await?;
    assert!(
        displayed.outputs.iter().any(
            |o| matches!(o, Output::Display { data, .. } if data["text/plain"] == "displayed")
        )
    );
    a.options.max_output_bytes = 64;
    let capped = a
        .execute("console.log('x'.repeat(1000)); throw new Error('still reported')")
        .await?;
    assert!(capped.output_truncated);
    assert!(
        capped
            .error
            .is_some_and(|e| e.value.contains("still reported"))
    );
    a.options.max_output_bytes = 4096;

    // Receipt of output while JS is still blocked proves streaming precedes completion.
    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    let cancelled_child = tempfile::tempdir()?;
    let source = child_source(cancelled_child.path(), true)?;
    let mut child_pid = None;
    let cancelled = a
        .execute_streaming(&source, cancellation, |output| {
            if let Output::Stream { text, .. } = output
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                child_pid = Some(pid);
                trigger.cancel();
            }
        })
        .await;
    let child_pid = child_pid.expect("child PID must stream before completion");
    assert!(matches!(cancelled, Err(KernelError::Cancelled)));
    assert_child_stopped(child_pid, cancelled_child.path()).await?;
    assert!(a.process.child.try_wait()?.is_some());
    assert!(!private_directory.exists());
    assert!(matches!(a.execute("1").await, Err(KernelError::Closed)));
    a.shutdown().await?;
    a.shutdown().await?;

    b.options.execute_timeout = Duration::from_millis(200);
    assert!(matches!(
        b.execute("while (true) {}").await,
        Err(KernelError::Timeout("execution"))
    ));
    assert!(b.process.child.try_wait()?.is_some());
    b.shutdown().await?;

    // A separate graceful stop verifies shutdown_request plus the force-stop fallback.
    let mut c = Kernel::start(options.clone()).await?;
    let shutdown_child = tempfile::tempdir()?;
    let spawned = c
        .execute(&child_source(shutdown_child.path(), false)?)
        .await?;
    let child_pid = stdout(&spawned).trim().parse()?;
    c.shutdown().await?;
    assert!(c.process.child.try_wait()?.is_some());
    assert_child_stopped(child_pid, shutdown_child.path()).await?;

    let mut dropped = Kernel::start(options.clone()).await?;
    let dropped_child = tempfile::tempdir()?;
    let source = child_source(dropped_child.path(), true)?;
    let (started, mut received) = tokio::sync::mpsc::unbounded_channel();
    // Drop the active future after observing the actual descendant, without a timing guess.
    let child_pid = {
        let execute = dropped.execute_streaming(&source, CancellationToken::new(), |output| {
            if let Output::Stream { text, .. } = output
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                let _ = started.send(pid);
            }
        });
        tokio::pin!(execute);
        tokio::select! {
            pid = received.recv() => pid.expect("child PID must stream before completion"),
            result = &mut execute => panic!("awaited child completed before release: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("descendant did not start"),
        }
    };
    assert!(matches!(
        dropped.execute("1").await,
        Err(KernelError::Closed)
    ));
    dropped.shutdown().await?;
    assert!(dropped.process.child.try_wait()?.is_some());
    assert_child_stopped(child_pid, dropped_child.path()).await?;

    let mut emergency = Kernel::start(options.clone()).await?;
    let emergency_child = tempfile::tempdir()?;
    let spawned = emergency
        .execute(&child_source(emergency_child.path(), false)?)
        .await?;
    let child_pid = stdout(&spawned).trim().parse()?;
    drop(emergency);
    assert_child_stopped(child_pid, emergency_child.path()).await?;

    let mut invalid = options;
    invalid
        .env
        .insert("DENO_V8_FLAGS".into(), "--invalid-codex-test-flag".into());
    assert!(matches!(
        Kernel::start(invalid).await,
        Err(KernelError::Exited { .. })
    ));
    Ok(())
}

#[cfg(not(unix))]
#[tokio::test]
async fn unsupported_platform_does_not_spawn() {
    assert!(matches!(
        Kernel::start(KernelOptions::default()).await,
        Err(KernelError::UnsupportedPlatform)
    ));
}

#[test]
fn correlation_rejects_unrelated_and_parentless_messages() {
    let request: JupyterMessage = ExecuteRequest::new("1".into()).into();
    let response = jupyter_protocol::Status {
        execution_state: ExecutionState::Idle,
    }
    .as_child_of(&request);
    assert!(correlated(&response, &request.header.msg_id));
    assert!(!correlated(&response, "other-cell"));
    assert!(!correlated(&request, &request.header.msg_id));
}
