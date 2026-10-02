use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use super::DenoNotebookSessionProvider;
use super::Journal;
use super::NotebookRequest;
use super::PathBuf;
use super::PersistenceBudget;

const MIB: usize = 1024 * 1024;

#[tokio::test]
async fn configured_budget_covers_large_cold_profiles_and_metadata_recovery() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join(".git")).unwrap();
    let make_provider = |heap| {
        DenoNotebookSessionProvider::new_with_identity(
            PathBuf::from("must-not-start-deno"),
            cwd.path().to_path_buf(),
            home.path().to_path_buf(),
            "thread".into(),
        )
        .with_max_heap_mib(heap)
    };
    // This payload exceeds the old 512 MiB heap's 64 MiB persistence limit.
    let large = make_provider(768);
    let snapshot = json!({
        "deno":"old-deno-provenance", "v8":"old-v8-provenance", "skipped":[],
        "entries":[{"name":"large", "kind":"value", "pinned":true,
            "data":STANDARD.encode(vec![1; 66 * MIB]), "length":66 * MIB}]
    });
    let mut store = large
        .durable_store(large.identity.as_ref().unwrap())
        .unwrap();
    store.checkpoint(&snapshot, &snapshot).await.unwrap();
    store.save_profile("large", &snapshot).await.unwrap();
    drop(snapshot);

    let listed = large
        .control(NotebookRequest::List { query: None })
        .await
        .unwrap();
    assert_eq!(listed.details["profiles"][0]["name"], "large");
    assert_eq!(store.binding_names().await.unwrap(), ["large"]);
    let unpinned = large
        .control(NotebookRequest::Unpin {
            names: vec!["large".into()],
        })
        .await
        .unwrap();
    assert_eq!(unpinned.details["unpinned"], json!(["large"]));
    // Cold controls must use the configured budget, not an unrelated ceiling either.
    let small = make_provider(256);
    assert!(
        small
            .control(NotebookRequest::List { query: None })
            .await
            .is_err()
    );
    assert!(
        small
            .control(NotebookRequest::Unpin {
                names: vec!["large".into()]
            })
            .await
            .is_err()
    );
    assert!(large.session.lock().unwrap().upgrade().is_none());
}

#[tokio::test]
async fn historical_diagnostics_read_large_journals_with_the_configured_budget() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let budget = PersistenceBudget::from_heap_mib(Some(768));
    let journal =
        Journal::with_budget(home.path(), cwd.path(), "thread", budget.payload_bytes()).unwrap();
    journal.append("first", "", "ok", None, &[]).unwrap();
    // Large outputs/metadata need not become LSP source. An empty code history never
    // starts Deno, so the nonexistent executable also guards the cold boundary.
    let document = json!({"nbformat":4, "nbformat_minor":5,
        "metadata":{"retainedOutput":"x".repeat(66 * MIB)}, "cells":[]});
    std::fs::write(&journal.path, serde_json::to_vec(&document).unwrap()).unwrap();
    drop(document);
    let large = crate::diagnostics::diagnostics_with_runtime(
        std::path::Path::new("must-not-start-deno"),
        cwd.path(),
        home.path(),
        "thread",
        "not_started",
        &[],
        budget,
    )
    .await
    .unwrap();
    assert!(large.details.get("error").is_none(), "{large:?}");
    let small = crate::diagnostics::diagnostics_with_runtime(
        std::path::Path::new("must-not-start-deno"),
        cwd.path(),
        home.path(),
        "thread",
        "not_started",
        &[],
        PersistenceBudget::from_heap_mib(Some(256)),
    )
    .await
    .unwrap();
    assert!(
        small.details["error"]
            .as_str()
            .unwrap()
            .contains("exceeds budget")
    );
}
