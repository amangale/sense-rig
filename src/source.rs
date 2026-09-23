//! Source simulator with a fault-injection dial.
//!
//! The simulator owns its clock and its RNG so runs are reproducible from
//! a seed. Every fault decision bumps a counter: generated, dropped,
//! duplicated, disconnects. Nothing is lost silently.

use std::fmt;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;

use crate::types::{ReadingKind, SensorReading, SourceEvent};

#[derive(Debug, Clone)]
pub struct SourceConfig {
    pub rate_hz: f64,
    /// Timestamp scatter applied on top of the period — this is what
    /// produces out-of-order arrivals downstream.
    pub jitter_ms: u64,
    pub dup_prob: f64,
    pub drop_prob: f64,
    /// Probability per tick that the "wire" breaks and the source
    /// goes quiet for `reconnect_ticks` periods.
    pub disconnect_prob: f64,
    pub reconnect_ticks: u32,
    pub seed: u64,
}

impl Default for SourceConfig {
    fn default() -> Self {
        SourceConfig {
            rate_hz: 10.0,
            jitter_ms: 5,
            dup_prob: 0.05,
            drop_prob: 0.02,
            disconnect_prob: 0.01,
            reconnect_ticks: 3,
            seed: 42,
        }
    }
}

/// Bad configuration is a value-level fact, so it gets an enum with
/// payloads rather than a stringly-typed error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceError {
    InvalidRate { rate_hz: f64 },
    InvalidProbability { field: &'static str, value: f64 },
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::InvalidRate { rate_hz } => {
                write!(f, "rate_hz must be finite and > 0, got {rate_hz}")
            }
            SourceError::InvalidProbability { field, value } => {
                write!(f, "{field} must be in [0, 1], got {value}")
            }
        }
    }
}

impl std::error::Error for SourceError {}

/// Conservation law: emitted == generated - dropped + duplicated.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceCounts {
    pub generated: u64,
    pub emitted: u64,
    pub duplicated: u64,
    pub dropped: u64,
    pub disconnects: u64,
}

impl fmt::Display for SourceCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- source ---")?;
        writeln!(f, "generated    {:>8}", self.generated)?;
        writeln!(f, "emitted      {:>8}", self.emitted)?;
        writeln!(f, "duplicated   {:>8}", self.duplicated)?;
        writeln!(f, "dropped      {:>8}", self.dropped)?;
        writeln!(f, "disconnects  {:>8}", self.disconnects)
    }
}

#[derive(Debug)]
pub struct Simulator {
    config: SourceConfig,
    rng: StdRng,
    next_seq: u64,
    now_ns: u64,
    down_for: Option<u32>,
    counts: SourceCounts,
}

impl Simulator {
    pub fn try_new(config: SourceConfig) -> Result<Self, SourceError> {
        Self::validate(&config)?;
        Ok(Simulator {
            rng: StdRng::seed_from_u64(config.seed),
            config,
            next_seq: 0,
            now_ns: 0,
            down_for: None,
            counts: SourceCounts::default(),
        })
    }

    pub fn counts(&self) -> SourceCounts {
        self.counts
    }

    pub fn config(&self) -> &SourceConfig {
        &self.config
    }

    /// Advance one source period. Outage semantics:
    ///
    /// - Tick T: `Disconnected` marker emitted, outage of exactly
    ///   `reconnect_ticks` dark ticks begins (no marker repeats).
    /// - Ticks T+1 .. T+reconnect_ticks: dark, empty.
    /// - Tick T+reconnect_ticks+1: recovery — generation resumes and the
    ///   disconnect dice are NOT rolled this tick. A source that can die
    ///   on the instant it reconnects is indistinguishable from one that
    ///   never recovered; the one-tick grace keeps the dial observable.
    pub fn tick(&mut self) -> Vec<SourceEvent> {
        let recovering = match self.down_for {
            Some(remaining) if remaining > 0 => {
                self.down_for = Some(remaining - 1);
                return Vec::new();
            }
            Some(_) => {
                self.down_for = None;
                true
            }
            None => false,
        };

        if !recovering && self.rng.gen_bool(self.config.disconnect_prob) {
            self.counts.disconnects += 1;
            self.down_for = Some(self.config.reconnect_ticks);
            return vec![SourceEvent::Disconnected];
        }

        let seq = self.next_seq;
        self.next_seq += 1;
        self.counts.generated += 1;

        if self.rng.gen_bool(self.config.drop_prob) {
            self.counts.dropped += 1;
            return Vec::new();
        }

        let period_ns = (1_000_000_000.0 / self.config.rate_hz) as u64;
        self.now_ns += period_ns;
        let jitter_ns = self.rng.gen_range(0..=self.config.jitter_ms) * 1_000_000;

        let kind = if seq % 2 == 0 {
            ReadingKind::Ultrasonic
        } else {
            ReadingKind::Imu
        };
        let values = match kind {
            ReadingKind::Ultrasonic => vec![self.rng.gen_range(5.0..250.0)],
            ReadingKind::Imu => (0..6).map(|_| self.rng.gen_range(-2.0..2.0)).collect(),
        };

        let reading = SensorReading {
            seq,
            ts_ns: self.now_ns + jitter_ns,
            kind,
            values,
        };

        if self.rng.gen_bool(self.config.dup_prob) {
            self.counts.duplicated += 1;
            self.counts.emitted += 2;
            return vec![
                SourceEvent::Reading(reading.clone()),
                SourceEvent::Reading(reading),
            ];
        }

        self.counts.emitted += 1;
        vec![SourceEvent::Reading(reading)]
    }

