mod pipeline;
mod source;
mod stats;
mod types;

use clap::Parser;
use tokio::sync::mpsc;

use crate::source::{run_source_task, SourceConfig, Simulator};
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
    #[arg(long, default_value_t = 32)]
    channel_capacity: usize,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let config = SourceConfig {
        rate_hz: args.rate_hz,
        jitter_ms: args.jitter_ms,
        dup_prob: args.dup_prob,
        drop_prob: args.drop_prob,
        disconnect_prob: args.disconnect_prob,
        reconnect_ticks: args.reconnect_ticks,
        seed: args.seed,
    };

    let (tx, rx) = mpsc::channel::<SourceEvent>(args.channel_capacity);

    let source_handle = tokio::spawn(run_source_task(config, args.ticks, tx));
    let pipeline_handle = tokio::spawn(pipeline::run_pipeline(rx, args.window));

    let (source_result, pipeline_result) = tokio::join!(source_handle, pipeline_handle);

    let (sim, channel_drops) = source_result??;
    let pipeline_stats = pipeline_result?;

    println!("{}", sim.counts());
    print!("{pipeline_stats}");
    eprintln!("channel drops: {channel_drops}");
    Ok(())
}

