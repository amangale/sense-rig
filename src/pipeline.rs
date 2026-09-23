use tokio::sync::mpsc;

use crate::stats::Stats;
use crate::types::SourceEvent;
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
    seen: std::collections::HashSet<u64>,
    high_water: u64,
    window: u64,
}

impl Deduplicator {
    pub fn new(window: u64) -> Self {
        assert!(window > 0, "window must be nonzero");
        Deduplicator {
            seen: std::collections::HashSet::new(),
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

/// Async pipeline task: consumes a bounded channel of events, runs the
/// deduplicator, accumulates stats.
///
/// Ownership note: `Deduplicator` and `Stats` live exclusively in this
/// task; no mutex. Dropping the sender causes `rx.recv()` to return
/// `None`, ending the loop — graceful shutdown by channel closure.
pub async fn run_pipeline(mut rx: mpsc::Receiver<SourceEvent>, window: u64) -> Stats {
    let mut dd = Deduplicator::new(window);
    let mut stats = Stats::default();

    while let Some(event) = rx.recv().await {
        match event {
            SourceEvent::Reading(r) => stats.record(dd.admit(&r)),
            SourceEvent::Disconnected => stats.record_disconnect(),
        }
    }

    stats
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

        #[tokio::test]
    async fn run_pipeline_adjudicates_and_shuts_down_on_sender_drop() {
        let (tx, rx) = mpsc::channel(4);

        // Sender must be its own task: an awaited send on a full channel
        // parks the task, so the consumer must already be scheduled
        // (single-threaded test runtime).
        let sender = tokio::spawn(async move {
            for seq in 0..5 {
                tx.send(SourceEvent::Reading(reading(seq))).await.unwrap();
            }
            tx.send(SourceEvent::Disconnected).await.unwrap();
            // tx drops here: pipeline must observe closure and exit.
        });

        let stats = run_pipeline(rx, 64).await;
        sender.await.unwrap();

        let t = stats.totals();
        assert_eq!(t.delivered, 5);
        assert_eq!(t.disconnected, 1);
        assert_eq!(stats.total_adjudicated(), 5);
    }

    #[tokio::test]
    async fn run_pipeline_counts_duplicates_across_channel() {
        let (tx, rx) = mpsc::channel(2);

        let sender = tokio::spawn(async move {
            for seq in [3, 3, 4] {
                tx.send(SourceEvent::Reading(reading(seq))).await.unwrap();
            }
        });

        let stats = run_pipeline(rx, 64).await;
        sender.await.unwrap();

        assert_eq!(stats.totals().delivered, 2);
        assert_eq!(stats.totals().duplicate, 1);
    }
}