//! Public progress and typed/spoken results share one explicit, generation-fenced writer.

use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn typed_task_streams_public_progress_and_speaks_its_final_once() {
    let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    let thread = activate_voice(&mut chat);
    chat.config.realtime.screenless = true;
    start_item(&mut chat, thread, "typed", user_item("check the build"));
    assert!(
        matches!(ops.try_recv(), Ok(AppCommand::RealtimeConversationUpdate { speak: false, text, .. })
        if text.as_str().contains("already accepted") && text.as_str().contains("check the build"))
    );
    let progress = agent_item(
        "progress",
        "Checking the build. Reading the failures.\n\nFound the cause.",
        Some(MessagePhase::Commentary),
    );
    start_item(&mut chat, thread, "typed", progress.clone());
    for delta in [
        "Checking the build.",
        " Reading the failures.",
        "\n\nFound the cause.",
    ] {
        chat.handle_server_notification(
            ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
                thread_id: thread.to_string(),
                turn_id: "typed".into(),
                item_id: "progress".into(),
                delta: delta.into(),
            }),
            None,
        );
    }
    complete_item(&mut chat, thread, "typed", progress.clone());
    complete_item(&mut chat, thread, "typed", progress);
    let updates = std::iter::from_fn(|| ops.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(updates.len(), 2);
    assert!(
        matches!(&updates[0], AppCommand::RealtimeConversationUpdate { speak: true, text, .. }
        if text.as_str() == "Checking the build. Reading the failures.")
    );
    assert!(
        matches!(&updates[1], AppCommand::RealtimeConversationUpdate { speak: true, text, .. }
        if text.as_str() == "Found the cause.")
    );
    let final_item = agent_item(
        "final",
        "The build is fixed.",
        Some(MessagePhase::FinalAnswer),
    );
    start_item(&mut chat, thread, "typed", final_item.clone());
    complete_item(&mut chat, thread, "typed", final_item.clone());
    assert!(ops.try_recv().is_err());
    finish_turn(
        &mut chat,
        thread,
        "typed",
        vec![final_item],
        TurnStatus::Completed,
    );
    assert!(
        matches!(ops.try_recv(), Ok(AppCommand::RealtimeConversationSpeech { text, .. }) if text.as_str() == "The build is fixed.")
    );
    assert!(ops.try_recv().is_err());
}

#[tokio::test]
async fn only_explicitly_visible_completed_public_summaries_can_be_spoken() {
    for (summary_setting, hidden, expected) in [
        (None, false, false),
        (
            Some(codex_protocol::config_types::ReasoningSummary::None),
            false,
            false,
        ),
        (
            Some(codex_protocol::config_types::ReasoningSummary::Concise),
            true,
            false,
        ),
        (
            Some(codex_protocol::config_types::ReasoningSummary::Concise),
            false,
            true,
        ),
    ] {
        let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
        let thread = activate_voice(&mut chat);
        chat.config.realtime.screenless = true;
        chat.config.model_reasoning_summary = summary_setting;
        chat.config.hide_agent_reasoning = hidden;
        start_item(
            &mut chat,
            thread,
            "voice",
            user_item("<realtime_delegation><input>check it</input></realtime_delegation>"),
        );
        let reasoning = ThreadItem::Reasoning {
            id: "reasoning".into(),
            summary: vec!["Public settled summary".into()],
            content: vec!["private-raw-secret".into()],
        };
        start_item(&mut chat, thread, "voice", reasoning.clone());
        chat.handle_server_notification(
            ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
                thread_id: thread.to_string(),
                turn_id: "voice".into(),
                item_id: "reasoning".into(),
                delta: "private-raw-secret".into(),
                content_index: 0,
            }),
            None,
        );
        assert!(ops.try_recv().is_err());
        complete_item(&mut chat, thread, "voice", reasoning.clone());
        complete_item(&mut chat, thread, "voice", reasoning);
        let update = ops.try_recv().ok();
        assert_eq!(update.is_some(), expected);
        if let Some(AppCommand::RealtimeConversationUpdate { text, speak, .. }) = update {
            assert!(speak);
            assert_eq!(text.as_str(), "Public settled summary");
        }
        assert!(ops.try_recv().is_err());
    }
}

#[tokio::test]
async fn a_new_input_fences_old_progress_and_raw_analysis() {
    let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    let thread = activate_voice(&mut chat);
    chat.config.realtime.screenless = true;
    start_item(
        &mut chat,
        thread,
        "old",
        user_item("<realtime_delegation><input>old task</input></realtime_delegation>"),
    );
    let progress = agent_item(
        "old-progress",
        "Old first sentence. Old second sentence.",
        Some(MessagePhase::Commentary),
    );
    start_item(&mut chat, thread, "old", progress.clone());
    chat.realtime_conversation.latest_voice_input_fingerprint =
        Some(super::super::realtime_input_fingerprint("old task"));
    chat.note_realtime_typed_input("new task");
    start_item(&mut chat, thread, "new", user_item("new task"));
    assert!(matches!(
        ops.try_recv(),
        Ok(AppCommand::RealtimeConversationUpdate { speak: false, .. })
    ));
    complete_item(&mut chat, thread, "old", progress);
    // A late, duplicate old voice delegation must not claim a newly admitted typed task.
    start_item(
        &mut chat,
        thread,
        "old",
        user_item("<realtime_delegation><input>old task</input></realtime_delegation>"),
    );
    assert!(!chat.realtime_conversation.latest_input_was_voice);
    let private = agent_item(
        "analysis",
        "[ANALYSIS] private first. private second.",
        Some(MessagePhase::Commentary),
    );
    start_item(&mut chat, thread, "new", private.clone());
    complete_item(&mut chat, thread, "new", private);
    assert!(ops.try_recv().is_err());
    assert!(
        chat.latest_realtime_input_can_speak(),
        "typing does not disable screenless results"
    );
}
