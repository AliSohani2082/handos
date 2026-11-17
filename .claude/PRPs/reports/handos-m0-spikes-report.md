# Implementation Report: M0 Risk Spikes (partial)

## Summary
Built all four throwaway spikes from `.claude/plans/handos-m0-spikes.plan.md` in `spikes/`, and ran the three that can run without you:

| Spike | Status | Result |
|---|---|---|
| 1. Two-hand tracking | Code done, **recording pending** | You need to record at the camera |
| 2. Overlay cost | Done | **NO-GO** (CPU 24% vs 3%, frames 4.7 ms vs 2 ms; RSS passes) |
| 3. Input and text | Clicks done; text nearly done | Clicks **GO** (56 of 56 at 0 px); keymap text exact in terminal, Qt and GTK; browser English needs a rerun; paste fails |
| 4. Script runtime | Done | **GO** (10 of 10 attacks survived, reload 0.7 ms, memory flat) |

Full numbers are in `spikes/RESULTS.md`.

## Assessment vs Reality

| Metric | Predicted (Plan) | Actual |
|---|---|---|
| Complexity | Medium | Medium–High (spike 3 needed a harness and a keymap fix) |
| Files | 4 spike dirs | 4 spike dirs, about 4,300 lines including tests and harness |

## Tasks Completed

| # | Task | Status | Notes |
|---|---|---|---|
| 1 | Two-hand tracking | Code complete; run pending | Recorder (guided reps, landmarks plus video), analysis (pass bar, 1€ grid, facing sign), 11 tests |
| 2 | Overlay cost | Complete | Measured three configurations; NO-GO |
| 3 | Input and text | Mostly complete | Clicks GO; text matrix automated; one browser case and paste outstanding |
| 4 | Script runtime | Complete | GO |

## Validation Results

| Level | Status | Notes |
|---|---|---|
| Static analysis | Pass | `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt` |
| Unit tests | Pass | Rust 26 (1 ignored benchmark), Python 11 |
| Build | Pass | `cargo build --release --workspace` |
| Integration | Partial | Spikes 2, 3 (clicks) and 4 ran on the live Hyprland session; spike 1 needs the camera |
| Edge cases | Pass | 10 Luau attacks, each in its own process |

## Files Changed

| File | Action |
|---|---|
| `.gitignore` | CREATED |
| `spikes/Cargo.toml`, `spikes/Cargo.lock` | CREATED |
| `spikes/two-hand/{pyproject.toml,handtrace.py,record.py,analyze.py,tests/test_analyze.py}` | CREATED |
| `spikes/overlay/{Cargo.toml,src/main.rs,src/replay.rs}` | CREATED |
| `spikes/input/{Cargo.toml,src/main.rs,src/keymap.rs,harness/*}` | CREATED |
| `spikes/luau/{Cargo.toml,src/lib.rs,src/main.rs,src/std.luau,tests/attacks.rs}` | CREATED |
| `spikes/RESULTS.md`, `spikes/results/*` | CREATED |

## Deviations from Plan
- **Spike 2.** Besides the plan's measurement, it adds a half-resolution run (the plan's own fallback) and an all-outputs worst case. Why: so the NO-GO is clearly a cost of the approach, not of one configuration.
- **Spike 3.** Paste uses `wl-copy` and a simulated Ctrl+V. The browser test uses Chromium with a throwaway profile and a local form server. Why: so every app's result can be checked by a script, not by eye.
- **Spike 4.** It adds attacks beyond the plan's four (endless loop or memory bomb inside `pcall`, string doubling, C-stack recursion, error-object bomb, `sleep(0)` loop). It also measures bytecode and a 3,000-line standard library. Why: these are cheap ways a sandbox commonly breaks.

## Issues Encountered
- **Overlay.** The first version used one damage box for both hands and kept requesting frames on idle outputs. Fixed to match the design; CPU fell from 37% to 24%, still NO-GO.
- **Luau executor.** A spawned task could reuse the slot of the task being polled, which made it vanish. Fixed.
- **Keymap.** Characters were placed on key codes that Chromium treats as Backspace or Ctrl. Fixed by using only ordinary character-key codes.
- **Paste.** Ctrl+V through the virtual keyboard has no effect in any app, and `wtype` fails the same way. Not resolved.
- **Text matrix interference.** It is disturbed when another window takes focus. The class guard refuses to type, so no keystrokes went astray, but those cases have to be rerun with the machine idle.

## Next Steps
- [ ] You: record spike 1 (four sessions, about 10 minutes; commands in `spikes/RESULTS.md`), then run `analyze.py`
- [ ] You: rerun the browser text case with the machine idle
- [ ] Decide the spike 2 fallback (GPU path), since ADR 5 is triggered
- [ ] Optional: investigate paste, or accept keymap typing as the only text method
- [ ] Add the results to the design doc's Decision records, then `/code-review`, `/prp-commit`, `/prp-pr`
