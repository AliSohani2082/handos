# Implementation Report: Hand stabilizer + live tracker view

## Summary
`spikes/two-hand/stabilizer.py` turns raw MediaPipe HandLandmarker output into steady, identity-tracked
hands. An adapter (`from_mediapipe`) maps results to tracker-agnostic `HandObs`; a stage pipeline
(`HandStabilizer`) runs Associate (track identity by palm position, 10-frame dropout grace) →
StickyLabel (label fixed per track; flips only after 30 confident disagreeing frames; two tracks never
share a label) → OneEuroStage (One Euro filter on image and world landmarks). Output is `StableHand`,
the type the main program should consume instead of raw landmarker output.

`spikes/two-hand/live.py` is a live webcam view with tracking stats, process CPU/RSS, per-hand
pitch/roll/yaw (full-range yaw), palm/back/edge readout, a live angle graph, and an `s` key that
toggles stabilized vs raw data.

## Assessment vs Reality

| Metric | Predicted (Plan) | Actual |
|---|---|---|
| Complexity | n/a (no plan file; spec from conversation) | Small |
| Files Changed | n/a | 3 created |

## Tasks Completed

| # | Task | Status | Notes |
|---|---|---|---|
| 1 | Adapter + `HandObs`/`StableHand` types | done | |
| 2 | Associate stage (track identity) | done | greedy nearest palm centre, max jump 0.25 |
| 3 | StickyLabel stage | done | |
| 4 | OneEuro stage | done | lm (1.5, 8.0), world (1.5, 15.0) |
| 5 | `s` toggle in live.py | done | stabilizer always runs so tracks stay warm |

## Validation Results

| Level | Status | Notes |
|---|---|---|
| Static Analysis | Pass | `uvx ruff check --line-length 120` clean |
| Unit Tests | Pass | 22 passed (5 new) |
| Build | n/a | Python |
| Integration | Pass | replay of both recorded sessions (4,818 frames): 2 stable tracks each, 0 label flips |
| Edge Cases | Pass | label flicker, sustained flip, shared label, dropout (unit tests) |

### Jitter on real data (hold phase, mean per-frame change)

| Session / hand | Index tip px | pitch / roll / yaw deg |
|---|---|---|
| palm Left | 7.98 → 7.40 | 2.06/1.37/1.58 → 1.29/1.17/0.92 |
| palm Right | 6.36 → 5.83 | 2.02/1.43/1.20 → 0.73/1.27/0.46 |
| pluck Right | 1.02 → 0.61 | 1.44/0.50/0.83 → 0.69/0.32/0.30 |
| pluck Left | 0.69 → 0.50 | 1.18/0.33/0.68 → 0.47/0.22/0.25 |

Gentler settings (lm 1.0/0.5, world 1.0/2.0) cut jitter ~15% further; lag was not measured, so the
defaults are kept.

## Deviations from Plan
No plan file existed; the feature was specified in conversation and implemented before this command.

## Issues Encountered
- The recordings contain no raw label flips (palms faced the camera), so the edge-on flicker fix is
  covered by unit tests only, not by real data.

## Tests Written

| Test File | Tests | Coverage |
|---|---|---|
| `spikes/two-hand/tests/test_stabilizer.py` | 5 | flicker hold, sustained flip, two-hand identity, dropout, One Euro jitter |

## Next Steps
- [ ] Check on camera: palm/back sign and edge-on label stability (`uv run live.py`, press `s`)
- [ ] Measure filter lag before tuning One Euro further
- [ ] Possible stage: entry-side prior for a track's first label
- [ ] Port the stage pipeline to the Rust daemon in M1
