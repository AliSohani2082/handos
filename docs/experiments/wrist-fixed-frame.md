# Experiment: the wrist-fixed hand frame

## Status

- **Parked.** Not part of the main plan.
- **Arm C of spike 5** (learned-pose classifier input), competing with the main plan's arms.
- **Owner:** Ali.
- **History:** chosen by ADR 12 (20 Nov 2026) as the input for classifying learned hand poses;
  superseded by ADR 13 (26 Nov 2026), which follows two Google papers (virtual-keypoint
  normalization and a pretrained hand embedder) and lets spike 5 decide the classifier input.
  Ali judged the frame an inaccurate representation of a gesture, but wants it kept and measured
  rather than deleted.
- Design doc: <https://claude.ai/artifact/NLXWfvV2Ra2f5kvzXcawBm>. Its main spec no longer
  holds ADR 12's text. This file is now the record of what ADR 12 defined.

## Hypothesis

A pose is a finger shape. Expressed in a frame fixed to the wrist, the same shape gives the same
21 points however the arm and wrist are turned. So a small MLP trained only on upright samples
should still recognise the pose at any angle, without recording each pose at many angles.

## Definition

As ADR 12 defined it:

- **Input:** the 21 MediaPipe **world** landmarks, in a frame fixed to the wrist.
  - Origin: landmark 0 (wrist).
  - y: towards landmark 9 (middle knuckle).
  - z: the palm normal (p5 − p0) × (p17 − p0).
  - x: completes a right-handed frame.
  - Distances in world hand units (hu): the world distance between landmarks 5 and 17.
- **Left hands** are mirrored in x, so one recording serves both hands.
- **Orientation is dropped.** `facing` and `roll` read it separately.
- **Model:** an MLP, 63 → 64 → 64 → (one output per pose + `none`), softmax. Retrained from
  scratch whenever the samples change, with a fixed seed.
- **Samples:** one `.hgs` file per pose, about 60 frames of local landmarks. `none` holds
  in-between hand movement.

## What exists

| What | Where | State |
|---|---|---|
| Arm C module: the frame, the MLP and the spike 5 `Arm` interface | `research/learned-poses/src/learned_poses/arms/c_wrist_fixed.py`, branch `exp/wrist-fixed-frame` | Written 27 Nov 2026 from the definition above. Lands on `main` once the spike 5 harness exists (see below). |
| Its tests: the same features under any 3D rotation, shift and scale; left-hand mirroring; column order; seed determinism | `research/learned-poses/tests/arms/test_c_wrist_fixed.py`, same branch | Pass. |
| A Rust wrist-fixed frame, `handos_hands::local` | `crates/handos-hands/src/local.rs`, M1 Phase D work (`feat/m1-pointer-d`) | **Not this experiment's code, and it stays in Phase D.** Same axes, but built from *image* landmarks with their depth, not world landmarks. It measures pinches (`PinchSpace::Local`, the M1 default: 49 of 50 recorded clicks) and draws the corner widgets. ADR 13 replaced only the classifier input, so this use is unaffected. |
| Pose classifier in Rust (`handos-learn`, `.hgs` files, `hand.local` in scripts) | none | Never written. ADR 12 was superseded first. |
| The 20 Nov simulation script (Python, sklearn) | none | Never saved. The table below is the only record of its results. |

Nothing wrist-fixed reached `main` as product code, so nothing was removed from it.

## Simulation results

20 Nov 2026, synthetic hands: 10 poses (Spider-Man, rock and L among them), 20 upright samples
per pose, 4 mm x/y noise and 10 mm depth noise. Accuracy per frame:

| Classifier input | Upright | Wide desk angles | Any angle |
|---|---|---|---|
| Raw landmarks, MLP | 100% | 62.8% | 21.2% |
| Wrist-fixed frame, MLP | 99.7% | 99.8% | 99.9% |
| Joint angles, nearest prototype | 90.1% | 92.0% | 90.7% |

- With a `none` class, the MLP kept 99.0% of real poses and accepted 28.5% of transitional
  shapes per frame.
