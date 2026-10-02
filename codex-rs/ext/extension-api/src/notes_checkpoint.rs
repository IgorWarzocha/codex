use std::collections::HashMap;
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
    // None until a write completes. False is sticky only within this host batch.
    batches: Vec<Option<bool>>,
    current_batch: usize,
    cell_batches: HashMap<String, usize>,
    pending_writes: usize,
}

/// Host sampling identity captured before async cell startup and retained across waits.
pub struct NotesCheckpointBatch {
    turn_id: String,
    generation: u64,
    batch_index: usize,
}

/// Captured before the backend operation. Late results from superseded work
/// cannot authorize a newer checkpoint, even when a steer reuses the turn id.
pub struct NotesWriteAttempt {
    batch: NotesCheckpointBatch,
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
            batches: vec![None],
            current_batch: 0,
            cell_batches: HashMap::new(),
            pending_writes: 0,
        });
    }

    /// Start the next host sampling batch. Direct sibling calls share this boundary,
    /// even if they execute sequentially. A batch without writes does not supersede notes.
    pub fn begin_batch(&self, turn_id: &str) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = run.as_mut().filter(|run| run.turn_id == turn_id) {
            run.current_batch = run.batches.len();
            run.batches.push(None);
        }
    }

    pub fn begin_write(&self, turn_id: &str) -> Option<NotesWriteAttempt> {
        self.begin_write_in_batch(turn_id, None)
    }

    pub fn capture_batch(&self, turn_id: &str) -> Option<NotesCheckpointBatch> {
        self.run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|run| run.turn_id == turn_id)
            .map(|run| NotesCheckpointBatch {
                turn_id: turn_id.to_owned(),
                generation: run.generation,
                batch_index: run.current_batch,
            })
    }

    /// Register before releasing the cell's nested-dispatch gate. Sibling cells and
    /// direct writes share the original host batch, not their completion order.
    pub fn register_cell(&self, batch: NotesCheckpointBatch, cell_id: &str) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = run
            .as_mut()
            .filter(|run| run.turn_id == batch.turn_id && run.generation == batch.generation)
        {
            run.cell_batches
                .entry(cell_id.to_owned())
                .or_insert(batch.batch_index);
        }
    }

    /// A yielded cell keeps its original batch even if its first write starts later.
    /// Unregistered or superseded cells cannot authorize a checkpoint.
    pub fn begin_cell_write(&self, turn_id: &str, cell_id: &str) -> Option<NotesWriteAttempt> {
        self.begin_write_in_batch(turn_id, Some(cell_id))
    }

    fn begin_write_in_batch(
        &self,
        turn_id: &str,
        cell_id: Option<&str>,
    ) -> Option<NotesWriteAttempt> {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let run = run.as_mut().filter(|run| run.turn_id == turn_id)?;
        let batch = if let Some(cell_id) = cell_id {
            *run.cell_batches.get(cell_id)?
        } else {
            run.current_batch
        };
        // A dropped/cancelled attempt remains pending and cannot authorize settlement.
        run.pending_writes += 1;
        Some(NotesWriteAttempt {
            batch: NotesCheckpointBatch {
                turn_id: turn_id.to_owned(),
                generation: run.generation,
                batch_index: batch,
            },
        })
    }

    pub fn finish_write(&self, attempt: NotesWriteAttempt, succeeded: bool) {
        let mut run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = run.as_mut().filter(|run| {
            run.turn_id == attempt.batch.turn_id && run.generation == attempt.batch.generation
        }) {
            run.pending_writes -= 1;
            *run.batches[attempt.batch.batch_index].get_or_insert(true) &= succeeded;
        }
    }

    pub fn has_successful_notes(&self, turn_id: &str) -> bool {
        self.run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|run| {
                run.turn_id == turn_id
                    && run.pending_writes == 0
                    && run.batches.iter().rev().find_map(|outcome| *outcome) == Some(true)
            })
    }
}
