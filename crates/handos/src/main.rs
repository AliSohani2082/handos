mod cli;
mod daemon;
mod trace_cmd;
mod tracker_proc;

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, TraceCommand};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .with_env_var("HANDOS_LOG")
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(anyhow::Error::msg)?;
    match cli.command {
        Command::Daemon { record, tracker } => daemon::run(&tracker, record.as_deref()),
        Command::Trace {
            command: TraceCommand::Record { file },
        } => daemon::run(Path::new("handos-tracker-mp"), Some(&file)),
        Command::Trace {
            command:
                TraceCommand::Replay {
                    file,
                    speed,
                    headless: _,
                },
        } => trace_cmd::replay(&file, speed),
    }
}
