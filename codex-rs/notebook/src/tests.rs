use super::*;
use crate::cell::text_item;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSessionCellExecutionLimits;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeToolKind;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::ToolDefinition;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_protocol::ToolName;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn status_message_is_complete_bounded_and_does_not_duplicate_binding_inventory() {
    let mut bindings = vec![
        json!({"name":"retainedMarker","pinned":true,"description":"useful helper","bytes":42}),
    ];
    bindings.extend(
        (0..10_000).map(|i| json!({"name":format!("binding{i}"),"description":"x".repeat(256)})),
    );
    let status = json!({"bindings":bindings,"memory":{"heapUsedBytes":42,"heapLimitBytes":1024,"rssBytes":99},"retainedBindings":1,"retainedBytes":42,"userCells":3,"defaultProfile":{"name":"default","loadedBindings":1,"skipped":[{"name":"collision","reason":"existing binding"}]},"npmImportsError":"import inventory failure"});
    let checkpoint = json!({"sessionEntries":1,"projectEntries":1,"conflicts":["concurrentMarker"],"skipped":[{"name":"handle","reason":"runtime-only"}]});
    let result = crate::lifecycle::status_result(
        &status,
        &checkpoint,
        Some("disk failure"),
        Some("*"),
        true,
    );
    assert!(result.message.len() < 16 * 1024);
    for text in [
        "heapUsedBytes",
        "heapLimitBytes",
        "rssBytes",
        "concurrentMarker",
        "runtime-only",
        "disk failure",
        "existing binding",
        "import inventory failure",
        "omitted",
        "3 completed cells",
    ] {
        assert!(
            result.message.contains(text),
            "missing {text}: {}",
            result.message
        );
    }
    assert_eq!(result.message.matches("retainedMarker").count(), 1);
    assert_eq!(result.message.matches("disk failure").count(), 1);
    assert!(result.details["omittedBindings"].as_u64().unwrap() > 0);
}

#[derive(Default)]
struct Delegate {
    calls: Mutex<Vec<CodeModeNestedToolCall>>,
    notifications: Mutex<Vec<(String, CellId, String)>>,
    closed: Mutex<Vec<CellId>>,
    blocked_cancelled: CancellationToken,
}

#[tokio::test]
async fn cold_storage_free_recovery_validates_names_without_starting_or_persisting() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let ephemeral = DenoNotebookSessionProvider::new_with_identity(
        "/nonexistent/notebook-deno".into(),
        cwd.path().into(),
        home.path().into(),
        "cold-ephemeral".into(),
    )
    .with_ephemeral(true);
    let no_identity =
        DenoNotebookSessionProvider::new("/nonexistent/notebook-deno".into(), cwd.path().into());
    for provider in [&ephemeral, &no_identity] {
        let reset = provider.control(NotebookRequest::Reset).await.unwrap();
        assert_eq!(reset.details["reset"], true);
        let unpin = provider
            .control(NotebookRequest::Unpin {
                names: vec!["missing".into()],
            })
            .await
            .unwrap();
        assert_eq!(unpin.details["unpinned"], json!([]));
        assert_eq!(unpin.details["missing"], json!(["missing"]));
        for names in [vec![], vec!["invalid-name".into()], vec!["".into()]] {
            assert!(
                provider
                    .control(NotebookRequest::Unpin { names })
                    .await
                    .unwrap_err()
                    .contains("valid binding names")
            );
        }
        assert!(
            provider
                .control(NotebookRequest::Status { query: None })
                .await
                .unwrap_err()
                .contains("has not been initialized")
        );
    }
    assert!(!home.path().join("notebook").exists());
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}

struct CallbackDrop {
    cancellation: CancellationToken,
    observed: CancellationToken,
}

impl Drop for CallbackDrop {
    fn drop(&mut self) {
        if self.cancellation.is_cancelled() {
            self.observed.cancel();
        }
    }
}

