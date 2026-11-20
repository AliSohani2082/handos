#![allow(unsafe_code)]

use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use handos::pipeline::{Config, Headless, Pipeline};
use handos_gesture::clutch::Mode;
use handos_platform::Desktop;
use handos_platform_wayland::{EventQueue, OverlayOutputs, Wayland, dispatch, hyprland};
use handos_proto::{
    MSG_FRAME, decode_frame,
    trace::{TraceReader, TraceWriter},
};

use crate::daemon::Frames;

/// While waiting for the next frame, Wayland events are handled at least this often.
const DISPATCH_SLICE: Duration = Duration::from_millis(20);

pub fn writer(path: &Path) -> Result<TraceWriter<BufWriter<File>>> {
    let file =
        File::create_new(path).with_context(|| format!("create trace {}", path.display()))?;
    Ok(TraceWriter::new(BufWriter::new(file), monotonic_us()?)?)
}

/// CLOCK_MONOTONIC in microseconds: the clock the tracker stamps frames with.
pub fn monotonic_us() -> std::io::Result<u64> {
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

pub struct ReplayOptions {
    /// 0 runs as fast as possible.
    pub speed: f64,
    /// No Wayland: the pipeline runs, nothing is drawn or sent.
    pub headless: bool,
    /// Start awake instead of waiting for the wake gesture.
    pub awake: bool,
}

/// Replays a trace through the pipeline on the trace's own clock. Without `headless`, the
/// skeletons appear on the overlay and the system pointer moves (Tasks C4 and C5 manual checks).
pub fn replay(path: &Path, options: &ReplayOptions) -> Result<()> {
    let speed = options.speed;
    if !speed.is_finite() || speed < 0.0 {
        bail!("replay speed must be finite and non-negative");
    }
    let file = File::open(path).with_context(|| format!("open trace {}", path.display()))?;
    let mut reader = TraceReader::new(BufReader::new(file))?;
    let mut wayland: Option<(Wayland, EventQueue<Wayland>)> = if options.headless {
        None
    } else {
        Some(
            handos_platform_wayland::connect(OverlayOutputs::All)
                .context("connect to the Wayland display (or pass --headless)")?,
        )
    };
    let mut headless = Headless;
    let mut pipeline = Pipeline::new(Config::default());
    let played = play(
        &mut reader,
        options,
        &mut pipeline,
        &mut wayland,
        &mut headless,
    );
    // Release what the replay pressed and clear the overlay whether or not the trace was good:
    // after an error, no further frame would do it.
    let cleanup = match &mut wayland {
        Some((wayland, queue)) => {
            pipeline.suspend(wayland);
            dispatch(queue, wayland, DISPATCH_SLICE).context("flush the final input events")
        }
        None => Ok(()),
    };
    let (frames, mode_changes) = played?;
    cleanup?;
    tracing::info!(frames, mode_changes, "replay complete");
    Ok(())
}

/// Feeds every frame of the trace to the pipeline on the trace's clock. Returns the frame count
/// and the number of mode changes.
fn play(
    reader: &mut TraceReader<BufReader<File>>,
    options: &ReplayOptions,
    pipeline: &mut Pipeline,
    wayland: &mut Option<(Wayland, EventQueue<Wayland>)>,
    headless: &mut Headless,
) -> Result<(u64, u64)> {
    // Headless replays never ask the compositor anything.
    let focused: &dyn Fn() -> Option<String> = if wayland.is_some() {
        &hyprland::focused_monitor
    } else {
        &|| None
    };
    let mut packet = Vec::new();
    let mut first = None;
    let mut previous = None;
    let mut frames = Frames::default();
    let mut mode_changes = 0u64;
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
        let due = if options.speed > 0.0 {
            Some(
                Duration::try_from_secs_f64((now - base) as f64 / (1_000_000.0 * options.speed))
                    .context("trace time is out of range at this replay speed")?,
            )
        } else {
            None
        };
        wait_until(due, started, wayland.as_mut())?;
        if options.awake && frames.count == 0 && pipeline.set_mode(Mode::Default, now).is_some() {
            mode_changes += 1;
        }
        let desktop: &mut dyn Desktop = match wayland {
            Some((wayland, _)) => wayland,
            None => headless,
        };
        let step = pipeline.on_frame(&frame, now, desktop, focused);
        mode_changes += u64::from(step.mode_changed.is_some());
        frames.on_frame(&frame);
    }
    Ok((frames.count, mode_changes))
}

/// Sleeps until `due` after `started`, handling Wayland events meanwhile; with no `due`, only
/// handles what is already waiting.
fn wait_until(
    due: Option<Duration>,
    started: Instant,
    mut wayland: Option<&mut (Wayland, EventQueue<Wayland>)>,
) -> Result<()> {
    loop {
        let remaining = due.and_then(|due| due.checked_sub(started.elapsed()));
        match (remaining, wayland.as_deref_mut()) {
            (Some(left), Some((wayland, queue))) => {
                dispatch(queue, wayland, left.min(DISPATCH_SLICE))?;
            }
            (Some(left), None) => {
                std::thread::sleep(left);
                return Ok(());
            }
            (None, Some((wayland, queue))) => {
                dispatch(queue, wayland, Duration::ZERO)?;
                return Ok(());
            }
            (None, None) => return Ok(()),
        }
    }
}
