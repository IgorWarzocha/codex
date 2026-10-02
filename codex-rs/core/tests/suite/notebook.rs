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
            config.code_mode.deno_program = Some(deno);
            config.ephemeral = true;
            config.base_instructions = Some("Keep this explicit base unchanged.".to_string());
            config.features.enable(Feature::TokenBudget).unwrap();
        })
        .build(&server)
        .await?;
    test.submit_turn("Retain a binding across a new context window")
        .await?;
    let requests = responses.requests();
    assert_eq!(requests.len(), 5);
    let instructions = requests[0].instructions_text();
    assert!(instructions.starts_with("Keep this explicit base unchanged.\n\n<exec_tools>\n"));
    assert!(instructions.contains("tools.exec_command("));
    for request in &requests {
        // Repeated samples and context rollover must not accumulate tool catalogs.
        assert_eq!(request.instructions_text(), instructions);
        assert_eq!(
            request.instructions_text().matches("<exec_tools>").count(),
            1
        );
        assert!(
            !request
                .message_input_texts("developer")
                .join("\n")
                .contains("<exec_tools>")
        );
        let body = request.body_json();
        let exec = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "exec")
            .expect("Notebook exec declaration");
        let description = exec["description"].as_str().unwrap();
        assert!(!description.contains("tools.exec_command("));
        assert!(!description.contains("<exec_tools>"));
    }
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
    assert!(checkpoint.contains("Notebook checkpoint"), "{checkpoint}");
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case::test_case(false; "json")]
#[test_case::test_case(true; "plain")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a local Deno executable and Unix shell"]
#[cfg(unix)]
async fn notebook_command_output_projects_only_the_displayed_result(plain: bool) -> Result<()> {
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("command-response"),
                ev_custom_tool_call(
                    "command-output",
                    "exec",
                    r#"var commandResult = await tools.exec_command({cmd: "printf 'first\\nsecond\\n'; exit 7", login: false});
text(commandResult);
text({...commandResult});
text(commandResult.output);"#,
                ),
                ev_completed("command-response"),
            ]),
            sse(vec![ev_assistant_message("done", "done"), ev_completed("done")]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(move |config| {
            config.code_mode.runtime = CodeModeRuntime::Notebook;
            config.code_mode.deno_program = Some(
                std::env::var_os("DENO_PROGRAM")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "deno".into()),
            );
            config.code_mode.notebook_plain_command_output = plain;
            config.ephemeral = true;
            config.features.enable(Feature::CodeModeOnly).unwrap();
        })
        .build(&server)
        .await?;
    test.submit_turn("Run the command and inspect the returned object")
        .await?;
    let requests = responses.requests();
    let result = requests[1].custom_tool_call_output("command-output");
    let items = result["output"].as_array().expect("three text emissions");
    assert_eq!(items.len(), 3, "{result}");
    let projected = items[0]["text"].as_str().unwrap();
    let projected_metadata: serde_json::Value = serde_json::from_str(if plain {
        let (metadata, output) = projected.split_once("\nOutput:\n").unwrap();
        assert_eq!(output, "first\nsecond\n");
        metadata
    } else {
        projected
    })?;
    assert_eq!(projected_metadata["exit_code"], 7);
    for key in ["chunk_id", "wall_time_seconds", "original_token_count"] {
        assert!(projected_metadata.get(key).is_none(), "{projected}");
    }
    if !plain {
        assert_eq!(projected_metadata["output"], "first\nsecond\n");
    }
    let raw: serde_json::Value = serde_json::from_str(items[1]["text"].as_str().unwrap())?;
    assert_eq!(raw["exit_code"], 7);
    assert_eq!(raw["output"], "first\nsecond\n");
    assert!(raw.get("chunk_id").is_some(), "{raw}");
    assert!(raw.get("wall_time_seconds").is_some(), "{raw}");
    assert_eq!(items[2]["text"], "first\nsecond\n");
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
