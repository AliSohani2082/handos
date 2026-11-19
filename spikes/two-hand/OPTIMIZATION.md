# live.py optimization log

Goal: lower the CPU and RAM that `live.py` (webcam → MediaPipe → stabilizer → overlay window) needs,
both while it is idle (no hands in view) and while it tracks hands. Each optimization is one commit,
measured with `bench.py`, and logged below. The numbers in **Current state** should go down with each entry.

Machine for all runs unless noted: Intel i7-8550U (4 cores / 8 threads), `powersave` governor,
Hyprland, camera 640x480 MJPG at 30 fps, Python 3.13, MediaPipe 1.2.3, OpenCV 5.0.0.

## How to measure

```bash
cd spikes/two-hand
uv run bench.py --label "<what changed>"          # 5 s warm-up, 20 s IDLE, 3 s get-ready, 20 s HANDS
uv run bench.py --report ../results/perf/<new>.json --against ../results/perf/<previous>.json
```

`bench.py` runs the real `live.py` loop through a timing probe (`live.main(probe=...)`), so it always
measures the current code. The banner at the bottom of the window tells you what to do:

1. **IDLE**: keep your hands out of view. Your face may be in view.
2. **HANDS**: hold both hands up and move them a little, the way you use the app.

Only 1 s windows where at least 90% of frames match the phase count (0 hands for IDLE, 2 for HANDS).
If a hand slips out, the bar turns red and the run waits. Results are saved to
`spikes/results/perf/<time>-<sha>.json`, and a Markdown summary is printed for this file.

Rules for a fair comparison:
- Commit the change first, then measure, so the JSON records a clean SHA (`dirty: false`).
- Close other heavy apps. Use the same light, the same seat and the same governor.
- Nothing else may hold the camera. A running `live.py` makes `bench.py` fail with "cannot open camera".
- Live input is noisy. Treat a change under the noise band (measured in the baseline section) as "no change".

## What is measured

| Metric | Meaning |
|---|---|
| CPU, % of one core | Whole-process CPU time / wall time. 100 = one core busy. Divide by 8 for share of the machine. |
| CPU by main-loop stage | Main-thread CPU spent inside each stage of the loop (`thread_time`), with wall ms per frame. Wall is much larger than CPU when a stage waits, e.g. `camera` waits for the next frame and `inference` waits for MediaPipe's worker threads. |
| CPU by thread | Per-thread CPU from `/proc/self/task`, grouped by thread name. Inference workers and BLAS pools show up here, not in the stage table. Many threads are just named `python`, so they are grouped as "unnamed threads". |
| RSS by owner | Resident memory from `/proc/self/smaps` at the end of each phase, bucketed by owner: anonymous memory (numpy, TFLite tensors, malloc arenas, thread stacks), `[heap]`, each wheel's native libraries (`cv2`, `mediapipe`, `numpy`), the Python runtime and system libs. |
| Frame rate, frame time | Loop iterations per second and the time between iterations (mean, p95). |
| Inference | Wall time of `detect_for_video` per frame (mean, p95). |
| Threads, context switches | Thread count and voluntary context switches per second. |

Stages, in loop order: `camera` (`cap.read`, MJPG decode), `preprocess` (flip, BGR→RGB, `mp.Image`),
`inference`, `stabilizer` (adapter + `HandStabilizer.update`), `angles` (pitch/roll/yaw),
`overlay` (skeleton, stats panel), `graph` (angle history strip), `compose` (stack + resize),
`display` (`imshow` + `waitKey`). "Main thread, outside stages" is loop glue. "Bench hooks" is the
cost of measuring itself; it is excluded from the stage numbers but included in process CPU.

## Current state

| | IDLE CPU % | HANDS CPU % | IDLE RSS MiB | HANDS RSS MiB | HANDS fps | Inference ms (hands) |
|---|---|---|---|---|---|---|
| Baseline | _pending_ | | | | | |
| Latest | _pending_ | | | | | |

## Baseline breakdown

_Pending: the first `uv run bench.py --label baseline` run goes here (full summary from bench.py)._

## Optimization log

One entry per optimization commit, newest last. Each entry gives the commit, a short description of
the change, before → after for the headline numbers, and the result JSON.

| # | Commit | Optimization | IDLE CPU % | HANDS CPU % | RSS MiB (hands) | Result |
|---|---|---|---|---|---|---|
| 0 | _baseline_ | No change: reference run | | | | |

## Backlog (candidates from reading the code, not yet measured)

Ordered by expected payoff. Pick the stage that the baseline breakdown shows as largest first; the
order may change once real numbers exist.

1. **Idle detection rate.** With no hands tracked, MediaPipe runs the palm detector on every frame,
   so IDLE costs nearly as much as HANDS. Run inference on every 2nd or 3rd frame while nothing is
   tracked, and return to every frame once a hand appears. Likely the biggest IDLE win.
2. **Thread pools.** Importing numpy and cv2 starts about 15 threads. numpy's OpenBLAS pool (7 threads)
   spins for about 2 s after startup and is never used by the loop. Set `OPENBLAS_NUM_THREADS=1` and
   possibly `cv2.setNumThreads(1)` before import. This saves thread stacks and startup CPU. Check that
   inference does not slow down.
3. **Graph strip.** `graph()` allocates a new 640x300 image every frame and draws up to
   2 hands x 3 axes x 300 samples as single `cv2.line` calls from Python, about 1,800 calls per frame
   with hands in view. Use one `cv2.polylines` call per series, or redraw only every Nth frame.
4. **Window scaling.** `compose` stacks the frame and graph and resizes 640x780 to 1024x1248 on the CPU
   every frame. The window is already `WINDOW_NORMAL`: show the native size and let the window scale it
   (`cv2.resizeWindow` once), or drop `--scale` to 1.
5. **Per-frame /proc reads in the panel.** `rss_mib()` and `os.getloadavg()` run every frame. Refresh
   them once a second, like CPU already is.
6. **Preprocess copies.** `flip` and `cvtColor` each allocate a new full frame. Reuse buffers through
   `dst=`, or convert once and flip the RGB copy in place.
7. **Camera format and size.** MJPG decode runs on the main thread inside `cap.read`. Try YUYV at
   640x480, or a smaller capture size, since the model input is only 192–224 px. Watch tracking
   quality, not just CPU.
8. **Adapter.** `from_mediapipe` builds landmark arrays with Python list comprehensions (2 x 21 points
   per hand). This is small, so only touch it if the stabilizer stage shows up.
