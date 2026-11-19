# M1 Pointer Phase A

Phase A adds the Rust workspace, HandFrame and control packets, the Python MediaPipe tracker, and live recording and headless replay. The daemon can start and supervise the tracker. Gesture processing, Wayland output, and settings are in later phases.

| Task | Result |
|---|---|
| A1 Workspace | All seven crates and the existing spike workspace build. |
| A2 HandFrame | Tests cover finite coordinates, sizes, hand counts, and bad packets. |
| A3 Control and traces | Tests cover control JSON, trace round trips, truncation, and oversize records. |
| A4 Golden packet | The Rust and Python encoders produce the same 1,065 bytes. |
| A5 Tracker package | The package installs and passes its four Python tests. MediaPipe loaded the verified model and detected two hands in the local spike video. |
| A6 Fixtures | Converted 2,412 palm frames and 2,406 pluck frames. The original hold windows contain two hands in 22 of 60 palm frames and 60 of 60 pluck frames. |
| A7 Daemon and replay | A stub tracker sends packets over a real Unix socket. The daemon records the valid frame, rejects a second daemon, and removes the socket on SIGTERM. Release replay reads all 2,406 pluck frames. |
| A8 Live handedness | Passed on the reference laptop. The original swap reversed both labels. After removing the swap, each hand showed its correct label. `traces/live-check.htrace` contains 154 frames over 9.93 seconds. In 148 frames, both hands had the correct labels. |

The palm recording starts the hold window before both hands appear. The conversion preserves the source timestamps and counts. A6's 95% hold coverage target passes for pluck. It requires a new palm recording.

The package uses OpenCV Contrib 5.0, which provides the same `cv2` API and satisfies MediaPipe's dependency. This prevents two OpenCV packages from installing files into the same module. The daemon uses a file lock to manage the runtime socket so that two daemons cannot race when they start. `handos trace record FILE` starts a recording daemon until the Phase D control socket is available.

The local M1 plan supplied the wire layout and interim behavior. The live check confirmed the design rule recorded in the plan: keep labels on raw camera images and swap labels when converting the mirrored M0 recordings. The converted traces and the Phase B palm-facing signs use this rule.

Rust formatting, Clippy, all 10 Rust tests, the release build, and Ruff passed.
