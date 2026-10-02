//! One payload policy for checkpoint capture, durable restore and journal rotation.

const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct PersistenceBudget {
    payload_bytes: usize,
}

impl PersistenceBudget {
    pub(crate) fn from_heap_mib(max_heap_mib: Option<u32>) -> Self {
        // An unconfigured heap has no fixed upper bound. Persistence still does.
        let relative = max_heap_mib.map_or(256 * MIB, |mib| {
            (u64::from(mib) * MIB as u64 / 8).min(256 * MIB as u64) as usize
        });
        Self {
            payload_bytes: relative.clamp(8 * MIB, 256 * MIB),
        }
    }

    pub(crate) fn payload_bytes(self) -> usize {
        self.payload_bytes
    }

    pub(crate) fn snapshot_json_bytes(self) -> usize {
        // Values travel as base64, padded separately for at most 10000 bindings.
        // Keep the reference's 8 MiB manifest allowance independent of payload.
        self.payload_bytes.div_ceil(3) * 4 + 4 * 10_000 + 8 * MIB
    }

    pub(crate) fn file_bytes(self) -> usize {
        // Private session files contain both a snapshot and its merge baseline.
        self.snapshot_json_bytes() * 2 + 4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_relative_payload_clamps_and_json_envelopes() {
        for (heap, expected) in [
            (Some(1), 8 * MIB),
            (Some(64), 8 * MIB),
            (Some(65), 65 * MIB / 8),
            (Some(512), 64 * MIB),
            (Some(2048), 256 * MIB),
            (Some(u32::MAX), 256 * MIB),
            (None, 256 * MIB),
        ] {
            let budget = PersistenceBudget::from_heap_mib(heap);
            assert_eq!(budget.payload_bytes(), expected);
            assert!(budget.snapshot_json_bytes() > expected.div_ceil(3) * 4);
            assert!(budget.file_bytes() > 2 * budget.snapshot_json_bytes());
        }
    }
}
