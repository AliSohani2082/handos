# Implementation Report: M0 extra spike — GPU overlay

## Summary
This spike draws spike 2's overlay with wgpu (Vulkan) instead of tiny-skia, and also measures Hyprland's CPU for both renderers. Verdict: **GO when frames follow the tracker** (2.4% CPU at 18 frames/s). At 60 frames/s it is NO-GO, at 6.8%, which is still four times less than CPU drawing. Full numbers are in `spikes/RESULTS.md` under Spike 2.

## Assessment vs Reality
| Metric | Predicted (Plan) | Actual |
|---|---|---|
| Complexity | Small | Small |
| Files | 1 crate and 1 script | 1 crate (Rust and WGSL), 1 script, 4 result files, RESULTS.md |

## Tasks Completed
| # | Task | Status | Notes |
|---|---|---|---|
| 1 | Layer-shell overlay with wgpu | Complete | One output, empty input region, frame callbacks |
| 2 | Instanced SDF skeleton | Complete | One draw call per frame |
| 3 | Reuse replay and the measurement code | Complete | `#[path]` include of `overlay/src/replay.rs` |
| 4 | Flags | Complete | `--backend`, `--adapter`, `--every`, `--seconds`, `--json` |
| 5 | `measure.sh` (Hyprland CPU) | Complete | |
| 6 | Runs and write-up | Complete | GL backend did not get an adapter; not pursued |

## Validation Results
| Level | Status | Notes |
|---|---|---|
| Static analysis | Pass | `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt` |
| Unit tests | Pass | `overlay-gpu`: 5 (3 new, 2 from replay) |
| Build | Pass | `cargo build --release` |
| Integration | Pass | 7 live runs on Hyprland 0.57.2 |
| Edge cases | Partial | Fractional scale and click-through not checked by hand (you were away) |

## Deviations from Plan
- Added `--every N` (draw every Nth display frame). Why: 60 fps missed the bar, and the tracker only produces about 15 fps.
- The first version of `--every` committed without a buffer to ask for the next callback, which hung, because Hyprland sends no callback for such a commit. It now waits on a timer, rounded up so that a sub-millisecond wait does not become a spinning 0 ms poll.

## Issues Encountered
- The Vulkan loader also loads NVIDIA's driver (+44 MiB RSS). Fixed for the measurement with `VK_DRIVER_FILES`; the daemon must choose the integrated GPU itself.
- GL backend: no compatible adapter without passing the display handle to the instance. Not pursued.

## Next Steps
- [ ] You: put the ADR 5 decision in the design doc (wgpu, Vulkan, integrated GPU, draw at tracking rate)
- [ ] You: decide whether compositor CPU counts toward the overlay budget
- [ ] Rerun the browser text case with the machine idle, then mark M0 complete
- [ ] `/code-review`, `/prp-commit`, `/prp-pr`
