use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::protocol::{
    ErrorEvent, NotesCheckpoint, ThreadRolledBackEvent, TurnAbortedEvent, TurnCompleteEvent,
    TurnStartedEvent,
};

fn started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_id.to_owned(),
        root_turn_id: None,
        trace_id: None,
        started_at: None,
        model_context_window: None,
        collaboration_mode_kind: ModeKind::Default,
    }))
}

fn user(text: &str) -> RolloutItem {
    RolloutItem::ResponseItem(codex_history::ResponseItemEnvelope::new(
        ResponseItem::Message {
            id: None,
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: text.to_owned(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ))
}

fn complete(turn_id: &str, checkpoint: Option<NotesCheckpoint>, failed: bool) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        turn_id: turn_id.to_owned(),
        notes_checkpoint: checkpoint,
        last_agent_message: Some("done".to_owned()),
        error: failed.then(|| ErrorEvent {
            message: "failed".to_owned(),
            codex_error_info: None,
            misalignment: None,
        }),
        started_at: None,
        completed_at: None,
        duration_ms: None,
        time_to_first_token_ms: None,
    }))
}

#[tokio::test]
async fn replay_only_reuses_notes_from_the_selected_settled_run() {
    let (session, turn) = super::tests::make_session_and_context().await;
    let checkpoint = NotesCheckpoint {
        window_id: session
            .state
            .lock()
            .await
            .auto_compact_window_ids()
            .window_id
            .to_string(),
        settled_at_ms: 100,
        fresh: true,
    };
    let saved = vec![
        started("saved"),
        user("old work"),
        complete("saved", Some(checkpoint.clone()), false),
    ];
    assert_eq!(
        session
            .reconstruct_history_from_rollout(&turn, &saved)
            .await
            .notes_checkpoint,
        Some(checkpoint.clone())
    );

    for tail in [
        vec![started("unfinished")],
        vec![
            started("cancelled"),
            RolloutItem::EventMsg(EventMsg::TurnAborted(TurnAbortedEvent {
                turn_id: Some("cancelled".to_owned()),
                reason: TurnAbortReason::Interrupted,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            })),
        ],
        vec![
            started("missing"),
            user("new work"),
            complete("missing", None, false),
        ],
        vec![
            started("failed"),
            user("new work"),
            complete("failed", Some(checkpoint.clone()), true),
        ],
        vec![user("injected after completion")],
    ] {
        let mut history = saved.clone();
        history.extend(tail);
        assert_eq!(
            session
                .reconstruct_history_from_rollout(&turn, &history)
                .await
                .notes_checkpoint,
            None
        );
    }

    let mut rolled_back = saved;
    rolled_back.extend([
        started("removed"),
        user("discarded work"),
        complete("removed", None, false),
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
            num_turns: 1,
        })),
    ]);
    assert_eq!(
        session
            .reconstruct_history_from_rollout(&turn, &rolled_back)
            .await
            .notes_checkpoint,
        Some(checkpoint)
    );
}

#[tokio::test]
async fn only_successful_settlement_authorizes_notes_reuse() {
    let (session, mut turn) = super::tests::make_session_and_context().await;
    turn.config = Arc::new({
        let mut config = (*turn.config).clone();
        config.context_strategy = codex_config::types::ContextStrategy::Notes;
        config
    });
    session.begin_notes_run(&turn).await;
    let tracker = session
        .services
        .thread_extension_data
        .get::<codex_extension_api::NotesCheckpointTracker>()
        .unwrap();
    tracker.finish_write(tracker.begin_write(&turn.sub_id).unwrap(), true);
    assert!(session.notes_settlement(&turn, true).await.fresh);
    assert!(!session.notes_settlement(&turn, false).await.fresh);
    tracker.finish_write(tracker.begin_write(&turn.sub_id).unwrap(), false);
    assert!(!session.notes_settlement(&turn, true).await.fresh);
    session.begin_notes_run(&turn).await;
    assert!(!session.notes_settlement(&turn, true).await.fresh);
}
