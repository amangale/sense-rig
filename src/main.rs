mod pipeline;
mod source;
mod stats;
mod types;

use clap::Parser;

use crate::pipeline::Deduplicator;
use crate::source::{Simulator, SourceConfig};
use crate::stats::Stats;
use crate::types::SourceEvent;

#[derive(Parser, Debug)]
#[command(
    name = "sense-rig",
    version,
    about = "Sensor-data ingestion with accountable delivery semantics"
)]
struct Args {
    /// Ticks of the source clock to simulate
    #[arg(long, default_value_t = 500)]
    ticks: usize,

    /// Sensor emission rate
    #[arg(long, default_value_t = 10.0)]
    rate_hz: f64,

    /// Timestamp scatter (ms) — produces out-of-order arrivals
    #[arg(long, default_value_t = 5)]
    jitter_ms: u64,

    /// Probability a reading is duplicated on the wire
    #[arg(long, default_value_t = 0.05)]
    dup_prob: f64,

    /// Probability a reading is dropped before emission
    #[arg(long, default_value_t = 0.02)]
    drop_prob: f64,

    /// Probability per tick of a simulated disconnect
    #[arg(long, default_value_t = 0.01)]
    disconnect_prob: f64,

    /// Ticks the source stays dark after a disconnect
    #[arg(long, default_value_t = 3)]
    reconnect_ticks: u32,

    /// RNG seed (runs are reproducible)
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Reorder/dedup window size
    #[arg(long, default_value_t = 64)]
    window: u64,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let config = SourceConfig {
        rate_hz: args.rate_hz,
        jitter_ms: args.jitter_ms,
        dup_prob: args.dup_prob,
        drop_prob: args.drop_prob,
        disconnect_prob: args.disconnect_prob,
        reconnect_ticks: args.reconnect_ticks,
        seed: args.seed,
    };

    let mut sim = Simulator::try_new(config)?;
    let mut dd = Deduplicator::new(args.window);
    let mut stats = Stats::default();

    for _ in 0..args.ticks {
        for event in sim.tick() {
            match event {
                SourceEvent::Reading(r) => stats.record(dd.admit(&r)),
                SourceEvent::Disconnected => stats.record_disconnect(),
            }
        }
    }

    println!("{}", sim.counts());
    print!("{stats}");
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}