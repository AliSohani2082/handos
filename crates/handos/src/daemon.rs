use std::{
    fs::File,
    io::{self, BufWriter},
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
    generic::Generic,
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
};
use handos::pipeline::{Config, Headless, Pipeline, Rate};
use handos_platform::Desktop;
use handos_platform_wayland::{OverlayOutputs, Wayland, WaylandSource, hyprland};
use handos_proto::{
    HandFrame, Handedness, MSG_CONTROL, MSG_FRAME, PROTOCOL,
    control::{DaemonMsg, MAX_PACKET, TrackerMode, TrackerMsg, decode_control, encode_control},
    decode_frame,
    trace::TraceWriter,
};

use crate::{
    trace_cmd,
    tracker_proc::{Listener, Tracker, descends_from, peer_pid, recv, send},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const FRAME_TIMEOUT: Duration = Duration::from_secs(1);
const HOUSEKEEPING: Duration = Duration::from_millis(250);
const BACKOFF: [u64; 3] = [1, 2, 5];

#[derive(Default)]
pub struct Frames {
    pub count: u64,
}

impl Frames {
    pub fn on_frame(&mut self, frame: &HandFrame) {
        self.count += 1;
        tracing::trace!(seq = frame.seq, hands = frame.hands.len(), "hand frame");
        if tracing::enabled!(tracing::Level::DEBUG) {
            for hand in &frame.hands {
                let handedness = match hand.handedness {
                    Handedness::Left => "left",
                    Handedness::Right => "right",
                };
                tracing::debug!(
                    seq = frame.seq,
                    track_id = hand.track_id,
                    handedness,
                    "hand"
                );
            }
        }
    }
}

pub struct Options {
    pub tracker: PathBuf,
    pub record: Option<PathBuf>,
    /// No Wayland: frames run through the pipeline, but nothing is drawn or sent.
    pub headless: bool,
}

struct Daemon {
    tracker: Option<Tracker>,
    connection: Option<RegistrationToken>,
    /// A second handle on the tracker connection, for the `set` messages the daemon sends.
    sender: Option<OwnedFd>,
    exit_watch: Option<RegistrationToken>,
    frames: Frames,
    pipeline: Pipeline,
    wayland: Option<Wayland>,
    headless: Headless,
    rate: Option<Rate>,
    last_frame: Option<Instant>,
    last_invalid: Option<Instant>,
    deadline: Instant,
    attempts: usize,
    recorder: Option<TraceWriter<BufWriter<File>>>,
}

impl Daemon {
    /// Housekeeping: restart a missing tracker after its backoff, drop one that stalled.
    fn watchdog(&mut self, handle: &LoopHandle<'static, Daemon>, command: &Path, socket: &Path) {
        let now = Instant::now();
        if let Some(tracker) = &mut self.tracker {
            // Without a pidfd, poll for the exit (try_wait never blocks).
            let exited = self.exit_watch.is_none() && tracker.exited().unwrap_or(false);
            if exited || now >= self.deadline {
                tracing::warn!(last_frame = ?self.last_frame, exited, "tracker stopped delivering frames");
                self.lose_tracker(handle, now);
            }
            return;
        }
        if now < self.deadline {
            return;
        }
        match Tracker::spawn(command, socket) {
            Ok(tracker) => {
                match tracker.exit_fd() {
                    Ok(fd) => {
                        // Weak: a strong handle inside a source keeps the loop (and the socket
                        // file its listener removes on drop) alive forever.
                        let watch = handle.downgrade();
                        self.exit_watch = handle
                            .insert_source(
                                Generic::new(fd, Interest::READ, Mode::Level),
                                move |_, _, daemon: &mut Daemon| {
                                    daemon.exit_watch = None;
                                    tracing::warn!("tracker exited");
                                    if let Some(handle) = watch.upgrade() {
                                        daemon.lose_tracker(&handle, Instant::now());
                                    }
                                    Ok(PostAction::Remove)
                                },
                            )
                            .map_err(|e| tracing::warn!(err = %e.error, "cannot watch the tracker's exit"))
                            .ok();
                    }
                    Err(err) => {
                        tracing::warn!(%err, "no pidfd: tracker exit is noticed by the frame timeout")
                    }
                }
                self.tracker = Some(tracker);
                self.deadline = now + STARTUP_TIMEOUT;
                self.last_frame = None;
            }
            Err(err) => {
                tracing::warn!(tracker = %command.display(), %err, "could not start tracker");
                self.schedule_restart(now);
            }
        }
    }

    /// The tracker is gone or wedged: release buttons, clear the overlay and plan a restart.
    fn lose_tracker(&mut self, handle: &LoopHandle<'static, Daemon>, now: Instant) {
        self.tracker = None;
        self.sender = None;
        self.rate = None;
        if let Some(token) = self.connection.take() {
            handle.remove(token);
        }
        if let Some(token) = self.exit_watch.take() {
            handle.remove(token);
        }
        let desktop = desktop_mut(&mut self.wayland, &mut self.headless);
        self.pipeline.suspend(desktop);
        self.schedule_restart(now);
    }

    fn schedule_restart(&mut self, now: Instant) {
        self.deadline = now + Duration::from_secs(BACKOFF[self.attempts]);
        self.attempts = (self.attempts + 1).min(BACKOFF.len() - 1);
    }

    fn read(&mut self, fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<PostAction> {
        // Bound the batch so the tracker can't starve other event sources.
        for _ in 0..32 {
            match recv(fd, buf) {
                Ok(0) => {
                    self.disconnected();
                    return Ok(PostAction::Remove);
                }
                Ok(size) => {
                    let valid = buf
                        .get(..size)
                        .map(|packet| self.packet(packet))
                        .transpose();
                    match valid {
                        Ok(Some(())) => {
                            if let Some(writer) = &mut self.recorder {
                                writer
                                    .write_packet(&buf[..size])
                                    .and_then(|()| writer.flush())
                                    .map_err(io::Error::other)?;
                            }
                        }
                        Ok(None) => self.invalid("oversize tracker packet"),
                        Err(err) => self.invalid(err),
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::warn!(%err, "tracker disconnected");
                    self.disconnected();
                    return Ok(PostAction::Remove);
                }
            }
        }
        Ok(PostAction::Continue)
    }

    fn disconnected(&mut self) {
        self.connection = None;
        self.sender = None;
        self.rate = None;
    }

    fn invalid(&mut self, err: impl std::fmt::Display) {
        let now = Instant::now();
        if self
            .last_invalid
            .is_none_or(|last| now - last >= Duration::from_secs(1))
        {
            tracing::warn!(%err, "ignoring malformed tracker packet");
            self.last_invalid = Some(now);
        }
    }

    fn packet(&mut self, bytes: &[u8]) -> Result<()> {
        match bytes.first().copied() {
            Some(MSG_FRAME) => {
                let frame = decode_frame(bytes)?;
                let now = Instant::now();
                self.last_frame = Some(now);
                self.deadline = now + FRAME_TIMEOUT;
                self.attempts = 0;
                // The tracker stamps frames with CLOCK_MONOTONIC; so does the pipeline's clock.
                let now_us = trace_cmd::monotonic_us()?;
                let desktop = desktop_mut(&mut self.wayland, &mut self.headless);
                let step =
                    self.pipeline
                        .on_frame(&frame, now_us, desktop, &hyprland::focused_monitor);
                if let Some(mode) = step.mode_changed {
                    tracing::info!(?mode, "mode changed");
                }
                self.set_rate(step.rate);
                self.frames.on_frame(&frame);
            }
            Some(MSG_CONTROL) => match decode_control(bytes)? {
                TrackerMsg::Hello {
                    protocol,
                    name,
                    camera,
                    ..
                } => {
                    anyhow::ensure!(
                        protocol == PROTOCOL,
                        "incompatible tracker protocol {protocol}"
                    );
                    tracing::info!(%name, width = camera.width, height = camera.height, "tracker connected");
                }
                TrackerMsg::Status {
                    fps,
                    infer_ms,
                    dropped,
                    error,
                } => {
                    if let Some(error) = error {
                        tracing::warn!(error, "tracker error");
                    } else {
                        tracing::info!(fps, infer_ms, dropped, "tracker status");
                    }
                }
            },
            _ => anyhow::bail!("unknown tracker packet type"),
        }
        Ok(())
    }

    /// Asks the tracker for a new rate when the pipeline's wish changes (adaptive rate, C7).
    fn set_rate(&mut self, rate: Rate) {
        if self.rate == Some(rate) {
            return;
        }
        let Some(fd) = &self.sender else {
            return;
        };
        let msg = match rate {
            Rate::Active { fps } => DaemonMsg::Set {
                mode: TrackerMode::Active,
                fps: Some(f32::from(fps)),
            },
            Rate::Idle => DaemonMsg::Set {
                mode: TrackerMode::Idle,
                fps: None,
            },
        };
        let sent = encode_control(&msg)
            .map_err(io::Error::other)
            .and_then(|packet| send(fd.as_fd(), &packet));
        match sent {
            Ok(true) => {
                tracing::debug!(?rate, "tracker rate");
                self.rate = Some(rate);
            }
            Ok(false) => {} // socket full: try again on the next frame
            Err(err) => tracing::warn!(%err, "could not set the tracker rate"),
        }
    }
}

/// The desktop the pipeline drives: Wayland, or nothing in `--headless` runs.
fn desktop_mut<'a>(
    wayland: &'a mut Option<Wayland>,
    headless: &'a mut Headless,
) -> &'a mut dyn Desktop {
    match wayland {
        Some(wayland) => wayland,
        None => headless,
    }
}

pub fn run(options: &Options) -> Result<()> {
    // Bind first: a second daemon fails here, before it touches the display.
    let listener = Listener::bind()?;
    let socket = listener.path.clone();
    let mut event_loop: EventLoop<'static, Daemon> =
        EventLoop::try_new().context("create event loop")?;
    let handle = event_loop.handle();
    let signal = event_loop.get_signal();
    handle
        .insert_source(
            Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?,
            move |_, _, _| {
                signal.stop();
            },
        )
        .map_err(|e| e.error)?;

    let wayland = if options.headless {
        None
    } else {
        let (wayland, queue) = handos_platform_wayland::connect(OverlayOutputs::All)
            .context("connect to the Wayland display (or pass --headless)")?;
        let source = WaylandSource::new(wayland.connection().clone(), queue);
        handle
            .insert_source(source, |_, queue, daemon: &mut Daemon| {
                match &mut daemon.wayland {
                    Some(wayland) => queue.dispatch_pending(wayland),
                    None => Ok(0),
                }
            })
            .map_err(|e| anyhow!("watch the Wayland connection: {}", e.error))?;
        Some(wayland)
    };

    let accept_handle = handle.downgrade();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, daemon: &mut Daemon| {
                let conn = match listener.accept() {
                    Ok(conn) => conn,
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        return Ok(PostAction::Continue);
                    }
                    Err(err) => return Err(err),
                };
                let is_tracker = daemon.tracker.as_ref().is_some_and(|child| {
                    peer_pid(conn.as_fd()).is_ok_and(|pid| descends_from(pid, child.id()))
                });
                if is_tracker && daemon.connection.is_none() {
                    let handle = accept_handle
                        .upgrade()
                        .ok_or_else(|| io::Error::other("event loop is shutting down"))?;
                    daemon.sender = conn.try_clone().ok();
                    daemon.rate = None;
                    let mut buf = vec![0; MAX_PACKET];
                    daemon.connection = Some(
                        handle
                            .insert_source(
                                Generic::new(conn, Interest::READ, Mode::Level),
                                move |_, fd, daemon: &mut Daemon| daemon.read(fd.as_fd(), &mut buf),
                            )
                            .map_err(|e| io::Error::other(e.error))?,
                    );
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| e.error)?;

    let watchdog_handle = handle.downgrade();
    let command = options.tracker.clone();
    handle
        .insert_source(
            Timer::immediate(),
            move |_, _, daemon: &mut Daemon| match watchdog_handle.upgrade() {
                Some(handle) => {
                    daemon.watchdog(&handle, &command, &socket);
                    TimeoutAction::ToDuration(HOUSEKEEPING)
                }
                None => TimeoutAction::Drop,
            },
        )
        .map_err(|e| anyhow!("start the housekeeping timer: {}", e.error))?;

    let mut daemon = Daemon {
        tracker: None,
        connection: None,
        sender: None,
        exit_watch: None,
        frames: Frames::default(),
        pipeline: Pipeline::new(Config::default()),
        wayland,
        headless: Headless,
        rate: None,
        last_frame: None,
        last_invalid: None,
        deadline: Instant::now(),
        attempts: 0,
        recorder: options
            .record
            .as_deref()
            .map(trace_cmd::writer)
            .transpose()?,
    };
    let result = event_loop
        .run(None, &mut daemon, |_| {})
        .context("dispatch daemon events");

    // Every exit path releases held buttons and clears the overlay before input goes away.
    let desktop = desktop_mut(&mut daemon.wayland, &mut daemon.headless);
    daemon.pipeline.suspend(desktop);
    if let Some(wayland) = &daemon.wayland
        && let Err(err) = wayland.flush()
    {
        tracing::warn!(%err, "could not flush the final input events");
    }
    if let Some(writer) = daemon.recorder.as_mut() {
        writer.flush().context("flush trace recording")?;
    }
    result
}
