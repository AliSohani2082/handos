# handOs

handOs lets you control a Linux desktop with your hands through a webcam. A Python tracker runs MediaPipe on camera frames and sends hand landmarks over a Unix socket to a Rust daemon. The daemon conditions the landmarks, recognises gestures, and drives the pointer.

> **Status: milestone M1 (pointer) is in progress.** Phases A to C are built: the tracker, wire protocol, trace record and replay, hand conditioning, gesture state machines and the Wayland backend. The daemon runs every frame through that pipeline: it **draws hand skeletons on a click-through overlay and moves the pointer** on the focused output (`--headless` runs it without a display). Pinch clicks rarely fire yet: real clicks don't reach the pinch threshold, which calibration settles in Phase D (see `.claude/PRPs/reports/handos-m1-pointer-phase-c-report.md`). Settings, the control socket, calibration and the `status`/`toggle`/`doctor`/`bench` commands come in Phase D. The full plan is in `.claude/PRPs/plans/handos-m1-pointer.plan.md`.

## How it fits together

```
 webcam ──► handos-tracker-mp (Python, MediaPipe)
                 │  HandFrame packets over a SOCK_SEQPACKET Unix socket
                 ▼  $XDG_RUNTIME_DIR/handos/tracker.sock
            handos daemon (Rust)
                 ├─ spawns the tracker and restarts it if it stalls or exits
                 ├─ validates packets (protocol version, size, shape)
                 ├─ optionally records them to a .htrace file
                 └─ hands → gestures → virtual pointer and layer-shell overlay
```

The daemon starts the tracker itself. You don't run the two side by side.

## Requirements

