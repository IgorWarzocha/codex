//! Cross-session acceptance against real Deno, with isolated project and state directories.

#![cfg(test)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionProvider;
use codex_code_mode_protocol::CodeModeToolKind;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::NotificationFuture;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::ToolDefinition;
use codex_code_mode_protocol::ToolInvocationFuture;
use codex_notebook::DenoNotebookSessionProvider;
use codex_notebook::NotebookControlResult;
use codex_protocol::ToolName;
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Echo {
    calls: Mutex<Vec<Value>>,
}

impl CodeModeSessionDelegate for Echo {
    fn invoke_tool<'a>(
        &'a self,
        call: CodeModeNestedToolCall,
        _cancellation: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            let input = call.input.unwrap_or(Value::Null);
            self.calls
                .lock()
                .expect("test call log poisoned")
                .push(input.clone());
            Ok(input)
        })
    }

    fn notify<'a>(
        &'a self,
        _call: String,
        _cell: CellId,
        _text: String,
        _cancellation: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell: &CellId) {}
}

fn provider(home: &Path, cwd: &Path, thread: &str) -> DenoNotebookSessionProvider {
    DenoNotebookSessionProvider::new_with_identity(
        std::env::var_os("DENO_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("deno")),
        cwd.to_path_buf(),
        home.to_path_buf(),
        thread.to_string(),
    )
}

fn request(source: &str) -> ExecuteRequest {
    ExecuteRequest {
        tool_call_id: "acceptance".into(),
        source: source.into(),
        enabled_tools: vec![ToolDefinition {
            name: "echo".into(),
            tool_name: ToolName::plain("echo"),
            kind: CodeModeToolKind::Function,
            description: "Return input".into(),
            input_schema: Some(json!({ "type": "object" })),
            input_schema_max_bytes: None,
            output_schema: None,
        }],
        yield_time_ms: Some(10_000),
        max_output_tokens: None,
    }
}

async fn execute(session: &Arc<dyn CodeModeSession>, source: &str, echo: Arc<Echo>) -> Vec<String> {
    let response = session
        .execute(request(source), echo, None)
        .await
        .expect("start acceptance cell")
        .initial_response()
        .await
        .expect("observe acceptance cell");
    match response {
        RuntimeResponse::Result {
            content_items,
            error_text: None,
            ..
        } => content_items
            .into_iter()
            .filter_map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => Some(text),
                _ => None,
            })
            .collect(),
        other => panic!("cell did not complete successfully: {other:?}"),
    }
}