- Accuracy fell to 85.3% at 20 mm and 69.9% at 30 mm of depth noise.
- Retraining took 0.34 s for 300 samples (sklearn, one core).

These are synthetic hands. The script is lost, so the numbers can't be reproduced. Spike 5
measures the same arm on real recordings.

## Known weaknesses

- **It rests on world landmarks**, and those miss fine finger detail on Ali's webcam. The M1
  Phase C report (`.claude/PRPs/reports/handos-m1-pointer-phase-c-report.md`) found a closed pinch
  reads 0.49 hu (median) in world space against 0.10 hu in image space. Poses that differ by a
  fingertip gap may collapse together.
- **It drops orientation**, so thumb up and thumb down give the same features. Any pose told
  apart only by its direction needs `facing` or `roll` on top.
- **Depth noise hurts it quickly:** 85.3% at 20 mm and 69.9% at 30 mm in simulation. Real webcam
  depth is an estimate.
- **Transitional shapes:** 28.5% of in-between frames were accepted as a pose, so it needs
  temporal smoothing or a hold time before firing.

## How it runs in spike 5

Spike 5 lives in `research/learned-poses/`, a uv project being built by another agent. Each arm
is a module in `research/learned-poses/src/learned_poses/arms/` with the agreed interface:

```python
class Arm(Protocol):
    name: str  # "C_wrist_fixed"
    def featurize(self, s: HandSample) -> np.ndarray: ...
    def fit(self, X: np.ndarray, y: np.ndarray, *, seed: int) -> None: ...
    def predict_proba(self, X: np.ndarray) -> np.ndarray: ...
```

Arm C (`c_wrist_fixed.py`, class `WristFixedArm`, alias `Arm`):

- `featurize` takes `HandSample.world`, mirrors left hands, builds the frame and returns 63
  floats in hu. It ignores `image`. If a frame has no world landmarks (`world is None`) or a
  degenerate palm, it raises `ValueError`. The harness should count those frames as arm C
  misses, because the product would have no answer for them either.
- `fit` trains sklearn's `MLPClassifier(hidden_layer_sizes=(64, 64), random_state=seed)` from
  scratch.
- `predict_proba` columns: poses in the order they first appear in `y`, then `none`. The
  `none` column is always there, all zeros if training had no `none` samples. `arm.classes_`
  lists the order.

**Landing:** the harness didn't exist on 27 Nov 2026, so the module and its test live only on
`exp/wrist-fixed-frame`. Once `research/learned-poses/` is on `main`, they go to `main` in a
small PR. That's research code, so it's allowed on `main`. Product code stays on the branch.
Until then, run the tests with:

```sh
cd research/learned-poses
PYTHONPATH=src uv run --no-project --python 3.12 \
  --with numpy --with scikit-learn --with pytest pytest -q tests
```

**What spike 5 must report for this arm**, to judge the revive criteria:

- accuracy overall, on the same splits as the other arms;
- accuracy on hands aimed at the camera (fingers towards the lens), where world depth is weakest
  and the frame should help most;
- `none` acceptance of transitional frames;
- the share of frames arm C could not featurize.

## Revive criteria

The frame returns to the main plan only if, in spike 5, it either:

1. beats arm B overall, or
2. beats arm B on hands aimed at the camera by **5 points or more**.

Then it needs a new ADR that supersedes ADR 13. Without one, it stays parked.

## Next steps if revived

1. Write the ADR that supersedes ADR 13, citing the spike 5 numbers.
2. Choose the landmark source. Test the image-landmark-plus-depth frame from
   `handos_hands::local` (already used for pinches) as a variant arm against world landmarks,
   since world landmarks lose fingertip gaps on Ali's webcam.
3. Add orientation (`facing`, `roll`) as features or a second stage, so thumb up and thumb down
   are told apart.
4. Build the product path: a `handos-learn` crate (MLP inference in Rust, the `.hgs` sample
   format, a recorder, retraining on sample change with a fixed seed), `hand.local` in scripts,
   and a hold time or smoothing against transitional shapes.
5. Collect real samples per pose and check the 99% upright and any-angle simulation numbers on
   them.
