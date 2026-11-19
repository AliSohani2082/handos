# Plan: M1 Pointer (v0.1) — hands-free pointer, no scripting

## Summary
Build the first usable handOs: a Rust daemon that receives HandFrames from a Python MediaPipe tracker, conditions them (identity, outlier rejection, 1€ filter, prediction), evaluates a fixed set of gestures coded in Rust, draws both hand skeletons on a click-through GPU overlay, and drives the system pointer (move, click, drag, right click, scroll) with a gesture or hotkey clutch for wake and sleep. It ships with `handos.yaml` settings, a JSON-RPC control socket, the `status`, `toggle`, `pause`, `doctor`, `bench` and `trace` commands, and trace record/replay so that every gesture bug is reproducible. Scenario S1 without scripting.

## User Story
As a Hyprland user at a laptop with a webcam, I want to wake handOs with an open palm, move the pointer with my hand, pinch to click, drag and scroll, and put it to sleep with a fist, so that I can control the desktop hands-free without anything firing while it sleeps.

## Problem → Solution
M0 left four throwaway spikes (`spikes/`) that proved each risky part separately, but nothing that runs end to end → one Cargo workspace with seven crates and one Python tracker package that together implement S1, measured against the PRD budgets, with the spikes deleted at the end.

## Metadata
- **Complexity**: XL (about 55 files). Delivered as four PRs, one per phase below; each phase ends green and usable on its own.
- **Source PRD**: `.claude/prds/handos.prd.md` (index) — source of truth is the design doc https://claude.ai/artifact/NLXWfvV2Ra2f5kvzXcawBm (Claude Docs id `acc9d8cf-4a4f-4415-94a3-945637208ce0`)
- **PRD Phase**: M1 Pointer (v0.1), weeks 5–10, ~90 h
- **Exit criteria (PRD)**: latency and jitter budgets met or revised; pinch click 95 of 100; no false activation while asleep in a 2-hour test
- **Estimated Files**: ~55 created, 3 updated, `spikes/` deleted

---

## UX Design

### Before
```
┌──────────────────────────────────────────────────────────┐
│ No handOs. Spikes run one at a time from a terminal:     │
│  uv run live.py            → OpenCV window, no pointer   │
│  overlay-gpu --synthetic   → fake skeleton, 20 s         │
│  input-spike clicks        → test grid, then exits       │
└──────────────────────────────────────────────────────────┘
```

### After
```
┌──────────────────────────────────────────────────────────┐
│ systemctl --user start handos   (or: handos daemon)      │
│                                                          │
│  asleep ── open palm 0.5 s / hotkey ──► awake            │
│     ▲                                    │ skeleton over │
│     │                                    │ the desktop,  │
│     └── fist 1 s / hotkey / 30 s no hands┘ green dot =   │
│                                            awake         │
│  awake, right hand:                                      │
│   move hand ............ pointer follows index knuckle   │
│   quick index pinch .... left click                      │
│   index pinch + move ... drag                            │
│   quick middle pinch ... right click                     │
│   middle pinch + move .. scroll                          │
│                                                          │
│  handos status | toggle | pause | resume | doctor        │
│  handos trace record f.htrace | trace replay f.htrace    │
│  handos bench                                            │
└──────────────────────────────────────────────────────────┘
```

### Interaction Changes
| Touchpoint | Before | After | Notes |
|---|---|---|---|
| Wake | none | open palm toward camera held 0.5 s, either hand; or `handos toggle` | Skeleton appears, small green dot (mode badge) top-right of the target output |
| Pointer | none | right hand's index knuckle (landmark 5), mapped through the box | Knuckle stays still while the fingertip pinches |
| Left click | none | thumb–index pinch released within 0.3 s and moved < 0.3 hu | Click fires on release (commit on `ended`) |
| Drag | none | thumb–index pinch held > 0.3 s or moved ≥ 0.3 hu | Button pressed at the pinch-begin position, released on `ended` or `cancelled` |
| Right click | none | thumb–middle pinch tap | |
| Scroll | none | thumb–middle pinch held > 0.3 s, moved up/down | Vertical only in M1 |
| Calibrate | none | `handos calibrate`: hold the hand at the comfortable left, right, top and bottom, then open and close a pinch | Terminal prompts; writes `~/.local/state/handos/calibration.yaml`, applied at once |
| Sleep | none | fist held 1 s; `handos toggle`; 30 s without hands | Skeleton hidden, tracker drops to 5 fps (or releases the camera with `clutch.wake: hotkey`) |
| Pause | none | `handos pause` / `resume` | Camera light goes off |

---

## Mandatory Reading

| Priority | File / source | Lines | Why |
|---|---|---|---|
| P0 | Design doc → *Specs → Tracking & gestures* | all | HandFrame wire format, control messages, signal conditioning, features, triggers, clutch, mapping. The normative spec for Phases A–B |
| P0 | Design doc → *Specs → CLI, daemon & config* | all | Commands, exit codes, file paths, `handos.yaml`, control socket methods |
| P0 | Design doc → *Specs → Platform backends* | all | Overlay/Input traits, protocols, camera rules |
| P0 | Design doc → *Architecture* | all | Threads, two clocks, crate list, failure handling table, repository layout |
| P0 | `spikes/overlay-gpu/src/main.rs` | 110–335, 582–640 | wgpu on a layer surface: adapter, surface config, premultiplied alpha, Mailbox, click-through region |
| P0 | `spikes/overlay-gpu/src/skeleton.wgsl` | all (58) | The instanced-quad shader to copy into `handos-render` |
| P0 | `spikes/input/src/main.rs` | 175–296 | Virtual pointer bound to an output, absolute motion, button + frame |
| P0 | `spikes/two-hand/stabilizer.py` | 73–181 | One Euro filter, track association, sticky labels — the reference to port |
| P1 | `spikes/two-hand/record.py` | 28–56, 105–135 | Model URL, camera setup (MJPG, 640x480, buffer 1), VIDEO running mode |
| P1 | `spikes/two-hand/handtrace.py` | 1–60 | Spike JSONL trace format (to convert into `.htrace` test fixtures) |
| P1 | `spikes/RESULTS.md` | spike 1, spike 2 sections | Measured numbers, the driver-selection finding, the facing sign |
| P2 | `spikes/luau/tests/attacks.rs` | 1–30 | Integration test that runs the built binary via `CARGO_BIN_EXE_*` |
| P2 | `spikes/two-hand/tests/test_stabilizer.py` | 1–40 | pytest style: tiny synthetic observations, plain asserts |

## External Documentation

| Topic | Source | Key Takeaway |
|---|---|---|
| serde-saphyr 2.1 | Context7 `/bourumir-wyngs/serde-saphyr` | `from_str_with_options(text, options!{ strict_booleans: true, reject_unsupported_tags: true, duplicate_keys: DuplicateKeyPolicy::Error, budget: budget!{ max_anchors: 0 } })`; errors implement Display with a source snippet. Combine with `#[serde(deny_unknown_fields)]` on every settings struct |
| calloop 0.14 | Context7 `/smithay/calloop` | `EventLoop::try_new()`, `handle.insert_source(Generic::new(fd, Interest::READ, Mode::Level), cb)`, `Timer::from_duration`, `event_loop.run(timeout, &mut state, idle_cb)`. One thread, no async runtime |
| SCTK 0.21 `calloop` feature | `cargo info smithay-client-toolkit@0.21` | Default features include `calloop` + `calloop-wayland-source`; use `WaylandSource::new(conn, queue).insert(handle)` instead of the spike's hand-written `poll` loop |
| MediaPipe Hand Landmarker | https://developers.google.com/edge/mediapipe/solutions/vision/hand_landmarker | VIDEO running mode needs strictly increasing timestamps; the live A8 check confirmed anatomical labels on raw camera images |
| wlr virtual pointer v2 | https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1 | `create_virtual_pointer_with_output` maps absolute motion to one output; every event batch ends with `frame()`; `axis_source(finger)` + `axis` for scroll |
| 1€ filter | https://gery.casiez.net/1euro/ | Tune with beta = 0, lower min_cutoff until still is still, then raise beta until fast moves stop lagging |

