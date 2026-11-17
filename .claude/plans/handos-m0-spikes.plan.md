# Plan: M0 Risk Spikes

**Source PRD**: `.claude/prds/handos.prd.md` (full doc: https://claude.ai/artifact/NLXWfvV2Ra2f5kvzXcawBm, Roadmap & testing tab)
**Selected Milestone**: M0 Spikes (weeks 1–4, ~60 h)
**Complexity**: Medium

## Summary
Four throwaway prototypes answer the four questions the design cannot answer on paper. Each ends with a short written result added to the Decision records tab. Spikes live in `spikes/` and are deleted when M1 begins.

## Patterns to Mirror
No code exists yet: the repo has only `README.md`. There are no patterns to follow. Project rules from the design doc: `cargo fmt`, clippy with warnings as errors, current stable Rust, and conventional commits.

## Files to Change
| File | Action | Why |
|---|---|---|
| `spikes/two-hand/` (Python) | CREATE | Spike 1: two-hand tracking |
| `spikes/overlay/` (Rust) | CREATE | Spike 2: overlay cost |
| `spikes/input/` (Rust) | CREATE | Spike 3: input and text |
| `spikes/luau/` (Rust) | CREATE | Spike 4: script runtime |

## Tasks
### Task 1: Two-hand tracking
- **Question**: Can MediaPipe follow both hands through the palm-touch gesture on the laptop webcam?
- **Action**: A Python script records video and landmarks for 50 palm touches and 50 fingertip plucks, in good and in dim light. Count dropouts and handedness swaps; tune the 1€ filter; check the `facing` sign for both hands.
- **Pass bar**: 45 of 50 palm touches recognised with no swap, and no dropout longer than 150 ms.
- **If it fails**: Redesign the reference gesture so the hands come close without overlapping.

### Task 2: Overlay cost
- **Question**: What do a click-through layer-shell overlay and CPU rendering cost on Hyprland?
- **Action**: A minimal Rust program draws a skeleton from a replayed trace at 60 Hz. Measure memory, CPU and frame time; confirm clicks pass through; test two outputs and fractional scaling.
- **Pass bar**: 40 MB RSS or less, 3% of a core or less while animating, frames of 2 ms or less.
- **If it fails**: Try a GPU path, or render the overlay at a lower resolution.

### Task 3: Input and text
- **Question**: Do the virtual pointer and keyboard give exact clicks and correct text, Persian included, in real applications?
- **Action**: Click a grid of targets across two monitors. Type an English and a Persian sentence into a terminal, a browser field, a GTK application and a Qt application, by generated keymap and by paste. Record what Hyprland's permission system requires.
- **Pass bar**: Every click within 2 px; both sentences correct in all four applications by at least one method.
- **If it fails**: Make paste the default; use uinput for the pointer.

### Task 4: Script runtime
- **Question**: Does mlua with Luau support the process model?
- **Action**: A 200-line scheduler with wait, sleep and an async HTTP call. Then attack it: an endless loop, a memory bomb, deep recursion at the memory limit, and 1,000 reloads.
- **Pass bar**: No case aborts the host; a reload of the standard library takes 50 ms or less; memory stays flat.
- **If it fails**: Switch mlua to Lua 5.4 with a hand-built sandbox, or run the VM in its own process.

## Validation
Each spike produces a measured result against its pass bar and a go or no-go.

## Risks
| Risk | Likelihood | Mitigation |
|---|---|---|
| R1: Tracking breaks when a finger touches the other palm | High | Spike 1 measures it on recorded clips. Fallbacks: short dwell, coasting through brief dropouts, gestures where the hands come close without overlapping |
| R7: Script runtime edge cases in Luau or mlua | Medium | Spike 4 tests them; mlua's Lua 5.4 feature keeps a fallback open |

## Acceptance
- [ ] Each spike has a measured result and a go or no-go
- [ ] Results added to the Decision records tab
