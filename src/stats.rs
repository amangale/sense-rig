//! Delivery-semantics counters. Every branch the pipeline can take has a
//! number here. The summary line is the artifact an interviewer reads.

use std::fmt;

use serde::Serialize;

use crate::pipeline::Verdict;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub delivered: u64,
    pub duplicate: u64,
    pub late: u64,
    pub dropped: u64,
    pub disconnected: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Stats {
    totals: Totals,
}

impl Stats {
    pub fn record(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Delivered => self.totals.delivered += 1,
            Verdict::Duplicate => self.totals.duplicate += 1,
            Verdict::Late => self.totals.late += 1,
        }
    }

    /// Buffer-level drop (channel eviction). Kept distinct from
    /// source-level drops in the API even though both land in the same
    /// counter — the caller decides which losses they caused.
    pub fn record_drop(&mut self) {
        self.totals.dropped += 1;
    }

    pub fn record_disconnect(&mut self) {
        self.totals.disconnected += 1;
    }

    pub fn totals(&self) -> Totals {
        self.totals
    }

    /// Readings the deduplicator actually adjudicated (excludes the ones
    /// the source never emitted or the channel evicted).
    pub fn total_adjudicated(&self) -> u64 {
        let t = self.totals;
        t.delivered + t.duplicate + t.late
    }
}

fn pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1}%", 100.0 * part as f64 / whole as f64)
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = self.totals;
        let adj = self.total_adjudicated();
        writeln!(f, "--- pipeline (adjudicated: {adj}) ---")?;
        writeln!(
            f,
            "delivered    {:>8} ({})",
            t.delivered,
            pct(t.delivered, adj)
        )?;
        writeln!(
            f,
            "duplicate    {:>8} ({})",
            t.duplicate,
            pct(t.duplicate, adj)
        )?;
        writeln!(f, "late         {:>8} ({})", t.late, pct(t.late, adj))?;
        writeln!(f, "dropped      {:>8} (source + channel)", t.dropped)?;
        writeln!(f, "disconnected {:>8}", t.disconnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Verdict;

    #[test]
    fn counts_verdicts_by_branch() {
        let mut s = Stats::default();
        s.record(Verdict::Delivered);
        s.record(Verdict::Delivered);
        s.record(Verdict::Duplicate);
        s.record(Verdict::Late);
        assert_eq!(s.total_adjudicated(), 4);
        assert_eq!(s.totals().delivered, 2);
    }

    #[test]
    fn displays_percentages_without_dividing_by_zero() {
        let mut s = Stats::default();
        s.record_disconnect();
        // No readings adjudicated yet: must not panic, must not print NaN.
        let text = s.to_string();
        assert!(text.contains("delivered"));
        assert!(text.contains("n/a"));
    }

    #[test]
    fn drop_and_disconnect_counters_are_independent_of_verdicts() {
        // Lossy paths must not contaminate adjudication math: total_
        // adjudicated counts only verdict branches, never dropped or
        // disconnected events.
        let mut s = Stats::default();
        s.record_drop();
        s.record_drop();
        s.record_disconnect();
        assert_eq!(s.total_adjudicated(), 0);
        assert_eq!(s.totals().dropped, 2);
        assert_eq!(s.totals().disconnected, 1);
    }

    #[test]
    fn default_stats_display_makes_sense_on_an_empty_run() {
        // A zero-tick run still prints; every line must carry n/a or 0.
        let text = Stats::default().to_string();
        assert!(text.contains("adjudicated: 0"));
        assert!(text.contains("dropped"));
        assert!(text.contains("disconnected"));
    }

    #[tokio::test]
    async fn full_run_conservation_through_the_pipeline() {
        // End-to-end: simulator -> channel -> run_pipeline -> Stats.
        // With a clean channel and generous window, every emission is
        // adjudicated exactly once: adjudicated == source-emitted.
        use crate::pipeline::run_pipeline;
        use crate::source::{run_source_task, SourceConfig};
        use crate::types::SourceEvent;

        let config = SourceConfig {
            dup_prob: 0.1,
            drop_prob: 0.05,
            disconnect_prob: 0.02,
            ..Default::default()
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<SourceEvent>(256);
        let source = tokio::spawn(run_source_task(config, 500, tx));
        let stats = run_pipeline(rx, 64).await;

        let (sim, channel_drops) = source.await.unwrap().unwrap();
        assert_eq!(channel_drops, 0, "generous channel must not evict");

        let counts = sim.counts();
        assert_eq!(
            stats.total_adjudicated(),
            counts.emitted,
            "adjudicated must equal source emissions"
        );
        // And the duplicate classifier catches exactly what the source
        // injected — two independent observers, one number.
        assert_eq!(stats.totals().duplicate, counts.duplicated);
        assert_eq!(stats.totals().delivered, counts.generated - counts.dropped);
    }
}