---

## Patterns to Mirror

### LAYER_SURFACE (click-through overlay)
// SOURCE: spikes/overlay-gpu/src/main.rs:619-636
```rust
let layer = layer_shell.create_layer_surface(&qh, surface.clone(), Layer::Overlay, Some("handos-spike"), Some(&output));
layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
layer.set_exclusive_zone(-1);
layer.set_keyboard_interactivity(KeyboardInteractivity::None);
layer.set_size(0, 0);
let region = Region::new(&compositor).expect("region"); // empty: every click passes through
layer.set_input_region(Some(region.wl_region()));
layer.commit();
```

### WGPU_ON_WAYLAND
// SOURCE: spikes/overlay-gpu/src/main.rs:130-200
```rust
let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
desc.backends = backend;                       // M1: wgpu::Backends::VULKAN
let instance = wgpu::Instance::new(desc);
// SAFETY: the connection and the wl_surface outlive `Gpu`.
let wsurface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
    raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display))),
    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
}) }?;
// format: first non-sRGB; alpha: PreMultiplied, else PostMultiplied; present: Mailbox, else Fifo;
// desired_maximum_frame_latency: 1
```
Request a frame callback **before** `present()` commits (`surface.frame(qh, ..)`, main.rs:369).

### VIRTUAL_POINTER
// SOURCE: spikes/input/src/main.rs:179-181, 236-247, 270-293
```rust
let vp_mgr: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, NoEvents)?;
let vp = vp_mgr.create_virtual_pointer_with_output(Some(&seat), Some(&output), &qh, NoEvents);
vp.motion_absolute(now_ms, x, y, out_w, out_h);  // logical px of that output
vp.frame();
vp.button(now_ms, BTN_LEFT /* 0x110 */, wl_pointer::ButtonState::Pressed);
vp.frame();
```

### ONE_EURO (reference to port to Rust)
// SOURCE: spikes/two-hand/stabilizer.py:73-104
```python
def _alpha(cutoff, dt):
    r = 2 * math.pi * cutoff * dt
    return r / (r + 1)
def __call__(self, x, t):
    if self.x is None: self.x, self.dx, self.t = x.copy(), zeros_like(x), t; return x.copy()
    dt = max(t - self.t, 1e-3); self.t = t
    dx = (x - self.x) / dt
    self.dx += self._alpha(self.d_cutoff, dt) * (dx - self.dx)
    a = self._alpha(self.min_cutoff + self.beta * abs(self.dx), dt)
    self.x = self.x + a * (x - self.x)
```
Defaults measured on the spike data: image landmarks (min_cutoff 1.5, beta 8.0), world landmarks (1.5, 15.0), d_cutoff 1.0.

### TRACK_ASSOCIATION (port into the Python tracker)
// SOURCE: spikes/two-hand/stabilizer.py:111-137
Greedy nearest-first match of palm centres (landmarks 0, 5, 9, 13, 17), max jump 0.25 (image units); unmatched observations get new ids; a track missing for N frames is dropped.

### CAMERA_SETUP
// SOURCE: spikes/two-hand/record.py:45-56
```python
cap = cv2.VideoCapture(device, cv2.CAP_V4L2)
cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640); cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
cap.set(cv2.CAP_PROP_FPS, 30); cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
```

### RUST_INTEGRATION_TEST (run the built binary)
// SOURCE: spikes/luau/tests/attacks.rs:1-16
```rust
let out = Command::new(env!("CARGO_BIN_EXE_luau-spike")).args(args).output().expect("spawn spike");
```

### PYTEST_STYLE
// SOURCE: spikes/two-hand/tests/test_stabilizer.py:1-20 — small synthetic inputs built by a helper, plain `assert`, one behaviour per test, no fixtures framework beyond pytest.

### ERROR_HANDLING (new convention — the spikes `expect()` everywhere and must not be copied)
- Library crates: one `thiserror` enum per crate (`ProtoError`, `ConfigError`, `PlatformError`, …), returned as `Result<T, XError>`.
- Binary (`handos`): `anyhow::Result` with `.context("…")` at I/O boundaries.
- No `unwrap`/`expect`/`panic!` in non-test code except for invariants documented with a comment. A daemon panic must never leave a button pressed (see Task D2).

### LOGGING (new convention — the spikes use `eprintln!`)
- `tracing` macros (`info!`, `warn!`, `debug!`, `trace!`) with structured fields: `info!(fps = 29.7, "tracker status")`.
- `tracing-subscriber` `fmt` to stderr (journald captures it), filter from `HANDOS_LOG` (default `info`).
- Per-frame paths log at `trace!` only.

---

## Files to Change

| File | Action | Justification |
|---|---|---|
| `Cargo.toml` | CREATE | Root workspace: `members = ["crates/*"]`, `exclude = ["spikes"]`, shared `[workspace.dependencies]`, edition 2024, `[workspace.lints]` |
| `rust-toolchain.toml` | CREATE | `channel = "stable"`, components rustfmt, clippy |
| `crates/handos-proto/{Cargo.toml,src/lib.rs,src/frame.rs,src/control.rs,src/trace.rs}` | CREATE | HandFrame codec, tracker control messages, `.htrace` read/write |
| `crates/handos-proto/tests/golden.rs`, `tests/golden/two_hands.bin` | CREATE | Byte-exact golden packet shared with the Python tests |
| `crates/handos-hands/{Cargo.toml,src/lib.rs,src/identity.rs,src/outlier.rs,src/one_euro.rs,src/predict.rs,src/quality.rs,src/features.rs}` | CREATE | Signal conditioning and derived features (pure, no I/O) |
| `crates/handos-gesture/{Cargo.toml,src/lib.rs,src/trigger.rs,src/phase.rs,src/claims.rs,src/clutch.rs,src/pointer.rs,src/mapping.rs}` | CREATE | M1 gesture set coded in Rust: triggers, hysteresis, phases, claims, sleep/default, pointer actions, box mapping (pure, no I/O) |
| `crates/handos-render/{Cargo.toml,src/lib.rs,src/display_list.rs,src/gpu.rs,src/skeleton.wgsl}` | CREATE | Display list + wgpu backend (instanced quads) |
| `crates/handos-platform/{Cargo.toml,src/lib.rs}` | CREATE | `Overlay` and `Input` traits, `Features` set |
| `crates/handos-platform-wayland/{Cargo.toml,src/lib.rs,src/overlay.rs,src/input.rs,src/outputs.rs,src/hyprland.rs,src/vulkan.rs}` | CREATE | Layer-shell overlay per output, virtual pointer per output, Hyprland focused-monitor query, Vulkan driver selection |
| `crates/handos/{Cargo.toml,src/main.rs,src/cli.rs,src/daemon.rs,src/tracker_proc.rs,src/control.rs,src/settings.rs,src/doctor.rs,src/bench.rs,src/trace_cmd.rs,src/pressed.rs,src/calibrate.rs}` | CREATE | Daemon, CLI client, settings, supervision, doctor, bench, calibration |
| `crates/handos/tests/{cli.rs,replay.rs}` | CREATE | Binary-level tests: exit codes, settings errors, headless trace replay |
| `trackers/mediapipe-py/{pyproject.toml,src/handos_tracker_mp/__init__.py,__main__.py,camera.py,model.py,protocol.py,tracks.py,tracker.py}` | CREATE | Python tracker: capture newest frame, MediaPipe, track ids, HandFrame packets, control messages |
| `trackers/mediapipe-py/tests/{test_protocol.py,test_tracks.py,test_handedness.py}` | CREATE | Codec vs Rust golden bytes, association, anatomical handedness |
| `tools/convert_spike_trace.py` | CREATE | Spike JSONL (mirrored) → `.htrace` (unmirrored) for test fixtures |
| `traces/palm-good.htrace`, `traces/pluck-good.htrace`, `traces/*.reps.json` | CREATE | Converted spike sessions + rep windows, used by replay tests |
| `packaging/systemd/handos.service` | CREATE | systemd user unit |
| `tools/click-test.html` | CREATE | 100 sequential targets with a hit counter, for the 95/100 exit test |
| `docs/m1-results.md` | CREATE | Exit-test numbers (Task D8) |
| `README.md` | UPDATE | Install, run, Hyprland hotkey binding, `doctor` |
| `.gitignore` | UPDATE | `trackers/mediapipe-py/.venv/`, `bench/` outputs |
| `.claude/prds/handos.prd.md` | UPDATE | M1 status at the end |
| `spikes/` | DELETE | Last task, after their code has been ported; `spikes/RESULTS.md` moves to `docs/m0-spike-results.md` |

