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

    /// Buffer-level drop (backpressure eviction, phase 3). Kept distinct
    /// from source-level drops in the API even though both land in the
    /// same counter — the caller decides which losses they caused.
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
    /// the source never emitted).
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
        writeln!(f, "dropped      {:>8} (source + buffer)", t.dropped)?;
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
}