use super::*;
use codex_notebook_kernel::Output;
use serde_json::Value;

fn provider(cwd: &std::path::Path, home: &std::path::Path) -> DenoNotebookSessionProvider {
    DenoNotebookSessionProvider::new_with_identity(
        std::env::var_os("DENO_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| "deno".into()),
        cwd.into(),
        home.into(),
        "prewarm".into(),
    )
}

async fn probe(kernel: &mut Kernel, expression: &str) -> Value {
    let result = kernel
        .execute(&format!("console.log(JSON.stringify({expression}))"))
        .await
        .unwrap();
    assert!(
        crate::session::result_error(&result).is_none(),
        "{result:?}"
    );
    let output: String = result
        .outputs
        .iter()
        .filter_map(|output| match output {
            Output::Stream { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    serde_json::from_str(output.trim()).unwrap()
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn prewarm_defers_saved_code_profiles_and_hooks_and_authorized_start_reuses_kernel() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let provider = provider(cwd.path(), home.path()).with_default_profile(Some("seed".into()));
    let store = provider
        .durable_store(provider.identity.as_ref().unwrap())
        .unwrap();
    let bootstrap = include_str!("../bootstrap.js")
        .replace("__PLAIN_COMMAND_OUTPUT__", "false")
        .replace("__ENDPOINT__", "\"http://127.0.0.1:1\"")
        .replace("__CREDENTIAL__", "\"test\"");
    let mut seed = Lifecycle::start(
        provider.kernel_options(provider.resolved_deno().await.unwrap()),
        None,
        bootstrap,
        Some(store),
        None,
        false,
    )
    .await
    .unwrap();
    probe(
        seed.kernel.as_mut().unwrap(),
        "(globalThis.profileOnly = 7)",
    )
    .await;
    seed.control(NotebookRequest::Save {
        name: "seed".into(),
    })
    .await
    .unwrap();
    seed.control(NotebookRequest::Prune {
        query: "profileOnly".into(),
    })
    .await
    .unwrap();
    std::fs::write(
        cwd.path().join("startup-import.ts"),
        "Deno.writeTextFileSync('import-runs', 'x', {append:true});",
    )
    .unwrap();
    probe(seed.kernel.as_mut().unwrap(), "(globalThis.saved = 41, globalThis.startup = async () => { await import('./startup-import.ts'); Deno.writeTextFileSync('hook-runs', String(Deno.pid) + '\\n', {append:true}); }, true)").await;
    seed.control(NotebookRequest::Pin {
        names: vec!["startup".into()],
        hook: Some(crate::control::NotebookHook::Startup),
    })
    .await
    .unwrap();
    seed.checkpoint(&[]).await.unwrap();
    seed.shutdown().await.unwrap();

    provider.prewarm().await.unwrap();
    provider.prewarm().await.unwrap();
    assert!(provider.session.lock().unwrap().upgrade().is_none());
    assert!(!cwd.path().join("hook-runs").exists());
    assert!(!cwd.path().join("import-runs").exists());
    let mut kernel = provider.prewarmed_kernel.lock().unwrap().take().unwrap();
    let state = probe(&mut kernel, "[Deno.pid, typeof saved, typeof startup, typeof profileOnly, typeof globalThis.__codexNotebookState]").await;
    assert_eq!(state.as_array().unwrap()[1..], vec![json!("undefined"); 4]);
    *provider.prewarmed_kernel.lock().unwrap() = Some(kernel);

    // Session creation is reached only after the core service validates Notebook access.
    let session = provider.create_session().await.unwrap();
    let status = provider
        .control(NotebookRequest::Status { query: None })
        .await
        .unwrap();
    let bindings = status.details["bindings"].as_array().unwrap();
    assert!(bindings.iter().any(|b| b["name"] == "saved"));
    assert!(bindings.iter().any(|b| b["name"] == "profileOnly"));
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("import-runs")).unwrap(),
        "x"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("hook-runs")).unwrap(),
        format!("{}\n", state[0])
    );
    assert!(provider.prewarmed_kernel.lock().unwrap().is_none());
    let again = provider.create_session().await.unwrap();
    assert!(Arc::ptr_eq(&session, &again));
    session.shutdown_without_cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable"]
async fn unused_prewarm_is_reaped_without_storage_or_hooks() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let provider = provider(cwd.path(), home.path());
    provider.prewarm().await.unwrap();
    assert!(!home.path().join("notebook").exists());
    let mut kernel = provider.prewarmed_kernel.lock().unwrap().take().unwrap();
    probe(
        &mut kernel,
        "(setInterval(() => Deno.writeTextFileSync('heartbeat', 'x', {append:true}), 20), true)",
    )
    .await;
    *provider.prewarmed_kernel.lock().unwrap() = Some(kernel);
    tokio::time::sleep(Duration::from_millis(100)).await;
    provider.shutdown_prewarm().await.unwrap();
    let heartbeat = std::fs::read(cwd.path().join("heartbeat")).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        std::fs::read(cwd.path().join("heartbeat")).unwrap(),
        heartbeat
    );
    provider.prewarm().await.unwrap();
    assert!(provider.prewarmed_kernel.lock().unwrap().is_none());
    assert!(!home.path().join("notebook").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_cancels_partial_prewarm_and_reaps_its_process() {
    use std::os::unix::fs::PermissionsExt;

    let cwd = tempfile::tempdir().unwrap();
    let program = cwd.path().join("stalled-deno");
    // An owned process that never opens Jupyter sockets exercises partial-startup cleanup.
    std::fs::write(
        &program,
        "#!/bin/sh\nwhile :; do printf x >> heartbeat; sleep 0.02; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let provider = Arc::new(DenoNotebookSessionProvider::new(program, cwd.path().into()));
    let warm_provider = provider.clone();
    let warmup = tokio::spawn(async move { warm_provider.prewarm().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !cwd.path().join("heartbeat").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), provider.shutdown_prewarm())
        .await
        .unwrap()
        .unwrap();
    assert!(warmup.await.unwrap().is_err());
    let heartbeat = std::fs::read(cwd.path().join("heartbeat")).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        std::fs::read(cwd.path().join("heartbeat")).unwrap(),
        heartbeat
    );
    assert!(provider.prewarmed_kernel.lock().unwrap().is_none());
}
