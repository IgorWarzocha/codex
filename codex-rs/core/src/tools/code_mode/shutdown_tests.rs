use super::*;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context_with_auth_and_config_and_rx;
use codex_protocol::models::PermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;

#[cfg(unix)]
#[tokio::test]
async fn notebook_prewarm_checks_authority_before_launching_configured_executable() {
    use std::os::unix::fs::PermissionsExt;

    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let executable = project.path().join("untrusted-deno");
    std::fs::write(&executable, "#!/bin/sh\nprintf x > launched\nexit 1\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (session, mut turn) = crate::session::tests::make_session_and_context().await;
    let mut config = (*turn.config).clone();
    config.cwd = AbsolutePathBuf::from_absolute_path(project.path()).unwrap();
    config.codex_home = AbsolutePathBuf::from_absolute_path(home.path()).unwrap();
    config.code_mode.runtime = codex_features::CodeModeRuntime::Notebook;
    config.code_mode.deno_program = Some(executable);
    config
        .permissions
        .set_permission_profile(PermissionProfile::workspace_write())
        .unwrap();
    turn.config = Arc::new(config);
    let service = CodeModeService::new(
        ThreadId::new(),
        Arc::new(codex_code_mode::DisabledCodeModeSessionProvider),
        &turn.config,
        session.services.executed_tool_calls.clone(),
    );
    service.prewarm(&turn).await.unwrap();
    assert!(!project.path().join("launched").exists());
    assert!(service.session.get().is_none());
    assert!(!home.path().join("notebook").exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a local Deno Jupyter executable"]
async fn notebook_permission_revocation_never_runs_disposal_or_startup_hooks() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (session, turn, _rx) = make_session_and_context_with_auth_and_config_and_rx(
        codex_login::CodexAuth::from_api_key("Test API Key"),
        Vec::new(),
        |config| {
            config.codex_home = AbsolutePathBuf::from_absolute_path(home.path()).unwrap();
            config.cwd = AbsolutePathBuf::from_absolute_path(project.path()).unwrap();
            config.code_mode.runtime = codex_features::CodeModeRuntime::Notebook;
            config.code_mode.deno_program = Some(
                std::env::var_os("DENO_PROGRAM")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "deno".into()),
            );
            config
                .permissions
                .set_permission_profile(PermissionProfile::Disabled)
                .unwrap();
        },
    )
    .await;
    let step = StepContext::for_test(turn.clone());
    let service = CodeModeService::new(
        ThreadId::new(),
        Arc::new(codex_code_mode::DisabledCodeModeSessionProvider),
        &turn.config,
        session.services.executed_tool_calls.clone(),
    );
    service.prewarm(&step.turn).await.unwrap();
    assert!(service.session.get().is_none());
    assert!(!home.path().join("notebook").exists());
    let started = service.execute(codex_code_mode::ExecuteRequest {
        tool_call_id: "seed-disposal".into(),
        source: "var resource = {[Symbol.dispose]() { Deno.writeTextFileSync('disposed', 'x'); }}; function startup() { Deno.writeTextFileSync('started', 'x', {append:true}); }".into(),
        enabled_tools: Vec::new(), yield_time_ms: Some(10_000), max_output_tokens: None,
    }, step.clone()).await.unwrap();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Result {
            error_text: None,
            ..
        }
    ));
    service
        .control_notebook(
            codex_notebook::NotebookRequest::Pin {
                names: vec!["startup".into()],
                hook: Some(codex_notebook::NotebookHook::Startup),
            },
            &step,
        )
        .await
        .unwrap();

    drop(step);
    let mut restricted_turn = Arc::try_unwrap(turn).ok().expect("test owns turn");
    let mut restricted_config = (*restricted_turn.config).clone();
    restricted_config
        .permissions
        .set_permission_profile(PermissionProfile::workspace_write())
        .unwrap();
    restricted_turn.config = Arc::new(restricted_config);
    let restricted = StepContext::for_test(Arc::new(restricted_turn));
    let error = service
        .validate_notebook_access(&restricted)
        .await
        .unwrap_err();
    assert!(error.contains("danger-full-access"), "{error}");
    assert!(!project.path().join("disposed").exists());
    assert!(!project.path().join("started").exists());
    assert!(
        service
            .session()
            .await
            .err()
            .unwrap()
            .contains("shutting down")
    );
    // A later normal shutdown cannot upgrade an already revoked session to cleanup.
    service.shutdown().await.unwrap();
    assert!(!project.path().join("disposed").exists());

    // A restricted service must neither spawn a bare executable nor restore this hook.
    let cold = CodeModeService::new(
        ThreadId::new(),
        Arc::new(codex_code_mode::DisabledCodeModeSessionProvider),
        &restricted.turn.config,
        session.services.executed_tool_calls.clone(),
    );
    cold.prewarm(&restricted.turn).await.unwrap();
    assert!(cold.session.get().is_none());
    let error = cold
        .execute(
            codex_code_mode::ExecuteRequest {
                tool_call_id: "unauthorized".into(),
                source: "Deno.writeTextFileSync('unauthorized', 'x')".into(),
                enabled_tools: Vec::new(),
                yield_time_ms: Some(10_000),
                max_output_tokens: None,
            },
            restricted,
        )
        .await
        .err()
        .unwrap();
    assert!(error.contains("danger-full-access"), "{error}");
    assert!(cold.session.get().is_none());
    assert!(!project.path().join("started").exists());
    assert!(!project.path().join("unauthorized").exists());
    cold.shutdown().await.unwrap();
}