impl CodeModeSessionDelegate for Delegate {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        cancellation: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(invocation.clone());
            if invocation.input == Some(json!({ "block": true })) {
                let _drop = CallbackDrop {
                    cancellation: cancellation.clone(),
                    observed: self.blocked_cancelled.clone(),
                };
                cancellation.cancelled().await;
                return Err("cancelled".to_string());
            }
            Ok(json!({ "input": invocation.input, "name": invocation.tool_name.name }))
        })
    }

    fn notify<'a>(
        &'a self,
        call_id: String,
        cell_id: CellId,
        text: String,
        _cancellation: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async move {
            self.notifications
                .lock()
                .unwrap()
                .push((call_id, cell_id, text));
            Ok(())
        })
    }

    fn cell_closed(&self, id: &CellId) {
        self.closed.lock().unwrap().push(id.clone());
    }
}

fn provider() -> DenoNotebookSessionProvider {
    DenoNotebookSessionProvider::new(
        std::env::var_os("DENO_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("deno")),
        std::env::current_dir().unwrap(),
    )
}

fn request(source: &str) -> ExecuteRequest {
    ExecuteRequest {
        tool_call_id: "outer-call".to_string(),
        source: source.to_string(),
        enabled_tools: vec![ToolDefinition {
            name: "echo".to_string(),
            tool_name: ToolName::namespaced("test", "echo"),
            kind: CodeModeToolKind::Function,
            description: "Echo input".to_string(),
            input_schema: Some(json!({ "type": "object" })),
            input_schema_max_bytes: None,
            output_schema: None,
        }],
        yield_time_ms: Some(5000),
        max_output_tokens: None,
    }
}

async fn execute(
    session: &Arc<dyn CodeModeSession>,
    source: &str,
    delegate: Arc<Delegate>,
) -> RuntimeResponse {
    session
        .execute(request(source), delegate, None)
        .await
        .unwrap()
        .initial_response()
        .await
        .unwrap()
}

fn contents(response: &RuntimeResponse) -> &Vec<FunctionCallOutputContentItem> {
    match response {
        RuntimeResponse::Result { content_items, .. }
        | RuntimeResponse::Yielded { content_items, .. }
        | RuntimeResponse::Terminated { content_items, .. } => content_items,
    }
}

fn texts(response: &RuntimeResponse) -> Vec<&str> {
    contents(response)
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_ok(response: &RuntimeResponse) {
    assert!(
        matches!(
            response,
            RuntimeResponse::Result {
                error_text: None,
                ..
            }
        ),
        "{response:?}"
    );
}

#[tokio::test]
async fn observation_drains_once_and_preemption_does_not_cancel() {
    let delegate = Arc::new(Delegate::default());
    let token = CancellationToken::new();
    let cell = Cell::new(
        CellId::new("cell".into()),
        "call".into(),
        delegate.clone(),
        HashMap::new(),
        token.clone(),
    );
    cell.push(text_item("before"));
    cell.yield_now();
    let first = cell.observe(Duration::from_secs(1), None).await.unwrap();
    assert_eq!(texts(&first), ["before"]);
    let preempt = CancellationToken::new();
    preempt.cancel();
    let second = cell
        .observe(Duration::from_secs(1), Some(preempt))
        .await
        .unwrap();
    assert!(texts(&second).is_empty());
    assert!(!token.is_cancelled());
    cell.push(text_item("after"));
    cell.finish(None);
    cell.finish(None);
    let terminal = cell.observe(Duration::ZERO, None).await.unwrap();
    assert_ok(&terminal);
    assert_eq!(texts(&terminal), ["after"]);
    assert!(cell.observe(Duration::ZERO, None).await.is_none());
    assert_eq!(delegate.closed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pending_byte_limit_preserves_output_below_the_limit() {
    let cell = Cell::new(
        CellId::new("cell".into()),
        "call".into(),
        Arc::new(Delegate::default()),
        HashMap::new(),
        CancellationToken::new(),
    );
    cell.push(text_item("🙂🙂"));
    cell.push(text_item("x".repeat(crate::cell::MAX_PENDING_BYTES + 1)));
    cell.finish(None);
    let response = cell.observe(Duration::ZERO, None).await.unwrap();
    assert_eq!(texts(&response), ["🙂🙂", "[Notebook output truncated]"]);
}

#[tokio::test]
async fn active_status_never_queues_and_mutations_require_settled_execution() {
    let registry = Arc::new(Registry::default());
    {
        let mut state = registry.state.lock().unwrap();
        state.active = Some(CellId::new("running".into()));
        state.status = json!({"bindings":[{"name":"Foo","type":"number"}]});
    }
    let (commands, mut incoming) = mpsc::unbounded_channel();
    let session = Session {
        registry,
        commands,
        resources: Mutex::new(None),
        shutdown_gate: Semaphore::new(1),
        cancellation: CancellationToken::new(),
    };
    let result = session
        .control(NotebookRequest::Status {
            query: Some("f*".into()),
        })
        .await
        .unwrap();
    assert_eq!(result.details["state"], "running");
    assert_eq!(result.details["matches"][0]["name"], "Foo");
    assert!(incoming.try_recv().is_err());
    assert!(
        session
            .control(NotebookRequest::Checkpoint)
            .await
            .unwrap_err()
            .contains("exec is active")
    );
    assert!(incoming.try_recv().is_err());
}

#[tokio::test]
async fn unsupported_resource_limits_are_rejected() {
    let result = provider()
        .create_session_with_limits(CodeModeSessionCellExecutionLimits {
            max_heap_size_bytes: Some(1),
            max_yield_time_ms: None,
        })
        .await;
    assert!(matches!(result, Err(error) if error.contains("does not support resource limits")));
}

#[tokio::test]
async fn bridge_requires_authentication_and_a_live_cell() {
    let registry = Arc::new(Registry::default());
    let mut bridge = Bridge::start(registry).await.unwrap();
    let client = reqwest::Client::new();
    let body = json!({ "cell_id": "missing", "op": "yield" });
    let unauthorized = client
        .post(&bridge.endpoint)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    let missing = client
        .post(&bridge.endpoint)
        .bearer_auth(&bridge.credential)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::BAD_REQUEST);
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn deno_persists_bindings_store_and_delegate_calls_after_errors() {
    let session = provider().create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    let first = execute(&session, "const retained = 41; store('key', { answer: retained }); text(await tools.echo({ value: retained })); await notify('hello'); text('before error'); throw new Error('expected');", delegate.clone()).await;
    assert!(
        matches!(first, RuntimeResponse::Result { error_text: Some(ref error), .. } if error.contains("expected")),
        "{first:?}"
    );
    assert_eq!(
        texts(&first),
        [
            "{\"input\":{\"value\":41},\"name\":\"echo\"}",
            "before error"
        ]
    );
    let second = execute(
        &session,
        "text(retained + 1); text(load('key')); console.log('console output'); 999;",
        delegate.clone(),
    )
    .await;
    assert_ok(&second);
    assert!(texts(&second).contains(&"42"));
    assert!(texts(&second).contains(&"{\"answer\":41}"));
    assert!(!texts(&second).contains(&"999"));
    assert_eq!(
        delegate.calls.lock().unwrap()[0].tool_name,
        ToolName::namespaced("test", "echo")
    );
    assert_eq!(delegate.notifications.lock().unwrap()[0].0, "outer-call");
    assert_eq!(delegate.notifications.lock().unwrap()[0].2, "hello");
    assert_eq!(delegate.closed.lock().unwrap().len(), 2);
    session.shutdown().await.unwrap();
    session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn deno_yields_waits_drains_terminal_output_and_isolates_sessions() {
    let session = provider().create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    let mut initial_request = request(
        "globalThis.onlyHere = true; text('first'); await yield_control(); await new Promise(r => setTimeout(r, 100)); text('last output exceeds the original exec budget');",
    );
    initial_request.max_output_tokens = Some(1);
    let first = session
        .execute(initial_request, delegate.clone(), None)
        .await
        .unwrap()
        .initial_response()
        .await
        .unwrap();
    let id = match &first {
        RuntimeResponse::Yielded { cell_id, .. } => cell_id.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(texts(&first), ["first"]);
    assert!(
        session
            .execute(request("text('rejected')"), delegate.clone(), None)
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    let final_response = RuntimeResponse::from(
        session
            .wait(
                WaitRequest {
                    cell_id: id.clone(),
                    yield_time_ms: 5000,
                },
                None,
            )
            .await
            .unwrap(),
    );
    assert_ok(&final_response);
    assert_eq!(
        texts(&final_response),
        ["last output exceeds the original exec budget"]
    );
    assert!(matches!(
        session
            .wait(
                WaitRequest {
                    cell_id: id,
                    yield_time_ms: 0
                },
                None
            )
            .await
            .unwrap(),
        WaitOutcome::MissingCell(_)
    ));
    let isolated = provider().create_session().await.unwrap();
    let response = execute(&isolated, "text(typeof globalThis.onlyHere)", delegate).await;
    assert_ok(&response);
    assert_eq!(texts(&response), ["undefined"]);
    isolated.shutdown().await.unwrap();
    session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn deno_preemption_and_termination_are_bounded() {
    let session = provider().create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    let preempt = CancellationToken::new();
    preempt.cancel();
    let started = session
        .execute(
            request("await tools.echo({ block: true })"),
            delegate.clone(),
            Some(preempt),
        )
        .await
        .unwrap();
    let id = started.cell_id.clone();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    let outcome = tokio::time::timeout(Duration::from_secs(8), session.terminate(id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        outcome,
        WaitOutcome::LiveCell(RuntimeResponse::Terminated { .. })
    ));
    assert!(
        session
            .execute(request("text('cannot restart')"), delegate.clone(), None)
            .await
            .is_err()
    );
    assert_eq!(delegate.closed.lock().unwrap().len(), 1);
    session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn unpin_runtime_only_replacement_preserves_project_value_without_repinning() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = DenoNotebookSessionProvider::new_with_identity(
        provider().deno_program.unwrap(),
        project.path().to_path_buf(),
        home.path().to_path_buf(),
        "unpin".into(),
    );
    let session = provider.create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    assert_ok(&execute(&session, "let promotedLexical = 42;", delegate.clone()).await);
    provider
        .control(NotebookRequest::Pin {
            names: vec!["promotedLexical".into()],
            hook: None,
        })
        .await
        .unwrap();
    assert_ok(
        &execute(
            &session,
            "promotedLexical = Promise.resolve(7);",
            delegate.clone(),
        )
        .await,
    );
    provider
        .control(NotebookRequest::Unpin {
            names: vec!["promotedLexical".into()],
        })
        .await
        .unwrap();
    provider.control(NotebookRequest::Restart).await.unwrap();
    let status = provider
        .control(NotebookRequest::Status {
            query: Some("promotedLexical".into()),
        })
        .await
        .unwrap();
    assert_ne!(status.details["matches"][0]["pinned"], true);
    assert_eq!(
        texts(&execute(&session, "text(promotedLexical);", delegate).await),
        ["42"]
    );
    provider
        .control(NotebookRequest::Release {
            names: vec!["promotedLexical".into()],
        })
        .await
        .unwrap();
    session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn deno_images_and_unawaited_tools_close_with_the_original_cell() {
    let session = provider().create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    let response = execute(&session, "void tools.echo({ block: true }).catch(() => {}); image({type:'image',mimeType:'image/png',data:'AA=='}); generatedImage({image_url:'data:image/png;base64,AA==',output_hint:'hint'}); await new Promise(r => setTimeout(r, 100));", delegate.clone()).await;
    assert_ok(&response);
    assert_eq!(contents(&response).len(), 3);
    assert_eq!(texts(&response), ["hint"]);
    tokio::time::timeout(
        Duration::from_secs(1),
        delegate.blocked_cancelled.cancelled(),
    )
    .await
    .unwrap();
    assert!(
        matches!(&contents(&response)[0], FunctionCallOutputContentItem::InputImage { image_url, .. } if image_url == "data:image/png;base64,AA==")
    );
    // Captured tools retain the old cell identity and cannot invoke the next cell's delegate.
    let capture = execute(
        &session,
        "globalThis.staleTool = tools.echo; text('capture');",
        delegate.clone(),
    )
    .await;
    assert_ok(&capture);
    let stale = execute(
        &session,
        "try { await staleTool({ value: 1 }); } catch (e) { text(e.message); }",
        delegate.clone(),
    )
    .await;
    assert_ok(&stale);
    assert!(texts(&stale)[0].contains("unknown or closed"));
    assert_eq!(delegate.calls.lock().unwrap().len(), 1);
    session.shutdown().await.unwrap();
}
