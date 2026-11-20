use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Hands-free desktop pointer")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Track hands and drive the pointer and overlay.
    Daemon {
        #[arg(long)]
        record: Option<PathBuf>,
        #[arg(long, default_value = "handos-tracker-mp")]
        tracker: PathBuf,
        /// No Wayland: frames run through the pipeline, nothing is drawn or sent (tests, CI).
        #[arg(long)]
        headless: bool,
    },
    /// Record or replay landmark packets.
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
}

#[derive(Subcommand)]
pub enum TraceCommand {
    /// Start a daemon and record its incoming tracker packets.
    Record { file: PathBuf },
    /// Replay packets on the trace clock (speed 0 runs as fast as possible).
    Replay {
        file: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// No Wayland: nothing is drawn and the pointer does not move.
        #[arg(long)]
        headless: bool,
        /// Start awake instead of waiting for the open-palm wake gesture.
        #[arg(long)]
        awake: bool,
    },
}