async fn control(provider: &DenoNotebookSessionProvider, value: Value) -> NotebookControlResult {
    provider
        .control(serde_json::from_value(value).expect("valid acceptance request"))
        .await
        .expect("acceptance management operation")
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn private_resume_project_isolation_and_profiles_never_replay_cells() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let a = provider(home.path(), project.path(), "thread-a");
    let b = provider(home.path(), project.path(), "thread-b");
    let a_session = a.create_session().await.unwrap();
    let b_session = b.create_session().await.unwrap();
    let echo = Arc::new(Echo::default());
    execute(
        &a_session,
        r#"
let privateNumber = 41n;
let privateMap = new Map([["answer", 42]]);
globalThis.projectValue = { count: 1 };
function helper(value) { return value + 1; }
helper.description = "Increment a number";
await Deno.writeTextFile("effect.txt", "once", { append: true });
"#,
        echo.clone(),
    )
    .await;
    control(&a, json!({"action":"pin","names":["helper"]})).await;
    control(&a, json!({"action":"checkpoint"})).await;
    control(&a, json!({"action":"save","name":"acceptance"})).await;
    assert_eq!(
        execute(
            &b_session,
            "text(typeof projectValue); text(typeof privateNumber);",
            echo.clone()
        )
        .await,
        ["undefined", "undefined"]
    );
    let collision = a
        .control(serde_json::from_value(json!({"action":"load","name":"acceptance"})).unwrap())
        .await;
    assert!(
        collision.is_err(),
        "profile must not overwrite existing bindings"
    );
    assert_eq!(
        execute(&a_session, "text(projectValue.count);", echo.clone()).await,
        ["1"]
    );
    a_session.shutdown().await.unwrap();
    b_session.shutdown().await.unwrap();

    let resumed = provider(home.path(), project.path(), "thread-a");
    let resumed_session = resumed.create_session().await.unwrap();
    assert_eq!(execute(&resumed_session, "text(String(privateNumber)); text(privateMap.get('answer')); text(helper(41)); text(helper.description);", echo.clone()).await, ["41", "42", "42", "Increment a number"]);
    assert_eq!(
        std::fs::read_to_string(project.path().join("effect.txt")).unwrap(),
        "once"
    );

    let fresh = provider(home.path(), project.path(), "fresh-thread");
    let fresh_session = fresh.create_session().await.unwrap();
    assert_eq!(
        execute(
            &fresh_session,
            "text(projectValue.count); text(helper(1)); text(typeof privateNumber);",
            echo.clone()
        )
        .await,
        ["1", "2", "undefined"]
    );

    let unrelated = tempfile::tempdir().unwrap();
    let profile = provider(home.path(), unrelated.path(), "profile-thread");
    let profile_session = profile.create_session().await.unwrap();
    control(&profile, json!({"action":"load","name":"acceptance"})).await;
    assert_eq!(
        execute(
            &profile_session,
            "text(String(privateNumber)); text(privateMap.get('answer'));",
            echo.clone()
        )
        .await,
        ["41", "42"]
    );
    assert!(!unrelated.path().join("effect.txt").exists());

    control(&resumed, json!({"action":"prune","query":"PRIVATE*"})).await;
    assert_eq!(
        execute(
            &resumed_session,
            "text(typeof privateNumber); text(typeof privateMap); text(helper(2));",
            echo
        )
        .await,
        ["undefined", "undefined", "3"]
    );
    resumed_session.shutdown().await.unwrap();
    fresh_session.shutdown().await.unwrap();
    profile_session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn hooks_restore_without_recursion_and_cancelled_cells_require_explicit_recovery() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let notebook = provider(home.path(), project.path(), "hooks");
    let session = notebook.create_session().await.unwrap();
    let echo = Arc::new(Echo::default());
    execute(
        &session,
        r#"
globalThis.events = [];
async function observe(event) {
  globalThis.events.push(event.input.value);
  await tools.echo({ fromHook: true });
}
function initialize() { globalThis.starts = (globalThis.starts ?? 0) + 1; }
"#,
        echo.clone(),
    )
    .await;
    control(
        &notebook,
        json!({"action":"pin","names":["observe"],"hook":"tool_result"}),
    )
    .await;
    control(
        &notebook,
        json!({"action":"pin","names":["initialize"],"hook":"startup"}),
    )
    .await;
    assert_eq!(
        execute(&session, "text(typeof starts);", echo.clone()).await,
        ["undefined"]
    );
    control(&notebook, json!({"action":"restart"})).await;
    assert_eq!(execute(&session, "await Promise.all([tools.echo({value:1}), tools.echo({value:2})]); text(events.slice().sort()); text(starts);", echo.clone()).await, ["[1,2]", "1"]);
    assert_eq!(
        echo.calls.lock().unwrap().len(),
        4,
        "hook calls must not retrigger hooks"
    );
    control(&notebook, json!({"action":"checkpoint"})).await;

    let mut pending = request("globalThis.partial = 999; await new Promise(() => {});");
    pending.yield_time_ms = Some(1);
    let started = session.execute(pending, echo.clone(), None).await.unwrap();
    let cell_id = started.cell_id.clone();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    let status = tokio::time::timeout(
        Duration::from_secs(2),
        control(&notebook, json!({"action":"status"})),
    )
    .await
    .unwrap();
    assert!(
        status.message.contains("running") || status.details.to_string().contains("running"),
        "{status:?}"
    );
    session.terminate(cell_id).await.unwrap();
    assert!(
        session
            .execute(request("text('unexpected');"), echo.clone(), None)
            .await
            .is_err()
    );
    control(&notebook, json!({"action":"restart"})).await;
    assert_eq!(
        execute(
            &session,
            "text(typeof partial); text(events.length); text(starts);",
            echo
        )
        .await,
        ["undefined", "2", "2"]
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn failing_startup_hook_can_be_unpinned_without_starting_a_kernel() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let original = provider(home.path(), project.path(), "startup-recovery");
    let session = original.create_session().await.unwrap();
    execute(
        &session,
        "function brokenStartup() { Deno.writeTextFileSync('hook-runs', 'x', {append:true}); throw new Error('expected startup failure'); }",
        Arc::new(Echo::default()),
    )
    .await;
    control(
        &original,
        json!({"action":"pin","names":["brokenStartup"],"hook":"startup"}),
    )
    .await;
    session.shutdown().await.unwrap();

    let resumed = provider(home.path(), project.path(), "startup-recovery");
    let error = resumed
        .create_session()
        .await
        .err()
        .expect("startup must fail");
    assert!(error.contains("expected startup failure"), "{error}");
    let diagnostic = control(&resumed, json!({"action":"diagnostics"})).await;
    assert_eq!(diagnostic.details["runtime"]["state"], "invalidated");
    assert!(diagnostic.details.get("error").is_none(), "{diagnostic:?}");
    control(&resumed, json!({"action":"list"})).await;
    assert_eq!(
        std::fs::read_to_string(project.path().join("hook-runs")).unwrap(),
        "x",
        "disk-only reads must not rerun failing startup hooks"
    );
    control(
        &resumed,
        json!({"action":"unpin","names":["brokenStartup"]}),
    )
    .await;
    let recovered = resumed.create_session().await.unwrap();
    assert_eq!(
        execute(
            &recovered,
            "text(typeof brokenStartup);",
            Arc::new(Echo::default())
        )
        .await,
        ["function"]
    );
    recovered.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn ephemeral_notebooks_never_create_durable_state() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let notebook = provider(home.path(), project.path(), "ephemeral").with_ephemeral(true);
    let session = notebook.create_session().await.unwrap();
    execute(&session, "let retained = 42;", Arc::new(Echo::default())).await;
    control(&notebook, json!({"action":"checkpoint"})).await;
    control(&notebook, json!({"action":"restart"})).await;
    assert_eq!(
        execute(&session, "text(retained);", Arc::new(Echo::default())).await,
        ["42"]
    );
    let save = notebook
        .control(
            serde_json::from_value(json!({"action":"save","name":"must-not-persist"})).unwrap(),
        )
        .await;
    assert!(save.is_err());
    control(&notebook, json!({"action":"diagnostics"})).await;
    session.shutdown().await.unwrap();
    assert!(!home.path().join("notebook").exists());
}
