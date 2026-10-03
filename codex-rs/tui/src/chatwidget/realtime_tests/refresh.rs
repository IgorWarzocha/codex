//! Serial refresh and one established-media replacement are independent of sideband reconnect.

use super::super::PreparedVoiceContext;
use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn refresh_preparation_failure_keeps_the_old_call_and_stop_cancels_preparation() {
    let (mut chat, _, mut events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    let Some(abort) = chat.realtime_conversation.refresh_abort.clone() else {
        panic!("preparation must be owned");
    };
    let attempt = chat.realtime_conversation.attempt_id;
    chat.on_realtime_refresh_prepared(
        attempt,
        0,
        1,
        Err("configured personality file missing".into()),
    );
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Active
    );
    assert!(ops.try_recv().is_err());
    assert!(!chat.realtime_conversation.refresh_requested);
    while events.try_recv().is_ok() {}
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    let new_abort = chat.realtime_conversation.refresh_abort.clone().unwrap();
    chat.stop_realtime_conversation();
    assert!(new_abort.is_aborted());
    chat.on_realtime_refresh_prepared(attempt, 0, 2, Ok(PreparedVoiceContext(Vec::new())));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Inactive
    );
    assert!(
        !abort.is_aborted(),
        "a completed preparation is no longer live"
    );
}

