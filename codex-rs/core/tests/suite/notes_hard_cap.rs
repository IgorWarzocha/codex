use anyhow::Result;
use codex_config::types::ContextStrategy;
use codex_core::TurnInputRequest;
use codex_core::compact::SUMMARIZATION_PROMPT;
use codex_core::config::TokenBudgetConfig;
use codex_login::CodexAuth;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use test_case::test_case;

const FULL_WINDOW: i64 = 64_000;
const USABLE_WINDOW: i64 = FULL_WINDOW * 80 / 100;
const SUMMARY: &str = "Readable checkpoint: retain job 42 and the tool's budget evidence.";

fn notes_fixture(scope: AutoCompactTokenLimitScope) -> TestCodexBuilder {
    test_codex()
        .with_context_strategy(ContextStrategy::Notes)
        .with_auth(
            CodexAuth::from_external_chatgpt_tokens(
                "header.e30.signature",
                "account-123",
                Some("plus"),
            )
            .expect("test backend authentication"),
        )
        .with_model_info_override("gpt-5.2", |model| {
            model.effective_context_window_percent = 80;
        })
        .with_config(move |config| {
            let base_url = config.model_provider.base_url.as_ref().unwrap();
            config.model_provider.base_url = Some(format!(
                "{}/backend-api/codex",
                base_url.strip_suffix("/v1").unwrap()
            ));
            config.model_context_window = Some(FULL_WINDOW);
            config.model_auto_compact_token_limit = Some(32_000);
            config.model_auto_compact_token_limit_scope = scope;
            config.base_instructions = Some("Complete the user's task.".into());
            config.token_budget = Some(TokenBudgetConfig {
                reminder_threshold_tokens: Some(6_000),
                reminder_message_template: "Native budget: {n_remaining} tokens remain.".into(),
                guidance_message: Some("Save notes before requesting a new context.".into()),
                auto_compact_fallback_prompt: Some("Save the current work to notes.".into()),
                auto_compact_fallback_buffer_tokens: Some(8_000),
                ..TokenBudgetConfig::default()
            });
        })
}

fn reply(id: &str, text: &str) -> String {
    sse(vec![ev_assistant_message(id, text), ev_completed(id)])
}

async fn complete(test: &TestCodex, text: &str) -> Result<TurnCompleteEvent> {
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let event = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(completion) = event else {
        unreachable!("waited for turn completion")
    };
    Ok(completion)
}

fn assert_same_window(requests: &[ResponsesRequest]) {
    let first = requests[0].header("x-codex-window-id").unwrap();
    assert!(
        requests.iter().all(|request| {
            request.header("x-codex-window-id").as_deref() == Some(first.as_str())
        }),
        "readable rescue must not retire the notes window"
    );
}

#[test_case(AutoCompactTokenLimitScope::Total, USABLE_WINDOW; "exact_usable_cap")]
#[test_case(AutoCompactTokenLimitScope::BodyAfterPrefix, USABLE_WINDOW + 1_000; "above_cap_with_body_after_prefix")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usable_hard_cap_rescues_tool_continuation_on_a_larger_backend(
    scope: AutoCompactTokenLimitScope,
    usage: i64,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    // Every request succeeds at this backend, including requests above our configured cap.
    // Usage is below the full window but reaches its usable, headroom-reserving limit.
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_function_call("budget-evidence", "get_context_remaining", "{}"),
                ev_completed_with_tokens("tool", usage),
            ]),
            reply("summary", SUMMARY),
            reply("final", "job 42 recovered"),
        ],
    )
    .await;
    let test = notes_fixture(scope).build(&server).await?;
    let completion = complete(&test, "Retain job 42").await?;
    assert!(completion.error.is_none(), "{completion:?}");
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some("job 42 recovered")
    );
    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        3,
        "summary must precede normal continuation"
    );
    assert!(requests[1].body_contains_text(SUMMARIZATION_PROMPT));
    assert!(
        requests[1].input().iter().any(|item| {
            item["type"] == "function_call_output" && item["call_id"] == "budget-evidence"
        }),
        "the readable summarizer must receive active tool evidence"
    );
    assert!(requests[2].body_contains_text(SUMMARY));
    assert!(requests[2].body_contains_text("Retain job 42"));
    assert_same_window(&requests);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_previous_turn_preserves_accepted_input_before_sampling() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_assistant_message("previous", "job 42 evidence"),
                ev_completed_with_tokens("previous", FULL_WINDOW + 1_000),
            ]),
            reply("summary", SUMMARY),
            reply("final", "continued"),
        ],
    )
    .await;
    let test = notes_fixture(AutoCompactTokenLimitScope::Total)
        .build(&server)
        .await?;
    assert!(complete(&test, "Previous task").await?.error.is_none());
    assert!(complete(&test, "New accepted task").await?.error.is_none());
    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].body_contains_text(SUMMARIZATION_PROMPT));
    assert!(requests[1].body_contains_text("job 42 evidence"));
    assert!(requests[1].body_contains_text("New accepted task"));
    assert!(requests[2].body_contains_text("New accepted task"));
    assert!(requests[2].body_contains_text(SUMMARY));
    assert_same_window(&requests);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_new_input_is_summarized_before_first_normal_request() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![reply("summary", SUMMARY), reply("final", "admitted")],
    )
    .await;
    let test = notes_fixture(AutoCompactTokenLimitScope::BodyAfterPrefix)
        .build(&server)
        .await?;
    let input = format!(
        "Accepted job 42 input {}",
        "x".repeat((FULL_WINDOW * 4 + 4_000) as usize)
    );
    let completion = complete(&test, &input).await?;
    assert!(completion.error.is_none(), "{completion:?}");
    assert_eq!(completion.last_agent_message.as_deref(), Some("admitted"));
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].body_contains_text(SUMMARIZATION_PROMPT));
    assert!(requests[0].body_contains_text("Accepted job 42 input"));
    assert!(requests[1].body_contains_text(SUMMARY));
    assert!(requests[1].body_contains_text("Accepted job 42 input"));
    assert_same_window(&requests);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(false; "summary_still_exceeds_hard_cap")]
#[test_case(true; "backend_overflow_after_hard_cap_rescue")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hard_cap_rescue_is_bounded_and_fails_visibly(backend_overflows: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let mut replies = vec![sse(vec![
        ev_function_call("budget-evidence", "get_context_remaining", "{}"),
        ev_completed_with_tokens("tool", FULL_WINDOW + 1_000),
    ])];
    replies.push(reply(
        "summary",
        &if backend_overflows {
            SUMMARY.into()
        } else {
            "x".repeat((FULL_WINDOW * 4 + 4_000) as usize)
        },
    ));
    if backend_overflows {
        replies.push(sse_failed(
            "overflow",
            "context_length_exceeded",
            "Input exceeds context",
        ));
    }
    let responses = mount_sse_sequence(&server, replies).await;
    let test = notes_fixture(AutoCompactTokenLimitScope::Total)
        .build(&server)
        .await?;
    let completion = complete(&test, "Retain job 42").await?;
    let error = completion
        .error
        .expect("oversized rescue must stop visibly");
    assert_eq!(
        error.codex_error_info,
        Some(CodexErrorInfo::ContextWindowExceeded)
    );
    let requests = responses.requests();
    assert_eq!(requests.len(), if backend_overflows { 3 } else { 2 });
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.body_contains_text(SUMMARIZATION_PROMPT))
            .count(),
        1,
        "configured exhaustion and backend overflow must share one rescue budget"
    );
    assert_same_window(&requests);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
