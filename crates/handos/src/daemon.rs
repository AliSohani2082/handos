use std::{
    fs::File,
    io::{self, BufWriter},
    os::fd::{AsFd, BorrowedFd},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use calloop::{
    EventLoop, Interest, Mode, PostAction, RegistrationToken,
    generic::Generic,
    signals::{Signal, Signals},
};
use handos_proto::{
    HandFrame, Handedness, MSG_CONTROL, MSG_FRAME, PROTOCOL,
    control::{MAX_PACKET, TrackerMsg, decode_control},
    decode_frame,
    trace::TraceWriter,
};

use crate::{
    trace_cmd,
    tracker_proc::{Listener, Tracker, peer_pid, recv},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const FRAME_TIMEOUT: Duration = Duration::from_secs(1);
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

struct Daemon {
    tracker: Option<Tracker>,
    connection: Option<RegistrationToken>,
    frames: Frames,
    last_frame: Option<Instant>,
    last_invalid: Option<Instant>,
    deadline: Instant,
    attempts: usize,
    recorder: Option<TraceWriter<BufWriter<File>>>,
}

impl Daemon {
    fn failed(&mut self, command: &Path, path: &Path) -> Result<bool> {
        let now = Instant::now();
        if let Some(child) = self.tracker.as_mut() {
            if child.exited().context("check tracker process")? || now >= self.deadline {
                tracing::warn!(last_frame = ?self.last_frame, "tracker stopped delivering frames");
                self.tracker = None;
                self.schedule_restart(now);
                return Ok(true);
            }
        } else if now >= self.deadline {
            match Tracker::spawn(command, path) {
                Ok(tracker) => {
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
        Ok(false)
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
                    self.connection = None;
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
                    self.connection = None;
                    return Ok(PostAction::Remove);
                }
            }
        }
        Ok(PostAction::Continue)
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
}

pub fn run(command: &Path, record: Option<&Path>) -> Result<()> {
    let listener = Listener::bind()?;
    let path = listener.path.clone();
    let mut event_loop: EventLoop<Daemon> = EventLoop::try_new().context("create event loop")?;
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
    let tracker_handle = handle.downgrade();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
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
                let is_tracker = state
                    .tracker
                    .as_ref()
                    .is_some_and(|child| peer_pid(conn.as_fd()).is_ok_and(|pid| pid == child.id()));
                if is_tracker && state.connection.is_none() {
                    let handle = tracker_handle
                        .upgrade()
                        .ok_or_else(|| io::Error::other("event loop is shutting down"))?;
                    let mut buf = vec![0; MAX_PACKET];
                    state.connection = Some(
                        handle
                            .insert_source(
                                Generic::new(conn, Interest::READ, Mode::Level),
                                move |_, fd, state| state.read(fd.as_fd(), &mut buf),
                            )
                            .map_err(|e| io::Error::other(e.error))?,
                    );
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| e.error)?;

    let mut daemon = Daemon {
        tracker: None,
        connection: None,
        frames: Frames::default(),
        last_frame: None,
        last_invalid: None,
        deadline: Instant::now(),
        attempts: 0,
        recorder: record.map(trace_cmd::writer).transpose()?,
    };
    let mut result = Ok(());
    let stop = event_loop.get_signal();
    event_loop
        .run(
            Duration::from_millis(250),
            &mut daemon,
            |daemon| match daemon.failed(command, &path) {
                Ok(true) => {
                    if let Some(token) = daemon.connection.take() {
                        handle.remove(token);
                    }
                }
                Ok(false) => {}
                Err(err) => {
                    result = Err(err);
                    stop.stop();
                }
            },
        )
        .context("dispatch daemon events")?;
    if let Some(writer) = daemon.recorder.as_mut() {
        writer.flush().context("flush trace recording")?;
    }
    result
}