## NOT Building

- Luau, scripting API, `check`/`preview`/`apply`/`eval`/`rollback`, generations, `init` (M2). M1 gestures are Rust code that M2 deletes in favour of `std.pointer`.
- `handos inspect` and the on-canvas HUD (M2). M1 debugging uses `HANDOS_LOG=debug` and trace replay.
- Canvas items, anchors, springs, text rendering of any kind (M3). The mode badge is a coloured dot, not text.
- Keyboard input, text entry, clipboard, screen capture, the Windows trait beyond one Hyprland focused-monitor query (M3/M5).
- An on-overlay calibration UI (M1's `handos calibrate` is terminal-guided, Task D7); pointing by finger aim (a ray to the screen — would need its own spike), relative pointing mode, `pose` labels other than `open` and `fist`, `motion` (shake/swipe), `near`, `seq`, `double_tap`.
- Providers, packages, grants, `secrets.yaml`, `handos logs` (journald already has the log).
- The native Rust tracker and its memory/CPU budgets (M5). The Python tracker is exempt from them (PRD).
- Horizontal scroll, middle click, multi-pointer, X11/KDE/GNOME backends.
- Display-rate animation: M1 draws only when a HandFrame arrives (ADR 5) and predicts one step to the expected presentation time.

---

## Step-by-Step Tasks

Phases are PR boundaries. Every task ends with `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green (and the Python checks for tracker tasks).

### Phase A — Wire protocol, tracker, traces (PR 1)

#### Task A1: Root workspace
- **ACTION**: Create `Cargo.toml`, `rust-toolchain.toml`, the seven crate skeletons with `lib.rs`/`main.rs` stubs.
- **IMPLEMENT**: `[workspace] resolver = "3"; members = ["crates/*"]; exclude = ["spikes"]`. `[workspace.package] edition = "2024", license = "MIT OR Apache-2.0", publish = false`. `[workspace.dependencies]`: `smithay-client-toolkit = "0.21"` (default features: keeps `calloop`), `calloop = "0.14"`, `wayland-client = "0.31"`, `wayland-backend = { version = "0.3", features = ["client_system"] }`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }`, `raw-window-handle = "0.6"`, `wgpu = { version = "30", default-features = false, features = ["vulkan", "wgsl", "std"] }`, `pollster = "0.4"`, `bytemuck = { version = "1", features = ["derive"] }`, `glam = "0.34"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `serde-saphyr = "2.1"`, `clap = { version = "4.6", features = ["derive"] }`, `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`, `thiserror = "2"`, `anyhow = "1"`, `libc = "0.2"`; dev: `proptest = "1.12"`. `[workspace.lints.rust] unsafe_code = "deny"` (crates that need `unsafe` — render, platform-wayland, the daemon's socket code — set `#![allow(unsafe_code)]` and give every block a `// SAFETY:` comment). `[workspace.lints.clippy] unwrap_used = "deny", expect_used = "warn"` (allowed in tests via `#[cfg_attr(test, allow(...))]`).
- **MIRROR**: `spikes/Cargo.toml` (resolver 3, edition 2024, workspace deps).
- **GOTCHA**: `spikes/` has its own `[workspace]`; without `exclude = ["spikes"]` cargo errors about nested workspaces. wgpu drops the `gles` feature here: GL found no adapter in spike 2 and only Vulkan is supported.
- **VALIDATE**: `cargo build --workspace` and `cargo build --manifest-path spikes/Cargo.toml` both succeed.

#### Task A2: `handos-proto` — HandFrame codec
- **ACTION**: Implement the wire format exactly as the Tracking spec defines it.
- **IMPLEMENT**: `pub const PROTOCOL: u8 = 1; MSG_FRAME: u8 = 0x01; MSG_CONTROL: u8 = 0x02;` `struct HandFrame { flags: u8, seq: u32, t_capture_us: u64, t_emit_us: u64, hands: Vec<Hand> /* ≤ 4 */ }`, `struct Hand { track_id: u32, handedness: Handedness /*0 left, 1 right*/, handedness_score: f32, presence: f32, landmarks: [[f32; 3]; 21], world: Option<[[f32; 3]; 21]> }`. Layout, little-endian: header 24 bytes = `protocol u8, flags u8, hand_count u8, pad u8, seq u32, t_capture_us u64, t_emit_us u64`; per hand 520 bytes with world (268 without) = `track_id u32, handedness u8, pad[3], handedness_score f32, presence f32, landmarks 21×3 f32, world 21×3 f32 if flags bit 0`. A packet on the socket is `MSG_FRAME` + that body (1 + 24 + 2×520 = 1,065 bytes for two hands; the spec's "1,064" excludes the type byte). `encode_frame(&HandFrame, &mut Vec<u8>)`, `decode_frame(&[u8]) -> Result<HandFrame, ProtoError>` rejecting: wrong protocol, `hand_count > 4`, length mismatch, non-finite floats.
- **MIRROR**: ERROR_HANDLING (thiserror `ProtoError`).
- **GOTCHA**: Do not use `#[repr(C)]` + transmute; decode field by field (`f32::from_le_bytes`) so that padding and endianness are explicit. Landmarks are **image space, un-mirrored**; the daemon mirrors (x' = 1 − x) for display and mapping only.
- **VALIDATE**: unit tests: round trip with 0, 1, 2, 4 hands, with and without world; each rejection case; `proptest` round trip for arbitrary finite values.

#### Task A3: `handos-proto` — control messages and `.htrace`
- **ACTION**: Tracker control JSON and the trace file format.
- **IMPLEMENT**: `enum TrackerMsg { Hello { protocol, name, max_hands, world, camera: CameraInfo }, Status { fps, infer_ms, dropped, error: Option<String> } }` and `enum DaemonMsg { Set { mode: TrackerMode /*active|idle|off*/, fps: Option<f32> }, Shutdown }` via `#[serde(tag = "type", rename_all = "snake_case")]`, sent as `MSG_CONTROL` + UTF-8 JSON. `fps` is the adaptive-rate field added after M0 (design doc, Tracking spec "Adaptive rate"). `.htrace`: 16-byte header `b"HTRC"`, `version u16 = 1`, `reserved u16`, `t0_us u64`; then records `u32 len` + packet bytes (`MSG_FRAME`… or `MSG_CONTROL`…, exactly what crossed the socket). `TraceWriter<W: Write>`, `TraceReader<R: Read>` yielding packets; replay timing comes from each frame's `t_capture_us`.
- **VALIDATE**: unit tests: JSON shapes match the spec examples (`{"type":"set","mode":"active"}` when `fps` is `None` — use `skip_serializing_if`); trace write→read round trip; truncated file → `ProtoError::Truncated`.

#### Task A4: Golden packet shared with Python
- **ACTION**: `crates/handos-proto/tests/golden.rs` writes/compares `tests/golden/two_hands.bin`.
- **IMPLEMENT**: A fixed two-hand frame (seq 7, deterministic landmark values `i as f32 / 100.0`). Test compares `encode_frame` output to the committed file; `UPDATE_GOLDEN=1 cargo test -p handos-proto` regenerates it.
- **VALIDATE**: `cargo test -p handos-proto golden`.

#### Task A5: Python tracker package
- **ACTION**: `trackers/mediapipe-py` (uv project, `package = true`, console script `handos-tracker-mp`).
- **IMPLEMENT**:
  - `pyproject.toml`: `requires-python = ">=3.12"`, deps `mediapipe>=1.2.3`, `opencv-python>=5.0`, `numpy>=2.0`; dev `pytest>=8`.
  - `model.py`: model at `~/.local/share/handos/models/hand_landmarker.task`, URL `https://storage.googleapis.com/mediapipe-models/hand_landmarker/hand_landmarker/float16/latest/hand_landmarker.task`, verify SHA-256 `fbc2a30080c3c557093b5ddfc334698132eb341044ccee322ccf8bcf3607cde1` after download (download to `.part`, verify, rename; mismatch → delete and report it in `status.error`). `latest` can change upstream; the error tells the user to check and update the pinned hash.
  - `camera.py`: CAMERA_SETUP; a capture **thread** that keeps only the newest frame (single slot + `threading.Event`), records `t_capture_us = time.monotonic_ns() // 1000` right after `cap.read()`; `close()` releases the device (camera light off).
  - `tracks.py`: TRACK_ASSOCIATION port → stable `track_id` per hand.
  - `protocol.py`: `encode_frame(...) -> bytes` with `struct` (`"<BBBxIQQ"` header, `"<IB3xff"` per hand + 126 floats), `encode_control(dict)`, `decode_control(bytes)`.
  - `tracker.py`: connect `socket.socket(AF_UNIX, SOCK_SEQPACKET)` to `--socket` (default `$XDG_RUNTIME_DIR/handos/tracker.sock`); send `hello`; loop: wait for a new frame, **no mirroring**, `detect_for_video(image, ts_ms)` with strictly increasing ts, build hands, map handedness (GOTCHA), send the newest frame, never queue; `status` once per second; non-blocking `recv` for `set`/`shutdown`; `idle` = same pipeline paced to the idle rate; `off` = close the camera, keep the socket; `fps` paces active mode. `--probe`: open the camera for 2 s, print one JSON line `{fps, brightness}` and exit (used by `doctor`).
- **MIRROR**: CAMERA_SETUP, TRACK_ASSOCIATION, PYTEST_STYLE; ruff line length 120.
- **GOTCHA**: **Handedness.** Keep MediaPipe 1.2.3 labels on raw camera images: `"Left"` → left (0), `"Right"` → right (1). The live check showed that swapping them reverses both hands. Swap labels only when converting recordings from mirrored inputs. **VIDEO mode** raises on non-increasing timestamps: use `max(prev + 1, now_ms)`.
- **VALIDATE**: `cd trackers/mediapipe-py && uv run pytest -q && uvx ruff check --line-length 120 .`; `test_protocol.py` asserts `encode_frame(...)` of the golden frame equals `crates/handos-proto/tests/golden/two_hands.bin`.

#### Task A6: Spike traces → test fixtures
- **ACTION**: `tools/convert_spike_trace.py` converts `spikes/two-hand/data/{palm,pluck}-good-*.jsonl` into `traces/{palm,pluck}-good.htrace` plus `traces/{palm,pluck}-good.reps.json` (rep and hold windows in µs).
- **IMPLEMENT**: Un-mirror: `lm x' = 1 − x`, `world x' = −x`; swap the original MediaPipe labels because the spike used mirrored input. Track ids by the same association as A5 (import it from the tracker package). `t_capture_us = t_ms × 1000`, presence 1.0, flags bit 0 set.
- **GOTCHA**: The facing sign flips with the un-mirroring: on converted data, palms toward the camera give normal z = **+1 for left, −1 for right**. Task B5 asserts this.
- **VALIDATE**: `uv run --project trackers/mediapipe-py tools/convert_spike_trace.py …`; a Rust test reads both traces and finds ≥ 2,000 frames each, with two hands in ≥ 95% of hold-window frames.

#### Task A7: Daemon skeleton + trace record/replay
- **ACTION**: `handos daemon` binds `tracker.sock`, spawns the tracker, logs frames; `handos trace record|replay`.
- **IMPLEMENT**: clap subcommands (`daemon`, `status`, `toggle`, `sleep`, `wake`, `pause`, `resume`, `doctor`, `bench`, `trace record <file>`, `trace replay <file> [--speed X] [--headless]`). Daemon main thread = calloop `EventLoop<Daemon>`; `tracker.sock` is a `SOCK_SEQPACKET` listener (`libc::socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0)`, socket dir `$XDG_RUNTIME_DIR/handos/` mode 0700) inserted as a `Generic` source; frames decoded and handed to `Daemon::on_frame`. `tracker_proc.rs` spawns `settings.tracker.command` with `--socket <path>`, restarts with backoff 1 s, 2 s, 5 s on exit or 1 s without a frame while awake (Architecture failure table). `trace record` = the daemon tees received packets to a `TraceWriter` (control method `trace.record`, Task D3; until D3 lands, a `--record <file>` daemon flag). `trace replay` = feed a trace into `on_frame` on the trace's own clock instead of spawning a tracker; `--headless` skips Wayland (used by tests and bench); `--speed 0` = as fast as possible.
- **MIRROR**: LOGGING, ERROR_HANDLING. SCTK `WaylandSource` joins in Phase C.
- **GOTCHA**: Remove a stale socket file before `bind`, but only after `connect` to it fails (another daemon may be running → exit 1 with "daemon already running").
- **VALIDATE**: `crates/handos/tests/replay.rs`: `handos trace replay traces/pluck-good.htrace --headless --speed 0` exits 0 and logs the frame count; manual: `handos daemon` with the real tracker logs ~30 fps.

#### Task A8: On-camera handedness check
- **ACTION**: Manual check, recorded in the PR description.
- **IMPLEMENT**: `HANDOS_LOG=debug handos daemon`, raise only the **right** hand, palm to camera; the log must show `handedness=right`. Repeat with the left. Then record 10 s with both hands (`handos daemon --record traces/live-check.htrace`) and keep it as a fixture.
- **VALIDATE**: Both labels correct; if not, flip the mapping in `tracker.py` and fix `test_handedness.py`; correct the Tracking spec's rule in the design doc either way.

### Phase B — Hands and gestures, pure and trace-tested (PR 2)

See the [Phase B report](../reports/handos-m1-pointer-phase-b-report.md) for the replay results. The M0 pluck exercise used image distances and a deliberately open receiving palm. A new recording of thumb-to-index clicks is needed to validate M1's 3D pinch threshold.

#### Task B1: One Euro + outlier rejection
- **ACTION**: `handos-hands::one_euro`, `::outlier`.
- **IMPLEMENT**: `OneEuro<const N: usize>` over `[f32; N]` (63 floats per landmark set), ONE_EURO port, params from settings (`tracker.filter` for image space, `tracker.filter_world` for world). Outlier: discard a hand's frame if any landmark moved > 1 hu (screen hu) since the previous accepted frame, at most two consecutive discards.
- **MIRROR**: ONE_EURO.
- **VALIDATE**: unit tests mirroring the spike's One Euro test: jitter reduction on noisy constant input ≥ 50%; step input converges within 10 frames; outlier dropped twice, accepted on the third consecutive frame.

#### Task B2: Identity (roles left/right)
- **ACTION**: `handos-hands::identity`.
- **IMPLEMENT**: Per tracker `track_id`: role assigned once `handedness_score ≥ 0.8` for 3 consecutive frames; kept until the track is gone 500 ms; frozen while the two hands' image bounding boxes overlap; two tracks never share a role (the later one takes the other role).
- **MIRROR**: `StickyLabel` in `stabilizer.py:140-155` (same intent, the spec's numbers).
- **VALIDATE**: unit tests: flicker held, overlap freeze, 500 ms grace, no shared role; replay `traces/*-good.htrace` → exactly two roles, 0 role swaps.

#### Task B3: Quality, coasting, prediction
- **ACTION**: `handos-hands::quality`, `::predict`.
- **IMPLEMENT**: States `Tracked` (presence ≥ 0.5) → `Coasting` (no usable frame, ≤ 150 ms; last pose held) → `Lost`. Velocity from the filter's derivative; `predict(t) = p + v × clamp(t − t_capture, 0, 33 ms)`, disabled when quality < 0.5.
- **VALIDATE**: unit tests for the transitions and the clamp.

#### Task B4: `HandState` output type
- **ACTION**: `handos-hands::HandState` — what the rest of the daemon consumes.
- **IMPLEMENT**: `struct HandState { role: Role, track_id: u32, quality: Quality, image: [[f32;3];21] /*filtered, mirror space*/, world: [[f32;3];21], velocity: [[f32;2];21], t_capture_us: u64 }` and `struct Hands { left: Option<HandState>, right: Option<HandState> }`; `Conditioner::update(&HandFrame, now_us) -> &Hands`.
- **VALIDATE**: replay both traces through `Conditioner` in a test; no panics, both roles present in hold windows.

#### Task B5: Derived features (M1 subset)
- **ACTION**: `handos-hands::features`.
- **IMPLEMENT**: `palm_center` (mean of 0, 5, 9, 13, 17), `hu_world` (|w5 − w17|), `hu_screen` (max of |s5 − s17| and 0.8 × |s0 − s9|), `facing` (normal `(w5 − w0) × (w17 − w0)` normalised, `facing = (1 + nz × sign) / 2` with sign −1 for left, +1 for right on **un-mirrored** world data; 0 palm to camera, 1 back), `pinch(f).strength = clamp((0.60 − d/hu)/0.40, 0, 1)` with d = world distance thumb tip to the tip of f, `pinch(f).point` midpoint, `extension(f)` (angle at the PIP joint: ≥ 160° → 1, ≤ 90° → 0), `pose` ∈ {open (all five extensions ≥ 0.7), fist (all four fingers ≤ 0.3), other} with score = smallest margin.
- **GOTCHA**: See A6: the sign is defined for un-mirrored world coordinates. Mirror only for display and mapping.
- **VALIDATE**: trace test: during every hold window of `palm-good.htrace` (palms to camera), median `facing` < 0.2 for both hands. Unit tests for pinch strength endpoints (0.20 hu → 1, 0.60 hu → 0).

#### Task B6: Triggers, phases, claims (M1 subset)
- **ACTION**: `handos-gesture::{trigger, phase, claims}`.
- **IMPLEMENT**: Primitives `Pinch { hand, finger }` (enter 0.75 / exit 0.45), `Pose { hand, is }` (0.7 / 0.5), `Facing { hand, side }` (0.75 / 0.6), `Present { hand }`; combinator `All`; modifiers `hold(t)`, `tap { max: 0.3 s, max_move: 0.3 hu }`. Phase machine per trigger: `Began` after score ≥ enter for **two tracked frames**, `Changed` at most once per frame, `Ended` at score ≤ exit, `Cancelled` when a hand it depends on is `Lost`, on mode change, or when a higher-priority owner takes its claim; taps emit a single `Fired`. Claims: resources `hand:left|right`, `pinch:<hand>:<finger>`, `pointer`; a trigger may begin only if its resources are free or held by a lower priority.
- **VALIDATE**: unit tests per transition; `proptest` properties over random score sequences: `Ended`/`Cancelled` never without `Began`; never two owners of one resource. Timings must hold at 15 fps (run the unit tests at 15 and 30 fps frame spacing).

#### Task B7: Clutch and modes
- **ACTION**: `handos-gesture::clutch`.
- **IMPLEMENT**: Modes `Sleep` (only the wake trigger live) and `Default`. Wake = `All[Pose{any, open}, Facing{any, palm}].hold(0.5 s)`; sleep = `Pose{any, fist}.hold(1.0 s)`, or 30 s with no hand `Tracked`, or external toggle/sleep/wake. After waking, pointer actions are suppressed for 300 ms (wake grace) so the palm-to-pointer transition cannot click. Emits `ModeChanged`.
- **VALIDATE**: unit tests for each transition; replay test: `pluck-good.htrace` replayed from asleep: count wakes that happen outside open-palm hold windows (expect 0).

#### Task B8: Mapping
- **ACTION**: `handos-gesture::mapping`.
- **IMPLEMENT**: The box comes in as a value (from `handos.yaml`, `calibration.yaml` or the default — resolved by the daemon, Task D7); the mapping itself does no I/O. Mirror x (x' = 1 − x). Step 1: `s = origin + (m − box.min) / (box.max − box.min) × size`, clamped, m = palm centre. Step 2: any point p → `s + k × (map(p) − map(m))`, k = `mapping.hand_scale` (0.6). Pointer = landmark 5 through this transform. Output = the target output's logical rect (`mapping.target`: `active-output` = Hyprland's focused monitor at wake time, `all-outputs` = the layout's bounding box, or an output name).
- **VALIDATE**: unit tests: box corners → output corners; clamping; k = 1 reduces to a plain box map.

#### Task B9: Pointer actions (the M1 gesture set)
- **ACTION**: `handos-gesture::pointer` — turns events into `PointerCmd { Move{x,y,output}, Button{btn, pressed}, Axis{dy}, AxisStop }`.
- **IMPLEMENT**: Pointer hand = `pointer.hand` (default right). Move: every frame while awake, unless a scroll is active. Left: on `Pinch{right,index}` `Began` remember time and point; if it `Ended` within 0.3 s and moved < 0.3 hu → `Button(left, down)` + `Button(left, up)` (click on release). Once held > 0.3 s or moved ≥ 0.3 hu → `Button(left, down)` at the begin position (drag starts), then moves, `Button(left, up)` on `Ended` **or `Cancelled`**. Right click = `Pinch{right,middle}.tap`. Scroll = `Pinch{right,middle}` held > 0.3 s: `Axis(dy = Δpinch.y × pointer.scroll_gain)` (default 1.0 screen px per px, natural direction: content follows the hand), pointer frozen, `AxisStop` on end. Index and middle pinches on one hand both claim `pointer` (first wins). A `PressedSet` records every button down; every cancel, sleep and shutdown path releases it first.
- **VALIDATE**: unit tests for click vs drag thresholds, cancel-releases-button, scroll freeze; `proptest`: after any event sequence ending in `Cancelled` or sleep, `PressedSet` is empty. Replay test on `pluck-good.htrace` awake: count left-pinch `Began` events inside rep windows; expect ≥ 45 of 50 (spike 1 pass bar), log the number.

### Phase C — Overlay, rendering and input on Wayland (PR 3)

#### Task C1: Platform traits
- **ACTION**: `handos-platform`.
- **IMPLEMENT**: `trait Overlay { fn outputs(&self) -> Vec<OutputInfo>; fn draw(&mut self, output: OutputId, list: &DisplayList) -> Result<(), PlatformError>; fn clear(&mut self, output: OutputId); }` and `trait Input { fn pointer_abs(&mut self, output: OutputId, x: f64, y: f64); fn button(&mut self, btn: Button, pressed: bool); fn axis(&mut self, dy: f64); fn axis_stop(&mut self); }`, `struct OutputInfo { id, name, logical_rect, scale }`, `Features` (`pointer.absolute`, `overlay`). Keyboard/clipboard/capture/windows are left out (NOT Building).
- **VALIDATE**: compiles; a `FakeInput`/`FakeOverlay` behind `#[cfg(any(test, feature = "fake"))]` records calls for daemon tests.

#### Task C2: `handos-render` — display list + wgpu backend
- **ACTION**: Port the spike's GPU path.
- **IMPLEMENT**: `enum Prim { Line { a, b, width, rgba }, Dot { at, radius, rgba } }`, `DisplayList(Vec<Prim>)`, `Gpu::new(display_ptr, surface_ptr)` per WGPU_ON_WAYLAND, `render(&DisplayList) -> bool` = one instanced draw (copy `skeleton.wgsl` verbatim), `resize`. `skeleton(hand_points_screen, style) -> impl Iterator<Item = Prim>` using the 21 `CONNECTIONS` (`spikes/overlay-gpu/src/main.rs:43-65`).
- **MIRROR**: WGPU_ON_WAYLAND; spike tests `instances_cover_every_bone_then_every_joint`, `instances_are_capped_at_the_buffer_size`, `shader_has_both_entry_points` (main.rs:730-768) → port as they are.
- **GOTCHA**: Golden-image tests compare within a tolerance (Roadmap tab), so there are no exact-pixel tests; test the display list instead.
- **VALIDATE**: ported unit tests pass.

#### Task C3: Vulkan driver selection
- **ACTION**: `handos-platform-wayland::vulkan::select_driver()` called first thing in `main`, before any thread exists.
- **IMPLEMENT**: If `VK_DRIVER_FILES` or `VK_ICD_FILENAMES` is set, leave it. Else, if `overlay.vulkan_driver` is set in settings, use it. Else list `/usr/share/vulkan/icd.d/*.json` and `/etc/vulkan/icd.d/*.json`, drop names containing `nvidia`, and if exactly one remains (or one named `intel_icd*`/`radeon_icd*`), set `VK_DRIVER_FILES` to it. Log the choice.
- **GOTCHA**: Spike 2: with every driver loaded RSS went from 24.7 to 69 MiB, failing the 40 MB budget. `std::env::set_var` is `unsafe` in edition 2024: call it before spawning any thread (document with `// SAFETY:`), which is why it runs first in `main` (after reading settings, which spawns nothing).
- **VALIDATE**: unit test on a temp dir of fake ICD files; `handos doctor` (D5) prints the chosen driver; bench RSS (D6) ≤ 40 MB.

#### Task C4: Wayland overlay
- **ACTION**: `handos-platform-wayland::{outputs, overlay}`.
- **IMPLEMENT**: One layer surface per output in `overlay.outputs` (LAYER_SURFACE), each with its own `Gpu`, created lazily on first draw. `draw()`: request a frame callback, render, present; `clear()`: render an empty list once, then **no further commits** until the next draw. Track output hot-plug (`new_output`/`output_destroyed`): create/destroy surfaces. Integer scale via `set_buffer_scale` (as the spike); fractional scale is untested (spike 2 open item) — if an output's scale is fractional, log a warning.
- **MIRROR**: LAYER_SURFACE, the `CompositorHandler`/`LayerShellHandler` impls in `spikes/overlay-gpu/src/main.rs:377-452`.
- **GOTCHA**: A commit without a new buffer gets no frame callback on Hyprland (spike comment main.rs:351-353): never wait for a callback to clear.
- **VALIDATE**: manual: `handos trace replay traces/palm-good.htrace` shows both skeletons; clicks on windows under the overlay still work (the click-through check left open by spike 2).

#### Task C5: Wayland input
- **ACTION**: `handos-platform-wayland::input`.
- **IMPLEMENT**: Bind `zwlr_virtual_pointer_manager_v1` v2 (fail with a clear error if only v1 is offered: absolute motion would span the layout); one virtual pointer per output (VIRTUAL_POINTER); `pointer_abs` uses that output's logical size as extent; `button` and `axis` (`axis_source(Finger)`, `axis(time, VerticalScroll, dy)`, `axis_stop` on scroll end) each followed by `frame()`.
- **MIRROR**: VIRTUAL_POINTER.
- **VALIDATE**: manual: `handos trace replay traces/live-check.htrace` moves the system pointer.

#### Task C6: Hyprland focused monitor
- **ACTION**: `handos-platform-wayland::hyprland::focused_monitor() -> Option<String>`.
- **IMPLEMENT**: Connect to `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`, write `j/monitors`, parse the JSON, return the `name` with `"focused": true`. `None` when not on Hyprland → fall back to the first output.
- **VALIDATE**: unit test parses a captured `hyprctl -j monitors` sample committed as `crates/handos-platform-wayland/tests/fixtures/monitors.json`.

#### Task C7: Daemon wiring
- **ACTION**: `handos::daemon` puts it together on one thread.
- **IMPLEMENT**: calloop loop with sources: `WaylandSource`, tracker listener + connection, control listener (D3), a housekeeping `Timer` every 250 ms (tracker watchdog, idle and adaptive rate, the 30 s sleep timer). `on_frame`: `Conditioner::update` → gesture engine → `PointerCmd`s to `Input` → if awake and a hand is visible, build the display list (skeletons mapped with B8, predicted to now + 8 ms, plus the mode dot) → `Overlay::draw`; if no hand is visible or asleep → `clear` once. Adaptive rate: send `Set{mode: active, fps: 30}` while a trigger is active or palm speed > 0.5 hu/s, else `fps: 15`; `idle` (5 fps) after 3 s without hands; `off` when asleep with `clutch.wake: hotkey`, or paused.
- **GOTCHA**: The main thread never blocks: tracker exit is watched with a `Generic` source on a pidfd (`libc::syscall(SYS_pidfd_open, pid, 0)`), not `wait()`.
- **VALIDATE**: `crates/handos/tests/replay.rs` drives `Daemon` with `FakeOverlay`/`FakeInput` from `traces/pluck-good.htrace` awake and asserts ≥ 45 left clicks or drags begin inside rep windows and the pressed set is empty at the end. Manual: live tracker, S1 end to end.

### Phase D — Settings, control, operations, exit tests (PR 4)

#### Task D1: Settings
- **ACTION**: `handos::settings`.
- **IMPLEMENT**: Structs for the M1 subset of `handos.yaml`: `tracker { command, camera, resolution, fps, idle_fps, filter{min_cutoff,beta,d_cutoff}, filter_world{…} }`, `overlay { outputs: all|[names], skeleton: bool, vulkan_driver: Option<path> }`, `mapping { target, box, hand_scale }`, `pointer { hand: left|right, scroll_gain }`, `clutch { wake: gesture|hotkey|both }`, `control { eval: bool }` (accepted, unused until M2). Every struct `#[serde(deny_unknown_fields, default)]`; parse with `serde_saphyr::from_str_with_options(text, options!{ strict_booleans: true, reject_unsupported_tags: true, duplicate_keys: DuplicateKeyPolicy::Error, budget: budget!{ max_anchors: 0 } })`. A missing file → defaults. Keys from later milestones (`limits`, `permissions`, `providers`, `packages`) are **rejected** in M1 with a message naming the milestone, so typos are never silently ignored. Path: `~/.config/handos/handos.yaml`, overridable with `--config`.
- **GOTCHA**: Default `tracker.command` = `["handos-tracker-mp"]` (installed with `uv tool install ./trackers/mediapipe-py`). Filter defaults are the spike-measured values (ONE_EURO section), not the doc's placeholders. Check against Context7 that the `budget!` field name for anchors is `max_anchors` in serde-saphyr 2.1 (the docs example used it).
- **VALIDATE**: unit tests: empty file → defaults; unknown key → error with line snippet; `wake: no` → error (unknown enum variant); anchors rejected; `skeleton: yes` → error (strict booleans). `tests/cli.rs`: `handos daemon --config bad.yaml` exits 1 with the snippet on stderr.

#### Task D2: Pressed-set safety
- **ACTION**: `handos::pressed`.
- **IMPLEMENT**: Wrapper around `Input` that records buttons down; `release_all()` called first on: cancel, sleep, pause, tracker loss, control `shutdown`, SIGTERM/SIGINT (calloop's `signals` feature), and in `Drop`. The main loop wraps `on_frame` in `std::panic::catch_unwind`; on a panic it releases all, logs, and exits non-zero (systemd restarts it).
- **GOTCHA**: After SIGKILL nothing runs: verify manually whether Hyprland releases buttons when the virtual pointer's client disconnects (kill -9 during a drag). Record the answer in `docs/m1-results.md` and the Architecture failure table.
- **VALIDATE**: unit tests with `FakeInput`: every listed path ends with all buttons up.

#### Task D3: Control socket + CLI client
- **ACTION**: `handos::control` (server) and the CLI side in `cli.rs`.
- **IMPLEMENT**: `control.sock` (mode 0600), JSON-RPC 2.0, one object per line, non-blocking per-connection buffers in calloop. M1 methods: `status` → `{ mode, tracker: { state, fps, infer_ms, dropped }, uptime_s, outputs, vulkan_driver }`; `mode.get`; `clutch.toggle|sleep|wake`; `tracker.pause|resume`; `trace.record { path }` / `trace.record { stop: true }`; `bench.run` (D6); `calibrate.capture` and `calibrate.reload` (D7); `shutdown`; `subscribe { topics }` for notifications `mode.changed`, `tracker.state`. Others → JSON-RPC error `-32601`. CLI: thin client; exit codes 0 ok, 1 error, 2 usage (clap), 3 daemon not running.
- **VALIDATE**: `tests/cli.rs`: `handos status` with no daemon → exit 3; with a headless daemon (`handos daemon --headless --replay traces/palm-good.htrace`) → exit 0 and JSON containing `"mode"`.

#### Task D4: systemd unit + hotkey
- **ACTION**: `packaging/systemd/handos.service`, README section.
- **IMPLEMENT**: `[Unit] PartOf=graphical-session.target`, `After=graphical-session.target`; `[Service] ExecStart=%h/.cargo/bin/handos daemon`, `Restart=on-failure`, `RestartSec=1`, `Environment=HANDOS_LOG=info`; `[Install] WantedBy=graphical-session.target`. README: `bind = SUPER, H, exec, handos toggle` for Hyprland, install steps (`cargo install --path crates/handos`, `uv tool install ./trackers/mediapipe-py`).
- **VALIDATE**: manual: `systemctl --user start handos` → `handos status` works; `kill -9` the daemon → it is back within 2 s.

#### Task D5: `handos doctor`
- **ACTION**: `handos::doctor` — runs without a daemon; prints ✓/✗ lines and exits 1 if any required check fails.
- **IMPLEMENT**: Wayland display reachable; globals present: `zwlr_layer_shell_v1`, `zwlr_virtual_pointer_manager_v1` (version ≥ 2); Vulkan ICD choice (C3) and that an adapter opens; tracker command found on `PATH`; model file present with the pinned SHA-256; camera via `handos-tracker-mp --probe` — warn below 24 fps (too dark) and print brightness; Hyprland `ecosystem:enforce_permissions` via `hyprctl getoption ecosystem:enforce_permissions -j` — if on, print the permission rules to add; settings parse.
- **VALIDATE**: `tests/cli.rs`: `WAYLAND_DISPLAY=nonexistent handos doctor` exits 1 and names the failed check; a manual run on this laptop shows all ✓.

#### Task D6: `handos bench`
- **ACTION**: `handos::bench` — measures the PRD budgets that M1 can measure.
- **IMPLEMENT**: Against the running daemon (`bench.run`): 20 s asleep then 20 s with hands (prompts on stderr, like `spikes/two-hand/bench.py`): CPU % of one core for daemon, tracker and compositor (find Hyprland's pid; subtract its idle baseline from a 5 s window with the overlay cleared), daemon RSS, frame rate, internal latency `t_capture → commit` p50/p95 (the daemon timestamps each present). Offline: `handos bench --jitter <trace> [--windows <reps.json>]` replays headless and reports pointer jitter in px RMS over still windows. Writes JSON to `bench/<time>-<sha>.json` and prints a Markdown summary.
- **GOTCHA**: Motion-to-photon cannot be measured internally; the slow-motion video method from the Roadmap tab stays manual (D8).
- **VALIDATE**: `handos bench --jitter traces/palm-good.htrace --windows traces/palm-good.reps.json` prints a number; unit test for the RMS function.

#### Task D7: `handos calibrate`
- **ACTION**: Terminal-guided calibration of the mapping box and the user's pinch distances (Tracking spec, "Calibration"; CLI spec, `handos calibrate`). Runs before the exit tests so they use the user's own values.
- **IMPLEMENT**:
  - Control method `calibrate.capture { what, seconds }` (D3's server), `what` ∈ `left|right|top|bottom|pinch_open|pinch_closed`. The daemon waits until the pointer hand is `Tracked` and still (palm speed < 0.1 hu/s for 0.5 s), then averages over `seconds` (default 1.0) and returns `{ value, samples, stddev }`: the palm centre's x (left/right) or y (top/bottom) in mirror space, or the thumb–index world distance in hu (pinch). Times out after 15 s with an error the CLI shows.
  - CLI `handos calibrate` (`crates/handos/src/calibrate.rs`): wakes the daemon, then prompts on stderr one step at a time — "Rest your elbows as you would while working. Move your right hand to the furthest LEFT you can reach comfortably and hold still…" — for the four edges, then "Open thumb and index fully… hold", "Pinch thumb and index firmly… hold". Shows a countdown and the captured value after each step; `r` repeats a step, `q` aborts without writing.
  - Result: box = `(left, top)`–`(right, bottom)` in mirror space; pinch `open_hu`, `closed_hu`. Reject and repeat the step when the box is narrower or shorter than 0.15, or `open_hu − closed_hu < 0.15`. Write `~/.local/state/handos/calibration.yaml` (`version: 1`, `box`, `pinch: { open_hu, closed_hu }`, `measured_at`), then call `calibrate.reload` so the running daemon uses it at once.
  - Use: `pinch(f).strength = clamp((open_hu − d/hu) / (open_hu − closed_hu), 0, 1)` (defaults 0.60 and 0.20 reproduce B5's formula). Precedence: an explicit `mapping.box` in `handos.yaml` > `calibration.yaml` > built-in default; `handos calibrate` warns when `handos.yaml` sets `mapping.box`, because that value would win. `handos status` reports which source is in use; `handos doctor` reports whether a calibration exists.
- **MIRROR**: D1's strict parsing (`deny_unknown_fields`, serde-saphyr options) for reading `calibration.yaml`; D3's JSON-RPC style.
- **GOTCHA**: Capture the palm centre, not the pointer (landmark 5): the box is defined on the palm centre (B8 step 1). Mirror space, so "left" is the user's left as seen on screen. Calibrate with the pointer hand (`pointer.hand`); the box applies to both hands.
- **VALIDATE**: unit tests: box from four samples; rejection of a too-small box and too-close pinch distances; precedence of the three sources; strength formula equals B5's at the defaults. Headless test: `handos daemon --headless --replay traces/palm-good.htrace` + `calibrate.capture { what: "pinch_open" }` returns a value within the trace's range. Manual: after calibrating, the comfortable left/right/top/bottom positions reach the output's edges.

#### Task D8: Exit tests and budget record
- **ACTION**: Run the M1 exit criteria and record the results.
- **IMPLEMENT**: (1) 20 s still hold recorded with `handos trace record traces/still-hold.htrace`, then `handos bench --jitter` → compare with 3 px RMS. (2) Pinch click 95/100: open `tools/click-test.html` (100 sequential 48 px targets at random positions, counts hits and misses, shows the score), click all 100 by pinching. (3) Asleep false activations: `handos sleep`, then 2 hours of normal desk work with `handos trace record traces/desk-2h.htrace`; count wakes from `mode.changed` (expect 0); keep the trace as a soak fixture if under ~150 MB, else a 20-minute excerpt. (4) `handos bench` live. (5) Slow-motion video latency (phone at 240 fps filming hand and screen, count frames) for p50.
- **VALIDATE**: All five numbers written into `docs/m1-results.md`, and the design doc's PRD budget table updated (met, or revised with the measured value) — the M1 exit criterion is "met or revised".

#### Task D9: Delete the spikes
- **ACTION**: Remove `spikes/` and its `exclude` entry; update `.gitignore`.
- **IMPLEMENT**: Before deleting, confirm every pattern listed in this plan has been ported and `traces/` holds the converted fixtures. `spikes/RESULTS.md` moves to `docs/m0-spike-results.md` (the design doc and PRD index cite it — update those references); raw spike JSONL stays in git history.
- **VALIDATE**: `cargo test --workspace` and the Python tests still pass; `grep -rn "spikes/" --include='*.rs' --include='*.py' --include='*.md' .` finds only historical references in `docs/`.

---

## Testing Strategy

### Unit Tests

| Test | Input | Expected Output | Edge Case? |
|---|---|---|---|
| proto round trip | 0/1/2/4 hands, ± world | identical frame | yes (0 and 4) |
| proto rejects | bad protocol, 5 hands, short buffer, NaN | `ProtoError` variant | yes |
| golden bytes (Rust + Python) | fixed frame | equals `two_hands.bin` | no |
| One Euro | noisy constant; step | ≥ 50% jitter cut; converges ≤ 10 frames | no |
| outlier | one 2-hu jump; three in a row | dropped; third accepted | yes |
| identity | flicker; overlap; 500 ms gap; same raw label twice | role held; frozen; kept; never shared | yes |
| facing (trace) | palm-good hold windows | median facing < 0.2 both hands | no |
| pinch strength | d = 0.2 hu, 0.6 hu | 1.0, 0.0 | yes |
| phases (proptest) | random score sequences at 15 and 30 fps | no Ended without Began; ≤ 1 owner per resource | yes |
| click vs drag | release at 0.25 s / 0.35 s; move 0.2 / 0.4 hu | click; drag | yes |
| pressed set (proptest) | any sequence ending in cancel/sleep/shutdown | empty | yes |
| clutch | open palm 0.4 s / 0.6 s; fist 1 s; 30 s no hands | no wake / wake; sleep; sleep | yes |
| mapping | box corners, outside box, k = 1 | output corners; clamped; plain map | yes |
| settings | empty, unknown key, anchors, `wake: no`, `skeleton: yes` | defaults; error with snippet ×4 | yes |
| vulkan select | fake icd.d with intel+nvidia; env preset | intel chosen; env untouched | yes |
| calibration | four edge samples; 0.1-wide box; open − closed = 0.1 hu; yaml + calibration + default box | box; rejected; rejected; yaml wins, then calibration | yes |
| pinch strength with calibration | open 0.60 / closed 0.20 | identical to B5 | no |
| replay: pluck awake | `pluck-good.htrace` | ≥ 45/50 pinch begins inside reps; pressed set empty | no |
| replay: asleep | `pluck-good.htrace` from Sleep | 0 wakes outside open-palm holds | no |

### Edge Cases Checklist
- [ ] No camera / camera busy → tracker status error, daemon asleep, "no camera" in `status`, retry with backoff
- [ ] Tracker crash mid-drag → hands lost → `Cancelled` → button released
- [ ] Hand leaves the frame mid-drag (coasting 150 ms, then cancel)
- [ ] Output unplugged while awake → surfaces recreated, mapping target falls back to the first output
- [ ] Two daemons started → the second exits 1 "already running"
- [ ] Malformed packet from the tracker → logged at most once per second, connection kept
- [ ] Settings file with a later-milestone key → clear error
- [ ] Fractional output scale → warning (untested path)
- [ ] `clutch.wake: hotkey` asleep → camera released (light off)

---

## Validation Commands

### Static Analysis
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
(cd trackers/mediapipe-py && uvx ruff check --line-length 120 .)
```
EXPECT: no warnings

### Unit + replay tests
```bash
cargo test --workspace
(cd trackers/mediapipe-py && uv run pytest -q)
```
EXPECT: all pass

### Build
```bash
cargo build --release -p handos
uv tool install --force ./trackers/mediapipe-py
```
EXPECT: `handos` and `handos-tracker-mp` on PATH

### Headless end to end
```bash
handos trace replay traces/pluck-good.htrace --headless --speed 0
handos bench --jitter traces/palm-good.htrace --windows traces/palm-good.reps.json
```
EXPECT: exit 0, click count and jitter printed

### Manual Validation
- [ ] `handos doctor` all ✓ on the reference laptop
- [ ] Right hand raised → log says `handedness=right` (Task A8)
- [ ] Wake with an open palm; skeleton and green dot appear on the focused monitor
- [ ] Pointer follows the right index knuckle across the whole output with hands at chest height
- [ ] Quick pinch clicks; held pinch drags a window; middle pinch scrolls a browser page; quick middle pinch opens a context menu
- [ ] Windows under the overlay receive real mouse clicks while it is drawn (spike 2 open item)
- [ ] Fist 1 s → sleep; skeleton gone; `handos toggle` wakes and sleeps
- [ ] `handos pause` → camera light off; `resume` → back
- [ ] `handos calibrate` completes; afterwards the comfortable reach positions hit the output's edges, and `handos status` says the box comes from `calibration.yaml`
- [ ] `kill -9` the tracker mid-drag → button released, tracker restarted
- [ ] `kill -9` the daemon mid-drag → note whether Hyprland released the button (D2)
- [ ] Exit tests D8 (jitter, 95/100 clicks, 2 h asleep, bench, slow-motion latency)

---

## Acceptance Criteria
- [ ] All tasks completed, four PRs merged
- [ ] All validation commands pass
- [ ] Pinch click ≥ 95 of 100 on `tools/click-test.html`
- [ ] 0 false activations asleep in 2 hours
- [ ] Latency and jitter budgets met, or revised in the design doc with measured values
- [ ] Daemon RSS ≤ 40 MB; compositor overhead ≤ 6% of a core while tracking (PRD)
- [ ] `spikes/` deleted; `docs/m0-spike-results.md` and `docs/m1-results.md` present
- [ ] PRD index: M1 `complete`

## Completion Checklist
- [ ] No `unwrap`/`expect` outside tests (clippy enforces `unwrap_used`)
- [ ] Errors: `thiserror` per library crate, `anyhow` + context in the binary
- [ ] Logging: `tracing` only, `trace!` on per-frame paths
- [ ] Every cancel/sleep/pause/shutdown path releases pressed buttons first
- [ ] Hand and gesture crates do no I/O (Architecture rule) — tested from traces
- [ ] Spec thresholds are named constants in one `defaults` module per crate, settings where the spec says so
- [ ] Design doc corrections made where the plan found them (see Notes)

## Risks
| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Handedness swapped on un-mirrored input | High | High | Explicit on-camera check (A8) before Phase B; the swap is one line |
| Pointer jitter > 3 px RMS at chest-height reach | Medium | High | Measure first (D8.1); tune 1€ with the doc's procedure; revise the budget if needed |
| Pinch click < 95/100 (tap vs drag confusion at 15 fps, or pinch distances that do not fit the user) | Medium | High | Calibrate first (D7); thresholds are constants/settings; recorded misses become new `traces/` fixtures |
| Latency above 90 ms p50 (inference 27 ms vs 16 ms budgeted) | High | Medium | Prediction on; record the measured value and revise the budget (allowed by the exit criterion) |
| Hyprland does not release buttons when a killed client's virtual pointer goes away | Low | High | D2 manual check; document it; systemd restart releases on the next start (`release_all` at startup) |
| wgpu or Mesa update breaks the Vulkan path | Low | High | `tiny-skia` fallback remains possible behind the display list (ADR 5); not built in M1 |
| XL scope slips past week 10 | Medium | Medium | Four PRs, each usable; D8's 2-hour test can run while polishing |

## Notes
- **Design doc corrections found while planning** (lay them as suggestions when the related task lands):
  1. Tracking spec, tracker rule: the A8 live check supports keeping raw labels and swapping labels on mirrored recordings.
  2. Platform backends, Buffers row: still says `wl_shm` ARGB8888; after ADR 5 it is a wgpu Vulkan swapchain.
  3. CLI spec, `handos.yaml` sample: the filter values are placeholders; replace them with the spike-measured defaults (D1); add `pointer` and `overlay.vulkan_driver`.
  4. Tracking spec, `set` control message table: add the `fps` field (already described in the "Adaptive rate" paragraph).
  5. Tracking spec, `facing`: state the coordinate convention (un-mirrored world) next to the sign.
- **Why the gestures are in Rust**: the M1 roadmap row says "pinch click and drag coded in Rust"; M2's exit criterion deletes them for `std.pointer`. Keep `handos-gesture::pointer` small and isolated so that deletion is clean.
- **Python tracker CPU**: 61–111% of a core (spikes/RESULTS.md); exempt until M5. Adaptive rate (C7) is the M1 lever.
- **Defaults chosen in this plan, easy to change**: pointer hand right; natural scroll direction; 300 ms wake grace; 250 ms housekeeping tick; mode badge = green dot top-right.