    fn validate(config: &SourceConfig) -> Result<(), SourceError> {
        if !(config.rate_hz.is_finite() && config.rate_hz > 0.0) {
            return Err(SourceError::InvalidRate {
                rate_hz: config.rate_hz,
            });
        }
        Self::check_prob("dup_prob", config.dup_prob)?;
        Self::check_prob("drop_prob", config.drop_prob)?;
        Self::check_prob("disconnect_prob", config.disconnect_prob)
    }

    fn check_prob(field: &'static str, value: f64) -> Result<(), SourceError> {
        if (0.0..=1.0).contains(&value) {
            Ok(())
        } else {
            Err(SourceError::InvalidProbability { field, value })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean_config() -> SourceConfig {
        SourceConfig {
            dup_prob: 0.0,
            drop_prob: 0.0,
            disconnect_prob: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn rejects_bad_probability() {
        let cfg = SourceConfig {
            dup_prob: 1.5,
            ..clean_config()
        };
        assert_eq!(
            Simulator::try_new(cfg).unwrap_err(),
            SourceError::InvalidProbability {
                field: "dup_prob",
                value: 1.5
            }
        );
    }

    #[test]
    fn rejects_nan_probability() {
        let cfg = SourceConfig {
            drop_prob: f64::NAN,
            ..clean_config()
        };
        assert!(matches!(
            Simulator::try_new(cfg),
            Err(SourceError::InvalidProbability { .. })
        ));
    }

    #[test]
    fn rejects_non_positive_rate() {
        let cfg = SourceConfig {
            rate_hz: 0.0,
            ..clean_config()
        };
        assert_eq!(
            Simulator::try_new(cfg).unwrap_err(),
            SourceError::InvalidRate { rate_hz: 0.0 }
        );
    }

    #[test]
    fn dup_prob_one_duplicates_everything() {
        let cfg = SourceConfig {
            dup_prob: 1.0,
            ..clean_config()
        };
        let mut sim = Simulator::try_new(cfg).unwrap();
        for _ in 0..100 {
            let events = sim.tick();
            match events.as_slice() {
                [SourceEvent::Reading(a), SourceEvent::Reading(b)] => {
                    assert_eq!(a, b, "duplicate pair must be byte-identical");
                }
                other => panic!("expected exactly two readings, got {other:?}"),
            }
        }
        let c = sim.counts();
        assert_eq!(c.generated, 100);
        assert_eq!(c.duplicated, 100);
        assert_eq!(c.emitted, 200);
    }

    #[test]
    fn drop_prob_one_emits_nothing() {
        let cfg = SourceConfig {
            drop_prob: 1.0,
            ..clean_config()
        };
        let mut sim = Simulator::try_new(cfg).unwrap();
        for _ in 0..100 {
            assert!(sim.tick().is_empty(), "drop_prob=1 must never emit");
        }
        let c = sim.counts();
        assert_eq!(c.generated, 100);
        assert_eq!(c.dropped, 100);
        assert_eq!(c.emitted, 0);
    }

    #[test]
    fn disconnect_outage_is_exact_and_recovery_has_grace() {
        let cfg = SourceConfig {
            disconnect_prob: 1.0,
            reconnect_ticks: 3,
            ..clean_config()
        };
        let mut sim = Simulator::try_new(cfg).unwrap();

        // Tick 1: marker fires, outage begins. It must not repeat.
        assert!(matches!(
            sim.tick().as_slice(),
            [SourceEvent::Disconnected]
        ));

        // Ticks 2-4: exactly reconnect_ticks dark ticks.
        assert!(sim.tick().is_empty());
        assert!(sim.tick().is_empty());
        assert!(sim.tick().is_empty());

        // Tick 5: recovery — grace tick, no dice rolled, reading flows.
        assert!(matches!(sim.tick().as_slice(), [SourceEvent::Reading(_)]));

        // Tick 6: back to rolling loaded dice. Certain disconnect.
        assert!(matches!(
            sim.tick().as_slice(),
            [SourceEvent::Disconnected]
        ));
        assert_eq!(sim.counts().disconnects, 2);
    }

    #[test]
    fn conservation_law_holds_under_mixed_faults() {
        let mut sim = Simulator::try_new(SourceConfig::default()).unwrap();
        for _ in 0..2000 {
            sim.tick();
        }
        let c = sim.counts();
        assert_eq!(c.emitted + c.dropped, c.generated + c.duplicated);
    }

    #[test]
    fn timestamps_never_regress_more_than_jitter() {
        let cfg = SourceConfig {
            jitter_ms: 50,
            rate_hz: 10.0,
            ..clean_config()
        };
        let mut sim = Simulator::try_new(cfg).unwrap();
        let mut prev_ts = 0;
        for _ in 0..500 {
            for event in sim.tick() {
                if let SourceEvent::Reading(r) = event {
                    assert!(r.ts_ns + 50_000_000 >= prev_ts);
                    prev_ts = r.ts_ns;
                }
            }
        }
    }
}