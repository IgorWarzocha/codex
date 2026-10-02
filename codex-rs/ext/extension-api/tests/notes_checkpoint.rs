use codex_extension_api::NotesCheckpointTracker;

#[test]
fn only_successful_writes_in_selected_run_count() {
    let tracker = NotesCheckpointTracker::default();
    tracker.begin_run("selected");
    assert!(tracker.begin_write("old").is_none());
    assert!(!tracker.has_successful_notes("selected"));
    tracker.finish_write(tracker.begin_write("selected").unwrap(), true);
    assert!(tracker.has_successful_notes("selected"));
    tracker.finish_write(tracker.begin_write("selected").unwrap(), false);
    assert!(!tracker.has_successful_notes("selected"));
    let late_attempt = tracker.begin_write("selected").unwrap();
    tracker.begin_run("selected");
    tracker.finish_write(late_attempt, true);
    assert!(!tracker.has_successful_notes("selected"));
    let old_attempt = tracker.begin_write("selected").unwrap();
    tracker.begin_run("next");
    assert!(!tracker.has_successful_notes("next"));
    tracker.finish_write(old_attempt, true);
    assert!(!tracker.has_successful_notes("next"));
}
