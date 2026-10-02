use super::*;

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn recovery_restores_successful_values_and_hooks_without_replaying_failed_work() {
    for ephemeral in [false, true] {
        for failure in ["interrupt", "fatal", "bootstrap"] {
            let home = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let provider = DenoNotebookSessionProvider::new_with_identity(
                provider().deno_program.unwrap(),
                project.path().into(),
                home.path().into(),
                format!("{ephemeral}-{failure}"),
            )
            .with_ephemeral(ephemeral);
            let session = provider.create_session().await.unwrap();
            let delegate = Arc::new(Delegate::default());
            assert_ok(&execute(&session, "var saved = 1; function recreate() { Deno.writeTextFileSync('hook-runs', 'x', {append:true}); globalThis.resource = { [Symbol.dispose]() { Deno.writeTextFileSync('disposed', 'x', {append:true}); } }; }", delegate.clone()).await);
            provider
                .control(NotebookRequest::Pin {
                    names: vec!["recreate".into()],
                    hook: Some(crate::control::NotebookHook::Startup),
                })
                .await
                .unwrap();
            let failed = execute(&session, "saved = 9; Deno.writeTextFileSync('failed-work', 'x', {append:true}); throw new Error('ordinary failure')", delegate.clone()).await;
            assert!(matches!(
                failed,
                RuntimeResponse::Result {
                    error_text: Some(_),
                    ..
                }
            ));
            match failure {
                "interrupt" => {
                    let mut req = request("saved = 17; await tools.echo({block:true})");
                    req.yield_time_ms = Some(20);
                    let started = session.execute(req, delegate.clone(), None).await.unwrap();
                    let id = started.cell_id.clone();
                    assert!(matches!(
                        started.initial_response().await.unwrap(),
                        RuntimeResponse::Yielded { .. }
                    ));
                    assert!(matches!(
                        session.terminate(id).await.unwrap(),
                        WaitOutcome::LiveCell(RuntimeResponse::Terminated { .. })
                    ));
                }
                "fatal" => {
                    let fatal = execute(&session, "saved = 23; Deno.writeTextFileSync('fatal-work', 'x', {append:true}); Deno.exit(17)", delegate.clone()).await;
                    let RuntimeResponse::Result {
                        error_text: Some(error),
                        ..
                    } = fatal
                    else {
                        panic!("{fatal:?}")
                    };
                    assert!(error.contains("Notebook restored"), "{error}");
                    // Fatal recovery completes before the failing cell is observed.
                    assert_eq!(
                        std::fs::read_to_string(project.path().join("hook-runs")).unwrap(),
                        "x"
                    );
                    assert_eq!(
                        std::fs::read_to_string(project.path().join("fatal-work")).unwrap(),
                        "x"
                    );
                }
                "bootstrap" => {
                    let lost =
                        execute(&session, "saved = 33; globalThis = {}", delegate.clone()).await;
                    assert!(
                        matches!(
                            lost,
                            RuntimeResponse::Result {
                                error_text: Some(_),
                                ..
                            }
                        ),
                        "{lost:?}"
                    );
                    assert!(
                        !project.path().join("hook-runs").exists(),
                        "bootstrap recovery must remain lazy"
                    );
                    // Status is an operation too, unlike cold historical diagnostics.
                    let status = provider
                        .control(NotebookRequest::Status { query: None })
                        .await
                        .unwrap();
                    assert!(status.message.contains("Notebook restored"));
                }
                _ => unreachable!(),
            }
            let recovered = execute(
                &session,
                "text(saved); text(typeof resource[Symbol.dispose])",
                delegate,
            )
            .await;
            assert_ok(&recovered);
            assert!(texts(&recovered).contains(&"1"), "{failure}: {recovered:?}");
            assert!(texts(&recovered).contains(&"function"));
            assert_eq!(
                std::fs::read_to_string(project.path().join("failed-work")).unwrap(),
                "x"
            );
            assert_eq!(
                std::fs::read_to_string(project.path().join("hook-runs")).unwrap(),
                "x"
            );
            session.shutdown().await.unwrap();
            assert_eq!(
                std::fs::read_to_string(project.path().join("disposed")).unwrap(),
                "x"
            );
            if ephemeral {
                assert!(!home.path().join("notebook").exists());
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn normal_exit_checkpoints_then_disposes_idle_resources_once() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = DenoNotebookSessionProvider::new_with_identity(
        provider().deno_program.unwrap(),
        project.path().into(),
        home.path().into(),
        "exit".into(),
    );
    let session = provider.create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    assert_ok(&execute(&session, "var saved = 1; var syncResource = {[Symbol.dispose]() { Deno.writeTextFileSync('sync', 'x', {append:true}); saved = 99; }}; globalThis.alias = syncResource; var asyncResource = {[Symbol.asyncDispose]: async function() { await new Promise(r => setTimeout(r, 20)); Deno.writeTextFileSync('async', 'x', {append:true}); }}", delegate.clone()).await);
    // Explicit normal exit, like an explicit checkpoint, may capture live mutations
    // left by an ordinary exception. It does so before disposal mutates values.
    let failed = execute(
        &session,
        "saved = 2; throw new Error('keep live state until normal exit')",
        delegate.clone(),
    )
    .await;
    assert!(matches!(
        failed,
        RuntimeResponse::Result {
            error_text: Some(_),
            ..
        }
    ));
    session.shutdown().await.unwrap();
    session.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(project.path().join("sync")).unwrap(),
        "x"
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("async")).unwrap(),
        "x"
    );
    assert!(
        session
            .execute(
                request("Deno.writeTextFileSync('after-exit', 'x')"),
                delegate.clone(),
                None
            )
            .await
            .is_err()
    );
    assert!(!project.path().join("after-exit").exists());
    let resumed = provider.create_session().await.unwrap();
    let value = execute(&resumed, "text(saved); text(typeof syncResource)", delegate).await;
    assert_ok(&value);
    assert_eq!(texts(&value), ["2", "undefined"]);
    resumed.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn disposal_errors_and_hangs_are_visible_bounded_and_close_the_session() {
    for body in [
        "throw new Error('disposal exploded')",
        "await new Promise(() => {})",
        "while (true) {}",
    ] {
        let project = tempfile::tempdir().unwrap();
        let provider = DenoNotebookSessionProvider::new(
            provider().deno_program.unwrap(),
            project.path().into(),
        );
        let session = provider.create_session().await.unwrap();
        let delegate = Arc::new(Delegate::default());
        assert_ok(&execute(&session, &format!("Deno.writeTextFileSync('pid', String(Deno.pid)); var resource = {{[Symbol.asyncDispose]: async function() {{ {body} }} }}"), delegate.clone()).await);
        let error = tokio::time::timeout(Duration::from_secs(7), session.shutdown())
            .await
            .expect("bounded shutdown")
            .unwrap_err();
        assert!(
            error.contains(if body.starts_with("throw") {
                "disposal exploded"
            } else {
                "timed out"
            }),
            "{error}"
        );
        session.shutdown().await.unwrap();
        assert!(
            session
                .execute(request("text('closed')"), delegate, None)
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            let pid = std::fs::read_to_string(project.path().join("pid")).unwrap();
            assert!(
                !std::process::Command::new("kill")
                    .args(["-0", pid.trim()])
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "kernel process survived teardown"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn shutdown_cancels_active_work_without_disposal_or_recovery() {
    for cleanup in [true, false] {
        let project = tempfile::tempdir().unwrap();
        let provider = DenoNotebookSessionProvider::new(
            provider().deno_program.unwrap(),
            project.path().into(),
        );
        let session = provider.create_session().await.unwrap();
        let delegate = Arc::new(Delegate::default());
        assert_ok(&execute(&session, "var resource = {[Symbol.dispose]() { Deno.writeTextFileSync('disposed', 'x'); }}; function hook() { Deno.writeTextFileSync('hook', 'x', {append:true}); }", delegate.clone()).await);
        provider
            .control(NotebookRequest::Pin {
                names: vec!["hook".into()],
                hook: Some(crate::control::NotebookHook::Startup),
            })
            .await
            .unwrap();
        let mut req = request("await tools.echo({block:true})");
        req.yield_time_ms = Some(100);
        let started = session.execute(req, delegate.clone(), None).await.unwrap();
        assert!(matches!(
            started.initial_response().await.unwrap(),
            RuntimeResponse::Yielded { .. }
        ));
        tokio::time::timeout(Duration::from_secs(7), async {
            if cleanup {
                session.shutdown().await
            } else {
                session.shutdown_without_cleanup().await
            }
        })
        .await
        .unwrap()
        .unwrap();
        assert!(!project.path().join("disposed").exists());
        assert!(!project.path().join("hook").exists());
        assert!(delegate.blocked_cancelled.is_cancelled());
        assert!(
            session
                .execute(request("text('closed')"), delegate, None)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn failed_recovery_hooks_leave_cold_reset_unpin_and_diagnostics_usable() {
    for ephemeral in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let provider = DenoNotebookSessionProvider::new_with_identity(
            provider().deno_program.unwrap(),
            project.path().into(),
            home.path().into(),
            "failed-recovery".into(),
        )
        .with_ephemeral(ephemeral);
        let session = provider.create_session().await.unwrap();
        let delegate = Arc::new(Delegate::default());
        assert_ok(&execute(&session, "function broken() { Deno.writeTextFileSync('hook-runs', 'x', {append:true}); throw new Error('recovery hook failed'); }", delegate.clone()).await);
        provider
            .control(NotebookRequest::Pin {
                names: vec!["broken".into()],
                hook: Some(crate::control::NotebookHook::Startup),
            })
            .await
            .unwrap();
        let failed = execute(&session, "Deno.exit(17)", delegate.clone()).await;
        let RuntimeResponse::Result {
            error_text: Some(error),
            ..
        } = failed
        else {
            panic!("{failed:?}");
        };
        assert!(error.contains("recovery hook failed"), "{error}");
        assert!(error.contains("Notebook recovery failed"), "{error}");
        let diagnostics = provider
            .control(NotebookRequest::Diagnostics)
            .await
            .unwrap();
        assert_eq!(
            if ephemeral {
                &diagnostics.details["runtimeHealth"]
            } else {
                &diagnostics.details["runtime"]["state"]
            },
            "invalidated"
        );
        provider.control(NotebookRequest::Reset).await.unwrap();
        provider
            .control(NotebookRequest::Unpin {
                names: vec!["broken".into()],
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(project.path().join("hook-runs")).unwrap(),
            "x"
        );
        assert_ok(
            &execute(
                &session,
                "text('recovered without retrying the hook')",
                delegate,
            )
            .await,
        );
        assert_eq!(
            std::fs::read_to_string(project.path().join("hook-runs")).unwrap(),
            "x"
        );
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn cold_reset_skips_profile_once_without_forgetting_the_configuration() {
    for invalidated in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let profiles = DenoNotebookSessionProvider::new_with_identity(
            provider().deno_program.unwrap(),
            source.path().into(),
            home.path().into(),
            "profile-source".into(),
        );
        let seed = profiles.create_session().await.unwrap();
        let delegate = Arc::new(Delegate::default());
        assert_ok(
            &execute(
                &seed,
                "function profileHelper() { return 42; }",
                delegate.clone(),
            )
            .await,
        );
        profiles
            .control(NotebookRequest::Save {
                name: "default".into(),
            })
            .await
            .unwrap();
        seed.shutdown().await.unwrap();
        let provider = DenoNotebookSessionProvider::new_with_identity(
            provider().deno_program.unwrap(),
            project.path().into(),
            home.path().into(),
            "reset-profile".into(),
        )
        .with_default_profile(Some("default".into()));
        let session = if invalidated {
            let session = provider.create_session().await.unwrap();
            assert_ok(&execute(&session, "text(profileHelper())", delegate.clone()).await);
            let mut req = request("await new Promise(() => {})");
            req.yield_time_ms = Some(20);
            let started = session.execute(req, delegate.clone(), None).await.unwrap();
            let id = started.cell_id.clone();
            assert!(matches!(
                started.initial_response().await.unwrap(),
                RuntimeResponse::Yielded { .. }
            ));
            session.terminate(id).await.unwrap();
            provider.control(NotebookRequest::Reset).await.unwrap();
            session
        } else {
            provider.control(NotebookRequest::Reset).await.unwrap();
            provider.create_session().await.unwrap()
        };
        let empty = execute(&session, "text(typeof profileHelper)", delegate.clone()).await;
        assert_ok(&empty);
        assert!(texts(&empty).contains(&"undefined"), "{empty:?}");
        provider.control(NotebookRequest::Restart).await.unwrap();
        let loaded = execute(&session, "text(profileHelper())", delegate).await;
        assert_ok(&loaded);
        assert_eq!(texts(&loaded), ["42"]);
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn shutdown_aborts_inflight_recovery_startup_without_starting_another_kernel() {
    let project = tempfile::tempdir().unwrap();
    let provider =
        DenoNotebookSessionProvider::new(provider().deno_program.unwrap(), project.path().into());
    let session = provider.create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    assert_ok(&execute(&session, "async function hang() { Deno.writeTextFileSync('hook-pid', String(Deno.pid)); await new Promise(() => {}); }", delegate.clone()).await);
    provider
        .control(NotebookRequest::Pin {
            names: vec!["hang".into()],
            hook: Some(crate::control::NotebookHook::Startup),
        })
        .await
        .unwrap();
    let mut req = request("Deno.exit(17)");
    req.yield_time_ms = Some(20);
    let started = session.execute(req, delegate.clone(), None).await.unwrap();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !project.path().join("hook-pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fatal recovery started its hook");
    tokio::time::timeout(Duration::from_secs(7), session.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        session
            .execute(request("text('closed')"), delegate, None)
            .await
            .is_err()
    );
    #[cfg(unix)]
    {
        let pid = std::fs::read_to_string(project.path().join("hook-pid")).unwrap();
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .output()
                .unwrap()
                .status
                .success(),
            "recovery kernel survived shutdown"
        );
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn hot_ephemeral_reset_is_authoritative_before_the_next_successful_cell() {
    for failure in ["fatal", "interrupt"] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let provider = DenoNotebookSessionProvider::new_with_identity(
            provider().deno_program.unwrap(),
            project.path().into(),
            home.path().into(),
            "reset-authority".into(),
        )
        .with_ephemeral(true);
        let session = provider.create_session().await.unwrap();
        let delegate = Arc::new(Delegate::default());
        assert_ok(&execute(&session, "var gone = 1", delegate.clone()).await);
        provider.control(NotebookRequest::Reset).await.unwrap();
        if failure == "fatal" {
            let response = execute(&session, "Deno.exit(17)", delegate.clone()).await;
            assert!(matches!(
                response,
                RuntimeResponse::Result {
                    error_text: Some(_),
                    ..
                }
            ));
        } else {
            let mut req = request("await tools.echo({block:true})");
            req.yield_time_ms = Some(20);
            let started = session.execute(req, delegate.clone(), None).await.unwrap();
            let id = started.cell_id.clone();
            assert!(matches!(
                started.initial_response().await.unwrap(),
                RuntimeResponse::Yielded { .. }
            ));
            session.terminate(id).await.unwrap();
        }
        let restored = execute(&session, "text(typeof gone)", delegate).await;
        assert_ok(&restored);
        assert!(
            texts(&restored).contains(&"undefined"),
            "{failure}: {restored:?}"
        );
        session.shutdown().await.unwrap();
        assert!(!home.path().join("notebook").exists());
    }
}
