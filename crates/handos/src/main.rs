#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod cli;
mod daemon;
mod trace_cmd;
mod tracker_proc;

use std::path::PathBuf;

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
    let draws = match &cli.command {
        Command::Daemon { headless, .. }
        | Command::Trace {
            command: TraceCommand::Replay { headless, .. },
        } => !headless,
        Command::Trace {
            command: TraceCommand::Record { .. },
        } => true,
    };
    if draws {
        // Settings (D1) will pass `overlay.vulkan_driver` here.
        #[allow(unsafe_code)]
        // SAFETY: nothing has spawned a thread yet (the tracing subscriber starts none).
        let _driver = unsafe { handos_platform_wayland::vulkan::select_driver(None) };
    }
    match cli.command {
        Command::Daemon {
            record,
            tracker,
            headless,
        } => daemon::run(&daemon::Options {
            tracker,
            record,
            headless,
        }),
        Command::Trace {
            command: TraceCommand::Record { file },
        } => daemon::run(&daemon::Options {
            tracker: PathBuf::from("handos-tracker-mp"),
            record: Some(file),
            headless: false,
        }),
        Command::Trace {
            command:
                TraceCommand::Replay {
                    file,
                    speed,
                    headless,
                    awake,
                },
        } => trace_cmd::replay(
            &file,
            &trace_cmd::ReplayOptions {
                speed,
                headless,
                awake,
            },
        ),
    }
}
