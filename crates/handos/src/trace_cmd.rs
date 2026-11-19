#![allow(unsafe_code)]

use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use handos_proto::{
    MSG_FRAME, decode_frame,
    trace::{TraceReader, TraceWriter},
};

use crate::daemon::Frames;

pub fn writer(path: &Path) -> Result<TraceWriter<BufWriter<File>>> {
    let file =
        File::create_new(path).with_context(|| format!("create trace {}", path.display()))?;
    Ok(TraceWriter::new(BufWriter::new(file), monotonic_us()?)?)
}

fn monotonic_us() -> std::io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: time points to a writable timespec for the duration of the call.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1000)
}

pub fn replay(path: &Path, speed: f64) -> Result<()> {
    if !speed.is_finite() || speed < 0.0 {
        bail!("replay speed must be finite and non-negative");
    }
    let file = File::open(path).with_context(|| format!("open trace {}", path.display()))?;
    let mut reader = TraceReader::new(BufReader::new(file))?;
    let mut packet = Vec::new();
    let mut first = None;
    let mut previous = None;
    let mut frames = Frames::default();
    let started = Instant::now();
    while reader.read_packet(&mut packet)? {
        if packet[0] != MSG_FRAME {
            continue;
        }
        let frame =
            decode_frame(&packet).with_context(|| format!("decode frame {}", frames.count))?;
        let now = frame.t_capture_us;
        if previous.is_some_and(|last| last > now) {
            bail!(
                "trace capture time moved backwards at frame {}",
                frames.count
            );
        }
        previous = Some(now);
        let base = *first.get_or_insert(now);
        if speed > 0.0 {
            let target = Duration::try_from_secs_f64((now - base) as f64 / (1_000_000.0 * speed))
                .context("trace time is out of range at this replay speed")?;
            if let Some(wait) = target.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        frames.on_frame(&frame);
    }
    tracing::info!(frames = frames.count, "replay complete");
    Ok(())
}
