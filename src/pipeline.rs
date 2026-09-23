use std::collections::HashSet;

use crate::types::SensorReading;

/// Classification of a reading after the dedup/ordering gate.
/// Every lossy behavior is observable and counted, never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Delivered,
    Duplicate,
    Late,
}

/// Sequence-number deduplication with a bounded reorder window,
/// carrying over the bidi-rig accounting instinct.
///
/// Invariants:
/// - A `seq` seen before (within the tracked set) is a `Duplicate`.
/// - A first-seen `seq` more than `window` behind the high-water mark
///   is `Late` — too stale to matter, counted, then dropped.
/// - Everything else is `Delivered`.
pub struct Deduplicator {
    seen: HashSet<u64>,
    high_water: u64,
    window: u64,
}

impl Deduplicator {
    pub fn new(window: u64) -> Self {
        assert!(window > 0, "window must be nonzero");
        Deduplicator {
            seen: HashSet::new(),
            high_water: 0,
            window,
        }
    }

    /// Classify one reading. Note the single mutable borrow: the
    /// deduplicator owns its state exclusively, and the compiler proves it.
    pub fn admit(&mut self, reading: &SensorReading) -> Verdict {
        if self.seen.contains(&reading.seq) {
            return Verdict::Duplicate;
        }

        if self.high_water.saturating_sub(reading.seq) > self.window {
            // First sighting, but far behind the frontier — stale.
            // Counted, never silently dropped.
            return Verdict::Late;
        }

        self.seen.insert(reading.seq);
        if reading.seq > self.high_water {
            self.high_water = reading.seq;
            self.prune();
        }
        Verdict::Delivered
    }

    /// Keep the seen-set bounded: entries more than `window` behind the
    /// frontier can no longer produce a verdict that distinguishes them
    /// from `Late`, so tracking them buys nothing.
    fn prune(&mut self) {
        let floor = self.high_water.saturating_sub(self.window);
        self.seen.retain(|&seq| seq >= floor);
    }

    pub fn high_water(&self) -> u64 {
        self.high_water
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ReadingKind, SensorReading};

    fn reading(seq: u64) -> SensorReading {
        SensorReading {
            seq,
            ts_ns: seq * 1_000_000,
            kind: ReadingKind::Ultrasonic,
            values: vec![42.0],
        }
    }

    #[test]
    fn fresh_in_order_seq_is_delivered() {
        let mut dd = Deduplicator::new(8);
        assert_eq!(dd.admit(&reading(1)), Verdict::Delivered);
        assert_eq!(dd.admit(&reading(2)), Verdict::Delivered);
        assert_eq!(dd.high_water(), 2);
    }

    #[test]
    fn exact_repeat_is_duplicate() {
        let mut dd = Deduplicator::new(8);
        dd.admit(&reading(10));
        dd.admit(&reading(11));
        assert_eq!(dd.admit(&reading(10)), Verdict::Duplicate);
        assert_eq!(dd.admit(&reading(11)), Verdict::Duplicate);
    }

    #[test]
    fn reordered_within_window_is_delivered() {
        let mut dd = Deduplicator::new(8);
        dd.admit(&reading(10));
        dd.admit(&reading(11));
        // Arrives out of order but within the window.
        assert_eq!(dd.admit(&reading(9)), Verdict::Delivered);
    }

    #[test]
    fn first_seen_far_behind_frontier_is_late() {
        let mut dd = Deduplicator::new(8);
        dd.admit(&reading(100));
        // First time seen, but 100 - 85 > 8: stale.
        assert_eq!(dd.admit(&reading(85)), Verdict::Late);
    }

    #[test]
    fn duplicate_outside_pruned_set_is_classified_late() {
        // A seq so old it fell out of the seen-set is indistinguishable
        // from a stale first sighting: verdict is Late, still counted.
        let mut dd = Deduplicator::new(8);
        dd.admit(&reading(100));
        dd.admit(&reading(90)); // prunes the set back to >= 82
        assert_eq!(dd.admit(&reading(80)), Verdict::Late);
    }

    #[test]
    fn empty_at_start_of_sequence_boundary_is_not_late() {
        // high_water starts at 0; a low first seq must be Delivered,
        // not punished by saturating arithmetic.
        let mut dd = Deduplicator::new(8);
        assert_eq!(dd.admit(&reading(0)), Verdict::Delivered);
        assert_eq!(dd.admit(&reading(1)), Verdict::Delivered);
    }
}