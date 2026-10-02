use anyhow::Result;
use codex_code_mode::CodeModeSessionProvider;
use codex_features::CodeModeRuntime;
use codex_features::Feature;
use codex_notebook::DenoNotebookSessionProvider;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_custom_tool_call;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notebook_context_restores_status_after_rollover_without_exposing_values() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let deno = std::env::var_os("DENO_PROGRAM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "deno".into());
    if let Err(error) =
        DenoNotebookSessionProvider::new(deno.clone(), std::env::current_dir()?).availability()
    {
        eprintln!("skipping Notebook integration test: {error}");
        return Ok(());
    }
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_custom_tool_call(
                    "create-binding",
                    "exec",
                    r#"globalThis.retainedNotebookBinding = "private-secret-marker"; text("created")"#,
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_function_call("rollover", "new_context", "{}"),
                ev_completed("resp-2"),
            ]),
            sse(vec![
                ev_response_created("resp-3"),
                ev_custom_tool_call(
                    "read-binding",
                    "exec",
                    r#"let rejected = false;
try { await tools.notebook({action: "prune", query: "*"}); } catch { rejected = true; }
text({value: globalThis.retainedNotebookBinding, rejected, status: (await tools.notebook({action: "status"})).message});"#,
                ),
                ev_completed("resp-3"),
            ]),
            sse(vec![
                ev_response_created("resp-4"),
                ev_function_call("checkpoint", "notebook", r#"{"action":"checkpoint"}"#),
                ev_completed("resp-4"),
            ]),
            sse(vec![
                ev_assistant_message("done", "done"),
                ev_completed("resp-5"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(move |config| {
            config.code_mode.runtime = CodeModeRuntime::Notebook;
            config.code_mode.deno_program = deno;
            config.ephemeral = true;
            config.features.enable(Feature::TokenBudget).unwrap();
        })
        .build(&server)
        .await?;
    test.submit_turn("Retain a binding across a new context window")
        .await?;
    let requests = responses.requests();
    assert_eq!(requests.len(), 5);
    let startup = requests[0].message_input_texts("developer").join("\n");
    assert!(startup.contains("Notebook idle"), "{startup}");
    assert!(!startup.contains("Notebook status unavailable"));
    let rollover = requests[2].message_input_texts("developer").join("\n");
    assert!(rollover.contains("retainedNotebookBinding"), "{rollover}");
    assert!(!rollover.contains("private-secret-marker"));
    let output = requests[3]
        .custom_tool_call_output("read-binding")
        .to_string();
    assert!(output.contains("private-secret-marker"), "{output}");
    assert!(
        output.contains("rejected") && output.contains("true"),
        "{output}"
    );
    assert!(output.contains("Notebook running (cached)"), "{output}");
    let checkpoint = requests[4].function_call_output("checkpoint").to_string();
    assert!(
        checkpoint.contains("Notebook management complete"),
        "{checkpoint}"
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
