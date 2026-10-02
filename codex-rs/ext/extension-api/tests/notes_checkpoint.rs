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

#[test]
fn a_clean_retry_batch_supersedes_failure_but_empty_batches_do_not() {
    let tracker = NotesCheckpointTracker::default();
    tracker.begin_run("run");
    tracker.finish_write(tracker.begin_write("run").unwrap(), false);
    // Sequential siblings, not just overlapping operations, share the failure.
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    assert!(!tracker.has_successful_notes("run"));
    tracker.begin_batch("run");
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    assert!(tracker.has_successful_notes("run"));
    tracker.begin_batch("run");
    assert!(tracker.has_successful_notes("run"));
    tracker.finish_write(tracker.begin_write("run").unwrap(), false);
    assert!(!tracker.has_successful_notes("run"));
    tracker.begin_batch("unselected");
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    assert!(!tracker.has_successful_notes("run"));
}

#[test]
fn parallel_partial_writes_deny_freshness_in_either_completion_order() {
    for failure_first in [true, false] {
        let tracker = NotesCheckpointTracker::default();
        tracker.begin_run("run");
        let first = tracker.begin_write("run").unwrap();
        let second = tracker.begin_write("run").unwrap();
        tracker.finish_write(first, !failure_first);
        assert!(!tracker.has_successful_notes("run"));
        tracker.finish_write(second, failure_first);
        assert!(!tracker.has_successful_notes("run"));
        tracker.begin_batch("run");
        let first = tracker.begin_write("run").unwrap();
        let second = tracker.begin_write("run").unwrap();
        tracker.finish_write(first, true);
        assert!(!tracker.has_successful_notes("run"));
        tracker.finish_write(second, true);
        assert!(tracker.has_successful_notes("run"));
    }
}

#[test]
fn batch_order_not_completion_order_selects_the_checkpoint() {
    for latest_succeeded in [true, false] {
        let tracker = NotesCheckpointTracker::default();
        tracker.begin_run("run");
        let old = tracker.begin_write("run").unwrap();
        tracker.begin_batch("run");
        tracker.finish_write(tracker.begin_write("run").unwrap(), latest_succeeded);
        assert!(!tracker.has_successful_notes("run"));
        tracker.finish_write(old, !latest_succeeded);
        assert_eq!(tracker.has_successful_notes("run"), latest_succeeded);
    }
}

#[test]
fn an_unfinished_attempt_cannot_be_hidden_by_a_later_successful_batch() {
    let tracker = NotesCheckpointTracker::default();
    tracker.begin_run("run");
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    let cancelled = tracker.begin_write("run").unwrap();
    assert!(!tracker.has_successful_notes("run"));
    drop(cancelled);
    tracker.begin_batch("run");
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    assert!(!tracker.has_successful_notes("run"));
}

#[test]
fn nested_writes_share_cell_identity_across_sampling_and_wait_boundaries() {
    let tracker = NotesCheckpointTracker::default();
    tracker.begin_run("run");
    tracker.register_cell(tracker.capture_batch("run").unwrap(), "failed-cell");
    tracker.finish_write(
        tracker.begin_cell_write("run", "failed-cell").unwrap(),
        false,
    );
    tracker.begin_batch("run");
    tracker.finish_write(
        tracker.begin_cell_write("run", "failed-cell").unwrap(),
        true,
    );
    assert!(!tracker.has_successful_notes("run"));
    tracker.register_cell(tracker.capture_batch("run").unwrap(), "retry-cell");
    tracker.finish_write(tracker.begin_cell_write("run", "retry-cell").unwrap(), true);
    assert!(tracker.has_successful_notes("run"));
    tracker.begin_batch("run");
    assert!(tracker.has_successful_notes("run"));
    // Direct writes in a later host batch can also supersede a failed nested cell.
    tracker.finish_write(tracker.begin_write("run").unwrap(), true);
    assert!(tracker.has_successful_notes("run"));
}

#[test]
fn sibling_cells_and_direct_writes_share_failure_until_a_later_response() {
    for direct_sibling in [true, false] {
        let tracker = NotesCheckpointTracker::default();
        tracker.begin_run("run");
        tracker.register_cell(tracker.capture_batch("run").unwrap(), "failed-cell");
        tracker.register_cell(tracker.capture_batch("run").unwrap(), "sibling-cell");
        tracker.finish_write(
            tracker.begin_cell_write("run", "failed-cell").unwrap(),
            false,
        );
        let success = if direct_sibling {
            tracker.begin_write("run").unwrap()
        } else {
            tracker.begin_cell_write("run", "sibling-cell").unwrap()
        };
        tracker.finish_write(success, true);
        assert!(!tracker.has_successful_notes("run"));
        tracker.begin_batch("run");
        tracker.register_cell(tracker.capture_batch("run").unwrap(), "retry-cell");
        tracker.finish_write(tracker.begin_cell_write("run", "retry-cell").unwrap(), true);
        assert!(tracker.has_successful_notes("run"));
    }
}

#[test]
fn first_late_cell_write_keeps_its_original_batch_and_stale_registration_is_denied() {
    let tracker = NotesCheckpointTracker::default();
    tracker.begin_run("run");
    tracker.finish_write(tracker.begin_write("run").unwrap(), false);
    tracker.register_cell(tracker.capture_batch("run").unwrap(), "yielded-cell");
    tracker.begin_batch("run");
    tracker.finish_write(
        tracker.begin_cell_write("run", "yielded-cell").unwrap(),
        true,
    );
    assert!(!tracker.has_successful_notes("run"));
    let stale_batch = tracker.capture_batch("run").unwrap();
    tracker.begin_run("run");
    tracker.register_cell(stale_batch, "stale-cell");
    assert!(tracker.begin_cell_write("run", "stale-cell").is_none());
    assert!(tracker.begin_cell_write("run", "yielded-cell").is_none());
    assert!(!tracker.has_successful_notes("run"));
}
