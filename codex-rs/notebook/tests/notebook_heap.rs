//! Heap configuration against the real Deno kernel, without executing user code.

use std::path::PathBuf;

use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_notebook::DenoNotebookSessionProvider;
use codex_notebook::NotebookRequest;

#[tokio::test]
async fn invalid_heap_is_reported_before_runtime_resolution() {
    let cwd = tempfile::tempdir().unwrap();
    for heap in [0, 255, 65_537, u32::MAX] {
        let provider = DenoNotebookSessionProvider::new(
            PathBuf::from("must-not-start-deno"),
            cwd.path().to_path_buf(),
        )
        .with_max_heap_mib(heap);
        assert!(
            provider
                .availability()
                .unwrap_err()
                .contains("256 through 65536")
        );
        assert!(
            provider
                .create_session()
                .await
                .err()
                .unwrap()
                .contains("256 through 65536")
        );
        assert!(
            provider
                .control(NotebookRequest::List { query: None })
                .await
                .is_err()
        );
    }
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn configured_and_default_heap_reach_real_deno() {
    let cwd = tempfile::tempdir().unwrap();
    for configured in [None, Some(768)] {
        let mut provider = DenoNotebookSessionProvider::new(
            std::env::var_os("DENO_PROGRAM")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("deno")),
            cwd.path().to_path_buf(),
        );
        if let Some(heap) = configured {
            provider = provider.with_max_heap_mib(heap);
        }
        let session = provider.create_session().await.unwrap();
        let status = provider
            .control(NotebookRequest::Status { query: None })
            .await
            .unwrap();
        let limit = status.details["memory"]["heapLimitBytes"].as_u64().unwrap();
        let requested = u64::from(configured.unwrap_or(4096)) * 1024 * 1024;
        // V8's reported total includes young-generation space beyond the old-space flag.
        assert!(
            (requested..requested + 128 * 1024 * 1024).contains(&limit),
            "{status:?}"
        );
        session.shutdown().await.unwrap();
    }
}