#[tokio::test]
async fn refresh_rejects_changed_input_and_replaces_serially_after_preparation() {
    let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    let thread = activate_voice(&mut chat);
    chat.realtime_conversation.microphone_muted = true;
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    let old_attempt = chat.realtime_conversation.attempt_id;
    chat.realtime_conversation.input_generation = 1;
    chat.on_realtime_refresh_prepared(old_attempt, 0, 1, Ok(PreparedVoiceContext(Vec::new())));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Active
    );
    assert!(ops.try_recv().is_err());
    assert!(chat.realtime_conversation.refresh_requested);
    chat.maybe_prepare_realtime_refresh();
    let seed = codex_app_server_protocol::ThreadRealtimeInitialItem {
        role: codex_protocol::protocol::ConversationTextRole::Developer,
        text: "fresh current-thread context".into(),
    };
    chat.on_realtime_refresh_prepared(
        old_attempt,
        1,
        1,
        Ok(PreparedVoiceContext(vec![seed.clone()])),
    );
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Stopping
    );
    assert!(
        matches!(ops.try_recv(), Ok(AppCommand::RealtimeConversationStop { thread_id }) if thread_id == thread)
    );
    assert!(
        ops.try_recv().is_err(),
        "no new call before old stop acknowledgement"
    );
    chat.on_realtime_conversation_closed(Some("requested".into()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Starting
    );
    assert_ne!(chat.realtime_conversation.attempt_id, old_attempt);
    assert!(chat.realtime_conversation.microphone_muted);
    assert_eq!(chat.take_prepared_realtime_context(), Some(vec![seed]));
    assert!(chat.realtime_conversation.replacement_call);
    assert!(!chat.realtime_conversation.media_resume_used);
    chat.on_realtime_refresh_prepared(old_attempt, 1, 1, Err("late failure".into()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Starting
    );
    chat.stop_realtime_conversation();
}

#[tokio::test]
async fn a_new_context_boundary_cancels_and_fences_old_preparation_without_new_user_input() {
    let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    let old_abort = chat.realtime_conversation.refresh_abort.clone().unwrap();
    let attempt = chat.realtime_conversation.attempt_id;
    chat.request_realtime_context_refresh();
    assert!(old_abort.is_aborted());
    chat.maybe_prepare_realtime_refresh();
    chat.on_realtime_refresh_prepared(attempt, 0, 1, Ok(PreparedVoiceContext(Vec::new())));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Active
    );
    assert!(chat.realtime_conversation.refresh_abort.is_some());
    assert!(ops.try_recv().is_err());
    chat.on_realtime_refresh_prepared(attempt, 0, 2, Err("latest preparation failed".into()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Active
    );
    assert!(chat.realtime_conversation.refresh_abort.is_none());
    chat.stop_realtime_conversation();
}

#[tokio::test]
async fn established_media_replacement_is_once_and_user_stop_or_optout_prevents_resume() {
    for enabled in [false, true] {
        let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
        let thread = activate_voice(&mut chat);
        chat.config.realtime.auto_resume = enabled;
        chat.realtime_conversation.microphone_muted = true;
        chat.on_realtime_media_closed();
        assert!(
            matches!(ops.try_recv(), Ok(AppCommand::RealtimeConversationStop { thread_id }) if thread_id == thread)
        );
        if enabled {
            chat.on_realtime_conversation_closed(Some("requested".into()));
            assert_eq!(
                chat.realtime_conversation.phase,
                RealtimeConversationPhase::Starting
            );
            assert!(chat.realtime_conversation.media_resume_used);
            assert!(chat.realtime_conversation.microphone_muted);
            chat.realtime_conversation.phase = RealtimeConversationPhase::Active;
            chat.on_realtime_media_closed();
            chat.on_realtime_conversation_closed(Some("requested".into()));
            assert_eq!(
                chat.realtime_conversation.phase,
                RealtimeConversationPhase::Inactive
            );
        } else {
            assert_eq!(
                chat.realtime_conversation.phase,
                RealtimeConversationPhase::Inactive
            );
        }
    }
    let (mut chat, _, _events, _ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.on_realtime_media_closed();
    chat.stop_realtime_conversation();
    chat.on_realtime_conversation_closed(Some("requested".into()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Inactive
    );
}

#[tokio::test]
async fn typing_during_serial_replacement_fences_old_work_and_discards_its_seed() {
    let (mut chat, _, _events, _ops) = make_chatwidget_manual_with_sender().await;
    let thread = activate_voice(&mut chat);
    chat.config.realtime.screenless = true;
    start_item(
        &mut chat,
        thread,
        "old",
        user_item("<realtime_delegation><input>old task</input></realtime_delegation>"),
    );
    let old_generation = chat.realtime_conversation.input_generation;
    chat.realtime_conversation.latest_voice_input_fingerprint =
        Some(super::super::realtime_input_fingerprint("old task"));
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    let attempt = chat.realtime_conversation.attempt_id;
    chat.on_realtime_refresh_prepared(
        attempt,
        old_generation,
        1,
        Ok(PreparedVoiceContext(Vec::new())),
    );
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Stopping
    );
    chat.note_realtime_typed_input("new task during replacement");
    assert!(chat.realtime_conversation.input_generation > old_generation);
    assert!(!chat.realtime_turn_may_speak("old"));
    assert!(chat.take_prepared_realtime_context().is_none());
    chat.on_realtime_conversation_closed(Some("requested".into()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Starting
    );
    assert!(!chat.realtime_turn_may_speak("old"));
    assert!(!chat.realtime_conversation.latest_input_was_voice);
    assert_eq!(
        chat.realtime_conversation.pending_typed_input.as_deref(),
        Some("new task during replacement")
    );
    assert_eq!(
        chat.realtime_conversation.latest_voice_input_fingerprint,
        Some(super::super::realtime_input_fingerprint("old task"))
    );
    let generation = chat.realtime_conversation.input_generation;
    chat.note_realtime_typed_input("another new task during startup");
    assert!(chat.realtime_conversation.input_generation > generation);
    assert!(!chat.realtime_turn_may_speak("old"));
    start_item(
        &mut chat,
        thread,
        "new",
        user_item("another new task during startup"),
    );
    finish_turn(&mut chat, thread, "new", Vec::new(), TurnStatus::Completed);
    assert!(!chat.turn_lifecycle.agent_turn_running);
    let generation = chat.realtime_conversation.input_generation;
    let attempt = chat.realtime_conversation.attempt_id;
    chat.on_realtime_conversation_started();
    chat.on_realtime_webrtc_connected(attempt, Ok(()));
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Active
    );
    assert!(!chat.realtime_conversation.latest_input_was_voice);
    chat.on_realtime_transcript_done("user".into(), "old task".into());
    start_item(
        &mut chat,
        thread,
        "old",
        user_item("<realtime_delegation><input>old task</input></realtime_delegation>"),
    );
    assert_eq!(chat.realtime_conversation.input_generation, generation);
    assert!(!chat.realtime_conversation.latest_input_was_voice);
    assert!(!chat.realtime_turn_may_speak("old"));
    chat.stop_realtime_conversation();
}

#[tokio::test]
async fn voice_toggle_during_replacement_cleanup_cancels_the_restart() {
    for reason in [
        super::super::Replacement::Media,
        super::super::Replacement::Context,
    ] {
        let (mut chat, _, _events, mut ops) = make_chatwidget_manual_with_sender().await;
        let thread = activate_voice(&mut chat);
        chat.begin_realtime_replacement(reason);
        assert!(
            matches!(ops.try_recv(), Ok(AppCommand::RealtimeConversationStop { thread_id }) if thread_id == thread)
        );
        chat.toggle_realtime_conversation();
        assert_eq!(
            chat.realtime_conversation.phase,
            RealtimeConversationPhase::Stopping
        );
        assert_eq!(
            chat.realtime_conversation.replacement,
            super::super::Replacement::None
        );
        assert!(ops.try_recv().is_err(), "stop cleanup is already queued");
        chat.on_realtime_conversation_closed(Some("requested".into()));
        assert_eq!(
            chat.realtime_conversation.phase,
            RealtimeConversationPhase::Inactive
        );
        assert!(ops.try_recv().is_err(), "toggle must not restart voice");
    }
}

#[tokio::test]
async fn context_refresh_waits_for_host_work_and_respects_optout() {
    let (mut chat, _, _events, _ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.config.realtime.refresh_on_context_change = false;
    chat.request_realtime_context_refresh();
    assert!(!chat.realtime_conversation.refresh_requested);
    chat.config.realtime.refresh_on_context_change = true;
    chat.turn_lifecycle.agent_turn_running = true;
    chat.request_realtime_context_refresh();
    chat.maybe_prepare_realtime_refresh();
    assert!(chat.realtime_conversation.refresh_abort.is_none());
    chat.turn_lifecycle.agent_turn_running = false;
    chat.maybe_prepare_realtime_refresh();
    assert!(chat.realtime_conversation.refresh_abort.is_some());
    chat.stop_realtime_conversation();
}
