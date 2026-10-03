use super::*;
use crate::session::tests::make_session_and_context_with_rx;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnLifecycleContributor;
use codex_protocol::AgentPath;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::turn_input::TurnStartOptions;
use tokio::time::timeout;

struct PendingTask;

impl SessionTask for PendingTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }
    fn span_name(&self) -> &'static str {
        "completion_wake.pending"
    }
    async fn run(
        self: Arc<Self>,
        _session: Arc<Session>,
        _ctx: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        cancellation: CancellationToken,
    ) -> SessionTaskResult {
        cancellation.cancelled().await;
        Ok(None)
    }
}

async fn enqueue_result(session: &Session, text: &str) -> InterAgentCommunication {
    let mail = InterAgentCommunication::new(
        AgentPath::root().join("worker").expect("worker path"),
        AgentPath::root(),
        Vec::new(),
        text.to_string(),
        false,
    );
    session
        .input_queue
        .enqueue_mailbox_communication(
            mail.clone(),
            TurnStartOptions {
                resume_parent_on_completion: true,
                ..Default::default()
            },
        )
        .await;
    mail
}

#[tokio::test]
async fn active_parent_consumes_completion_without_starting_another_turn() {
    let (session, turn, _rx) = make_session_and_context_with_rx().await;
    session
        .spawn_task(Arc::clone(&turn), Vec::new(), PendingTask)
        .await;
    let first = enqueue_result(&session, "first result").await;
    let second = enqueue_result(&session, "second result").await;
    tokio::join!(
        session.maybe_start_turn_for_pending_work(),
        session.maybe_start_turn_for_pending_work()
    );
    assert_eq!(
        session
            .active_turn
            .lock()
            .await
            .as_ref()
            .expect("active")
            .task
            .as_ref()
            .expect("original task")
            .turn_context
            .sub_id,
        turn.sub_id
    );
    assert_eq!(
        session
            .input_queue
            .get_pending_input(&session.active_turn)
            .await
            .0,
        vec![
            TurnInput::InterAgentCommunication(first),
            TurnInput::InterAgentCommunication(second),
        ]
    );
    assert!(!session.input_queue.has_completion_mailbox_items().await);
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}

#[test_case::test_case(false, false; "idle_stop")]
#[test_case::test_case(true, false; "active_stop")]
#[test_case::test_case(false, true; "idle_stop_with_durable_sleep")]
#[tokio::test]
async fn stop_blocks_late_completion_until_explicit_start(active: bool, sleep: bool) {
    let (session, turn, _rx) = make_session_and_context_with_rx().await;
    if active {
        session
            .spawn_task(Arc::clone(&turn), Vec::new(), PendingTask)
            .await;
    }
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
    if sleep {
        session
            .services
            .thread_extension_data
            .insert(codex_extension_items::sleep::SleepItem {
                id: "durable-sleep".to_string(),
                duration_ms: 60_000,
            });
        assert!(
            session.can_wake_for_pending_work(false, false).await,
            "legacy queue-only mail can still wake durable sleep"
        );
        assert!(
            session.can_wake_for_pending_work(true, true).await,
            "explicit followup remains authorized after Stop"
        );
    }
    let mail = enqueue_result(&session, "late result").await;
    session.maybe_start_turn_for_pending_work().await;
    assert!(session.active_turn.lock().await.is_none());
    assert!(!session.can_wake_for_pending_work(false, true).await);
    session
        .spawn_task(Arc::clone(&turn), Vec::new(), PendingTask)
        .await;
    assert!(session.can_wake_for_pending_work(false, true).await);
    assert_eq!(
        session
            .input_queue
            .get_pending_input(&session.active_turn)
            .await
            .0,
        vec![TurnInput::InterAgentCommunication(mail)]
    );
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}

struct StartGate {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl TurnLifecycleContributor for StartGate {
    fn turn_start_phase(
        &self,
        _data: &codex_extension_api::ExtensionData,
    ) -> codex_extension_api::TurnStartPhase {
        codex_extension_api::TurnStartPhase::BeforeTaskRegistration
    }
    fn on_turn_start<'a>(
        &'a self,
        _input: codex_extension_api::TurnStartInput<'a>,
    ) -> codex_extension_api::ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
        })
    }
}

