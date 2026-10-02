use std::sync::Mutex;

/// Host-only note-write outcomes shared by the notes executor and turn settlement.
/// Neither model text nor tool-result JSON can manufacture a successful checkpoint.
#[derive(Default)]
pub struct NotesCheckpointTracker {
    run: Mutex<Option<RunNotes>>,
}

struct RunNotes {
    turn_id: String,
    generation: u64,
    saved: bool,
    failed: bool,
}

/// Captured before the backend operation. Late results from superseded work
/// cannot authorize a newer checkpoint, even when a steer reuses the turn id.
pub struct NotesWriteAttempt {
    turn_id: String,
    generation: u64,
}

impl NotesCheckpointTracker {
    pub fn begin_run(&self, turn_id: &str) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = run.as_ref().map_or(0, |run| run.generation.wrapping_add(1));
        *run = Some(RunNotes {
            turn_id: turn_id.to_owned(),
            generation,
            saved: false,
            failed: false,
        });
    }

    pub fn begin_write(&self, turn_id: &str) -> Option<NotesWriteAttempt> {
        self.run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|run| run.turn_id == turn_id)
            .map(|run| NotesWriteAttempt {
                turn_id: turn_id.to_owned(),
                generation: run.generation,
            })
    }

    pub fn finish_write(&self, attempt: NotesWriteAttempt, succeeded: bool) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = run
            .as_mut()
            .filter(|run| run.turn_id == attempt.turn_id && run.generation == attempt.generation)
        {
            run.saved |= succeeded;
            run.failed |= !succeeded;
        }
    }

    pub fn has_successful_notes(&self, turn_id: &str) -> bool {
        self.run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|run| run.turn_id == turn_id && run.saved && !run.failed)
    }
}