| Need | Version | Notes |
|---|---|---|
| Linux | any recent | Uses Unix sockets, `flock` and `XDG_RUNTIME_DIR`. The daemon needs a Wayland compositor with wlr layer shell and virtual pointer manager version 2. Hyprland is the target desktop. `--headless` needs neither. |
| Rust | stable (pinned by `rust-toolchain.toml`) | Developed with 1.99. `rustup` installs rustfmt and clippy automatically. |
| Python | 3.12 or newer | For the tracker. The stub tracker in one Rust test runs on any `python3`. |
| [uv](https://docs.astral.sh/uv/) | 0.12 | Manages the tracker's virtualenv and lockfile. |
| Webcam | V4L2 device | `/dev/video0` by default. Your user must be able to read it, which usually means being in the `video` group. |
| Network (first run only) | | The tracker downloads the MediaPipe hand model once (see below). |

`XDG_RUNTIME_DIR` must be set. A normal desktop login session sets it; a bare `ssh` or `sudo` shell may not.

## Install

```bash
git clone https://github.com/AliSohani2082/handos.git handos
cd handos

# 1. Build the Rust daemon
cargo build --release
# the binary is target/release/handos

# 2. Install the tracker so `handos-tracker-mp` is on your PATH
uv tool install ./trackers/mediapipe-py
```

`uv tool install` puts `handos-tracker-mp` in `~/.local/bin`. If that directory isn't on your PATH, run `uv tool update-shell` and open a new shell. Otherwise the daemon fails with `could not start tracker`.

If you'd rather not install the tracker globally, create its virtualenv in place and point the daemon at it (see [Run](#run)):

```bash
(cd trackers/mediapipe-py && uv sync)
# the tracker is now at trackers/mediapipe-py/.venv/bin/handos-tracker-mp
```

### The hand model

On its first start the tracker downloads `hand_landmarker.task` (float16) from Google's MediaPipe model bucket to:

```
~/.local/share/handos/models/hand_landmarker.task
```

The file is checked against a pinned SHA-256 every time it loads. If the check fails, the tracker refuses to start. Delete the file and it is downloaded again. On an offline machine, copy the file to that path by hand.

## Run

### Check the camera first

```bash
handos-tracker-mp --probe                      # default /dev/video0
handos-tracker-mp --probe --camera /dev/video2

# installed with uv sync instead (from the repository root)
trackers/mediapipe-py/.venv/bin/handos-tracker-mp --probe
trackers/mediapipe-py/.venv/bin/handos-tracker-mp --probe --camera /dev/video2
```

This prints the resolution and frame rate the camera actually gives as JSON, then exits. The tracker asks for 640x480 MJPG at 30 fps.

### Start the daemon

```bash
# tracker on PATH
cargo run --release -- daemon

# tracker in the local virtualenv
cargo run --release -- daemon --tracker trackers/mediapipe-py/.venv/bin/handos-tracker-mp
```

Press Ctrl+C (or send SIGTERM) to stop. On startup the daemon:

1. creates `$XDG_RUNTIME_DIR/handos/` with mode 0700, refusing to continue if the directory is owned by someone else or has looser permissions,
2. takes an exclusive lock (a second daemon exits with `daemon already running`),
3. listens on `tracker.sock` and spawns `<tracker> --socket <path>` in its own process group,
4. accepts packets only from the process it spawned or its descendants (checked with the peer PID), so a tracker started through a wrapper such as `uv run` works,
5. unless `--headless`, connects to the Wayland display and picks one Vulkan driver (the Intel, else AMD, GPU's).

If the tracker fails to connect within 30 s or stops sending frames for 1 s, the daemon kills its whole process group and starts a new one, waiting 1, 2, then 5 s between attempts. Whenever the tracker goes away, and on shutdown, the daemon first releases any held button and clears the overlay.

Wake handOs with an open palm held toward the camera for half a second. The pointer follows the right hand on the monitor Hyprland reports as focused at that moment. A fist held for a second puts it back to sleep.

You'll see `tracker connected` with the camera size, then a `tracker status` line with fps, inference time and dropped frames each time the tracker reports.

### Commands

| Command | What it does |
|---|---|
| `handos daemon [--tracker PATH] [--record FILE] [--headless]` | Run the daemon. `--tracker` defaults to `handos-tracker-mp` on PATH. `--record` also writes every valid packet to `FILE`. `--headless` runs the pipeline without drawing or moving the pointer. |
| `handos trace record FILE` | Same as `daemon --record FILE` with the default tracker. |
| `handos trace replay FILE [--speed X] [--headless] [--awake]` | Play a trace through the frame pipeline on its original clock. `--speed 2` plays twice as fast; `--speed 0` plays as fast as possible. Without `--headless`, the skeletons appear on the overlay and **the system pointer moves**. `--awake` starts awake instead of waiting for the wake gesture. |
| `handos --help`, `handos <cmd> --help` | Usage. |

Recording never overwrites: if `FILE` already exists, the command fails.

```bash
# Record a session (Ctrl+C to stop), then replay it instantly
cargo run --release -- trace record session.htrace
cargo run --release -- trace replay --speed 0 session.htrace
```

### Logging

Logging goes to stderr. Set the level with `HANDOS_LOG` (same syntax as `RUST_LOG`). The default is `info`.

```bash
HANDOS_LOG=debug  handos daemon   # one line per detected hand (track id, handedness)
HANDOS_LOG=trace  handos daemon   # one line per frame as well
HANDOS_LOG=handos=debug,warn handos daemon
```

The daemon logs malformed tracker packets at most once per second, so a misbehaving tracker can't flood the log.

## Tests

Everything below runs without a camera or a person in front of it.

```bash
# Rust: unit, property (proptest), golden-file and trace-replay tests
cargo test --workspace

# The recording fakes in handos-platform sit behind a feature flag
cargo test -p handos-platform --features fake

# Lints, as strict as review expects
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

# Python tracker tests
cd trackers/mediapipe-py
uv run pytest
```

What the suites cover:

| Suite | Checks |
|---|---|
| `handos-proto` tests and `tests/golden/` | Frame encoding round-trips (proptest) and a golden packet, `two_hands.bin`, that the Python encoder must also reproduce byte for byte |
| `handos-hands` `tests/replay.rs` | Hand identity, outlier rejection, 1€ filter and prediction on recorded traces |
| `handos-gesture` `tests/actions.rs`, `tests/replay.rs` | Wake/sleep clutch, pinch click vs drag, scroll and claims, plus a replay of `pluck-good.htrace` |
| `handos` `tests/daemon.rs` | Starts the real binary with a stub tracker, sends one bad packet and one valid frame, and checks that only the valid one is recorded and the socket is released |
| `handos` `tests/replay.rs` | The recorded traces replay with every frame and hold window intact; the pipeline draws only on the focused output and leaves no button held. A click test on `click-good.htrace` is ignored until Phase D sets the pinch threshold |
| `trackers/mediapipe-py/tests` | Packet encoding against the Rust golden packet, camera handedness label conversion, and track IDs that survive reordering and brief dropout |

The `daemon` integration test runs a small Python 3 script as its stub tracker, so `python3` must be on PATH for `cargo test`.

### Trace fixtures

`traces/` holds real recordings used by the replay tests:

| File | Content |
|---|---|
| `palm-good.htrace` + `.reps.json` | Palm-touch session from the M0 spike, with hold and rep time windows |
| `pluck-good.htrace` + `.reps.json` | Fingertip-pluck session from the M0 spike |
| `live-check.htrace` | A short live recording from the M1 tracker |
| `click-good.htrace` + `.reps.json` | 50 guided thumb-to-index clicks (`spikes/two-hand/record.py --task click`) |

`tools/convert_spike_trace.py` converts an old M0 JSONL recording into an `.htrace` file plus its `.reps.json` windows. It imports the tracker package, so run it through the tracker's environment:

```bash
uv run --project trackers/mediapipe-py python tools/convert_spike_trace.py old.jsonl traces/new.htrace
```

## Repository layout

```
crates/
  handos/                    the binary: CLI, daemon loop, tracker process, trace commands, per-frame pipeline
  handos-proto/              wire protocol: HandFrame encoding, control messages, .htrace files
  handos-hands/              hand conditioning: stable identity, outliers, 1€ filter, prediction, features
  handos-gesture/            pure gesture state machines: clutch, pinch phases, pointer mapping, claims
  handos-render/             display list, skeleton drawing and the wgpu backend
  handos-platform/           Overlay and Input traits plus recording fakes (feature `fake`)
  handos-platform-wayland/   Wayland backend: overlay, virtual pointer, Vulkan driver choice, Hyprland focus
trackers/mediapipe-py/       the Python MediaPipe tracker (uv project)
traces/                      recorded traces used by the tests
tools/                       one-off scripts
spikes/                      M0 throwaway experiments, outside the workspace; see spikes/RESULTS.md
```

`spikes/` is excluded from the Cargo workspace and isn't built by `cargo build` at the root. The plan deletes it at the end of M1.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `set XDG_RUNTIME_DIR` | You're in a shell without a login session. Run from your desktop session, or `export XDG_RUNTIME_DIR=/run/user/$(id -u)`. |
| `daemon already running` | Another daemon holds the lock. Stop it first. The lock is released automatically when that process exits, even if it crashed. |
| `... must be a private directory owned by this user (0700)` | `$XDG_RUNTIME_DIR/handos` has the wrong owner or mode. Remove it and let the daemon recreate it. |
| `could not start tracker` | `handos-tracker-mp` isn't on PATH. Install it with `uv tool install`, or pass `--tracker` with the full path. |
| `cannot open camera /dev/video0` | Wrong device or no permission. List devices with `v4l2-ctl --list-devices` and check you're in the `video` group. |
| `model SHA-256 mismatch` | The downloaded model is corrupt or has changed upstream. Delete `~/.local/share/handos/models/hand_landmarker.task` and restart. |
| `ignoring malformed tracker packet` with `incompatible tracker protocol N` or `unsupported HandFrame protocol version N` | The tracker and daemon come from different versions. The daemon keeps running but drops every frame, logging this at most once per second. Reinstall the tracker from this checkout. |
| Low fps in `tracker status` | MediaPipe on CPU takes about 30 ms per frame on a 2018 laptop, which limits it to about 15 fps there (see `spikes/RESULTS.md`). A `performance` CPU governor helps. |

## License

MIT OR Apache-2.0, as declared in `Cargo.toml`.