#[test_case::test_case(false; "single_completion_keeps_stop_classification")]
#[test_case::test_case(true; "explicit_followup_keeps_start_metadata")]
#[tokio::test]
async fn cancelled_start_preserves_mail_metadata_with_sleep(followup: bool) {
    let (mut session, _turn, _rx) = make_session_and_context_with_rx().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.turn_lifecycle_contributor(Arc::new(StartGate {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
        starts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }));
    Arc::get_mut(&mut session)
        .expect("unique session")
        .services
        .extensions = Arc::new(extensions.build());
    session
        .services
        .thread_extension_data
        .insert(codex_extension_items::sleep::SleepItem {
            id: "durable-sleep".to_string(),
            duration_ms: 60_000,
        });
    let options = TurnStartOptions {
        resume_parent_on_completion: !followup,
        turn_trigger: Some("explicit_followup".to_string()),
        final_output_json_schema: Some(serde_json::json!({"type": "string"})),
        parent_turn_id: Some("original-parent".to_string()),
        root_turn_id: Some("original-root".to_string()),
        cyber_access_program: Some(codex_protocol::turn_input::CyberAccessProgram::Standard),
        ..Default::default()
    };
    let result = InterAgentCommunication::new(
        AgentPath::root().join("worker").expect("worker path"),
        AgentPath::root(),
        Vec::new(),
        "single delivery at startup".to_string(),
        followup,
    );
    session
        .input_queue
        .enqueue_mailbox_communication(result.clone(), options.clone())
        .await;
    let wake = tokio::spawn(session.maybe_start_turn_for_pending_work());
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("startup gate");
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
    release.notify_one();
    timeout(Duration::from_secs(5), wake)
        .await
        .expect("cancelled start returns")
        .expect("wake task");
    if !followup {
        assert!(
            session.input_queue.has_completion_mailbox_items().await,
            "restored result must retain its Stop-sensitive completion classification"
        );
        timeout(
            Duration::from_secs(5),
            session.maybe_start_turn_for_pending_work(),
        )
        .await
        .expect("Stop blocks the next wake even with durable sleep");
    }
    assert!(session.active_turn.lock().await.is_none());
    let (mail, restored_options) = session.input_queue.drain_mailbox_input_items().await;
    assert_eq!(mail, vec![TurnInput::InterAgentCommunication(result)]);
    if followup {
        assert_eq!(restored_options.turn_trigger, options.turn_trigger);
        assert_eq!(
            restored_options.final_output_json_schema,
            options.final_output_json_schema
        );
        assert_eq!(restored_options.parent_turn_id, options.parent_turn_id);
        assert_eq!(restored_options.root_turn_id, options.root_turn_id);
        assert_eq!(
            restored_options.cyber_access_program,
            options.cyber_access_program
        );
    }
}

#[test_case::test_case(false; "concurrent_wakes_share_one_reservation")]
#[test_case::test_case(true; "stop_during_start_cannot_resurrect_turn")]
#[tokio::test]
async fn completion_wake_reservation_is_atomic(stop: bool) {
    let (mut session, _turn, _rx) = make_session_and_context_with_rx().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.turn_lifecycle_contributor(Arc::new(StartGate {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
        starts: Arc::clone(&starts),
    }));
    Arc::get_mut(&mut session)
        .expect("unique session")
        .services
        .extensions = Arc::new(extensions.build());
    let result = enqueue_result(&session, "result at startup").await;
    let wake = tokio::spawn(session.maybe_start_turn_for_pending_work());
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("reserved start reaches lifecycle");
    let reservation = Arc::clone(
        &session
            .active_turn
            .lock()
            .await
            .as_ref()
            .expect("reservation")
            .turn_state,
    );
    let second = enqueue_result(&session, "concurrent result").await;
    session.maybe_start_turn_for_pending_work().await;
    assert!(Arc::ptr_eq(
        &reservation,
        &session
            .active_turn
            .lock()
            .await
            .as_ref()
            .expect("same reservation")
            .turn_state
    ));
    if stop {
        session.abort_all_tasks(TurnAbortReason::Interrupted).await;
    }
    release.notify_one();
    timeout(Duration::from_secs(5), wake)
        .await
        .expect("start finishes")
        .expect("wake task");
    if stop {
        session.maybe_start_turn_for_pending_work().await;
        assert!(session.active_turn.lock().await.is_none());
        let mail = session.input_queue.drain_mailbox_input_items().await.0;
        assert!(mail.contains(&TurnInput::InterAgentCommunication(result)));
        assert!(mail.contains(&TurnInput::InterAgentCommunication(second)));
    } else {
        // The regular task can already have finished against this fixture's offline provider.
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        session.abort_all_tasks(TurnAbortReason::Interrupted).await;
    }
}
