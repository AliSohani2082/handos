# M0 spike results

Plan: `.claude/plans/handos-m0-spikes.plan.md`. Raw data: `spikes/results/`.
Machine: Intel i7-8550U (4 cores, 2018), `powersave` governor, Hyprland 0.57.2, eDP-1 and HDMI-A-1 at 1920x1080 @ 60 Hz, scale 1. Rust 1.99, mlua 0.12.2 (Luau 0.740), SCTK 0.21, tiny-skia 0.12, MediaPipe Tasks with Python 3.13 (via uv).

| Spike | Verdict | One line |
|---|---|---|
| 1. Two-hand tracking | **GO** | Palm touch 50 of 50, pluck 49 of 50, no swaps, no dropout over 150 ms; tracker runs at only 15 fps on this CPU |
| 2. Overlay cost | CPU drawing **NO-GO**; GPU drawing **GO at tracking rate** | tiny-skia 24–26%; wgpu 6.8% at 60 fps and 2.4% at 18 fps; Hyprland adds 4–10% either way |
| 3. Input and text | **GO** | 56 of 56 clicks at 0 px; keymap typing exact in all four apps in both languages; paste fails everywhere |
| 4. Script runtime | **GO** | 10 attacks survived; reload 0.7 ms; memory flat |

## Spike 1: two-hand tracking — GO

Recorded in good light only. The dim-light sessions were dropped by decision. Data: `two-hand/data/*.jsonl`, `results/two-hand.json`.

| Session | Reps passed (bar 45) | Recognised | Reps with a swap | Dropouts over 150 ms |
|---|---|---|---|---|
| Palm touch | **50 of 50** | 50 | 0 | 0 |
| Fingertip pluck | **49 of 50** | 49 | 0 | 0 |

- **Overlapping hands are not a problem.** R1, the top risk, did not happen for the palm touch. The reference gesture can stay.
- **Speed is the real finding.** MediaPipe (Python, CPU) needs 30.6 ms per frame at p50 and 48 ms at p95 on this i7-8550U. The recorder therefore ran at 15.4 fps, not the camera's 30. That is twice the 16 ms inference budget and doubles tracking latency. The passes above were achieved at 15 fps.
- **Facing sign.** With palms to the camera, the palm-normal z is +1 for "Left" and −1 for "Right" in every frame. A single formula works once it is multiplied by −1 for right hands.
- **Jitter and 1€ tuning: inconclusive.** During the 4 s hold, hands entered the frame late, so the reported 200 px (palm) and 15 px (pluck) measure movement into position, not tracking noise. To measure properly, record a 20 s still hold with no reps:

  ```bash
  uv run record.py --task palm --light good --reps 0 --hold-seconds 20
  ```

## Spike 2: overlay cost — NO-GO

A click-through layer-shell overlay (empty input region, `Layer::Overlay`) draws two 21-point skeletons with tiny-skia into shm buffers. It uses frame callbacks (60 Hz), one damage box per hand, two buffers repaired from accumulated damage, and viewporter plus fractional-scale support. The skeletons move continuously (synthetic trace). Each run lasted 20 s; figures exclude a 2 s warm-up.

| Configuration | CPU (one core) | Frame p50 / p95 / p99 | RSS max | Bar |
|---|---|---|---|---|
| Hands on one output, other output idle (the design case) | 23.7% | 4.68 / 5.54 / 7.21 ms | 17.8 MiB | NO-GO |
| Same, rendered at half resolution (plan fallback) | 13.8% | 2.56 / 3.09 / 4.08 ms | 6.8 MiB | NO-GO |
| Hands mirrored on both outputs (worst case) | 36.8% | 2.91 / 5.01 / 5.65 ms | 31.9 MiB | NO-GO |

Pass bar: RSS 40 MB or less (passes in every configuration), CPU 3% or less (fails), frames 2 ms or less (fails).

- **Where the time goes.** In a tight loop, tiny-skia needs 0.76 ms to stroke 42 anti-aliased lines and 0.27 ms for the joints, per frame, for two hands. Inside the overlay that rises to about 2–3.5 ms, because the CPU sits at a low clock between frames (`powersave`). Copying into shm costs 0.2–0.4 ms. Wayland requests cost 0.02 ms.
- **What fixing the spike changed.** The first run used one damage box spanning both hands and kept requesting frames on the idle output. Moving to per-hand boxes and stopping frames on idle outputs, as the design specifies, cut CPU from 37% to 24% (one output). It is still 8 times over the bar.
- **Half resolution helps less than expected.** Rendering at half resolution halves memory and roughly halves CPU. tiny-skia's per-path cost does not scale with pixel count, so frame time stays above 2 ms.
- **Idle cost is zero.** An output with nothing changing gets no frame callbacks (0.0 frames/s).
- **Still to check by hand:** that clicks pass through while the overlay runs, and fractional scaling. Both outputs here are scale 1. To test:

  ```bash
  hyprctl keyword monitor HDMI-A-1,preferred,auto,1.25
  # run the overlay
  hyprctl reload
  ```

ADR 5 ("Reopen if spike 2 misses its bar") is triggered. The plan's fallback, a GPU path, is measured below.

### Follow-up: GPU drawing (wgpu, Vulkan) — GO when drawing at tracking rate

`spikes/overlay-gpu` draws the same two moving skeletons on the same click-through overlay (one output, frame callbacks). Every line and joint is one instanced quad, anti-aliased in the fragment shader by distance to the segment, so a frame is a single draw call. The adapter is the Intel UHD 620 (Mesa, Vulkan, Mailbox, premultiplied alpha). `measure.sh` also reports Hyprland's CPU over the same 20 s, because a GPU surface is damaged in full on every frame. With nothing on screen, Hyprland uses 2.6% of a core.

| Configuration | Frames/s | Overlay CPU | CPU per frame p50 / p99 | RSS max | Hyprland CPU | Bar |
|---|---|---|---|---|---|---|
| tiny-skia (spike 2, rerun under `measure.sh`) | 60 | 26.0% | 4.80 / 7.61 ms | 17.7 MiB | 15.0% | NO-GO |
| wgpu, every frame, all Vulkan drivers loaded | 58 | 6.5% | 1.10 / 1.66 ms | **69.0 MiB** | 12.6% | NO-GO |
| wgpu, every frame, Intel driver only | 59 | 6.8% | 1.10 / 1.52 ms | 24.7 MiB | 12.8% | NO-GO (CPU) |
| wgpu, about every 2nd frame | 39 | 4.9% | 1.26 / 1.71 ms | 24.7 MiB | 14.3% | NO-GO (CPU) |
| wgpu, every 4th frame | 18 | **2.4%** | 1.27 / 1.63 ms | 24.6 MiB | 7.3% | **GO** |

- **The GPU cuts the overlay's own CPU by four times** at the same frame rate (26% to 6.8%), and CPU time per frame meets the 2 ms bar in every GPU configuration.
- **Load only the Intel Vulkan driver.** By default the Vulkan loader also loads NVIDIA's driver on this hybrid laptop, which adds 44 MiB and fails the RSS bar. With `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json`, RSS is 24.7 MiB. The daemon has to pick the driver itself (or the integrated GPU's device), not leave it to the loader.
- **3% is met only when frames follow tracking, not the display.** At about 18 frames/s, the rate of the 15 fps tracker from spike 1, the overlay uses 2.4%. Drawing at 60 fps only interpolates between tracker frames, at roughly three times the cost.
- **The compositor cost is as large as the overlay's own and is not counted by the 3% bar.** Hyprland uses about 10% of a core above idle to composite a full-screen overlay at 60 fps, and about 4.6% at 18 fps, whichever renderer draws it. The PRD's "two-hand CPU ≤ 35% core (tracker + daemon)" budget leaves this out.
- **GL did not work.** wgpu's GL backend found no adapter compatible with the surface (it needs the Wayland display handle at instance creation, which this spike does not pass). It was not pursued, because Vulkan works.

**Recommendation for M1 (ADR 5):** draw with wgpu on Vulkan, explicitly on the integrated GPU; draw a frame only when a new HandFrame arrives (or a visible animation is running); and show nothing (no commits) when no hand is shown. Add compositor CPU to the overlay budget, or state that it is excluded.

Commands:

```bash
spikes/overlay-gpu/measure.sh sleep 20      # Hyprland's idle CPU
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json \
  spikes/overlay-gpu/measure.sh spikes/target/release/overlay-gpu --synthetic --seconds 20 [--every 4] [--json out.json]
```

Commands:

```bash
cargo run --release -p overlay-spike -- --synthetic --seconds 20 [--render-scale 0.5] [--all-outputs] [--trace two-hand/data/<file>.jsonl]
cargo test --release -p overlay-spike -- --ignored --nocapture   # tiny-skia microbenchmark
```

## Spike 3: input and text

### Clicks — GO

`input-spike clicks` covers every output with a target surface and creates a wlr virtual pointer bound to each output (manager v2, `create_virtual_pointer_with_output`). It clicks a 6x4 grid plus the four corners 3 px in. A click is sent only after the pointer is confirmed over the spike's own surface.

Result: **56 of 56 clicks landed with 0.00 px error** on both outputs.

Hyprland's permission system (`ecosystem:enforce_permissions`) is off on this machine, so neither protocol prompted.

### Text

`spikes/input/harness/text_matrix.sh` launches each app, waits until Hyprland reports its window as active, and runs `input-spike type`. That command refuses to type unless the active window class matches the app under test. The script then compares what the app received with what was sent:
- English: `The quick brown fox jumps over the lazy dog, 42 times!`
- Persian: `سلام دنیا، من می‌خواهم این متن را بنویسم.`, which includes a zero-width non-joiner and the Persian comma

Results from all runs. Runs where focus moved to another window part-way through (the guard refused to type) are left out.

| App | Keymap, English | Keymap, Persian | Paste |
|---|---|---|---|
| Terminal (kitty) | exact | exact | **fails** (empty) |
| Qt (kdialog) | exact (3 of 3 runs) | exact (3 of 3 runs) | **fails** (empty) |
| GTK 4 entry | exact | exact | **fails** (empty) |
| Browser (Chromium form field) | exact (rerun with the machine idle)* | exact, after the fix below | **fails** (empty) |

*Before the fix it dropped the comma. The first run after the fix was disturbed by focus changes; the rerun with nobody at the keyboard was exact for both sentences (`results/input-text-browser.jsonl`).

**Bug found and fixed: key codes have meanings of their own.** The first keymap gave characters codes 1, 2, 3… in order. Chromium treats some evdev codes by their physical meaning, whatever keysym the keymap puts there. «ت» landed on code 14 (Backspace) and deleted the «م» before it. The comma landed on 29 (Left Ctrl) and vanished. `wtype` numbers keys the same way. Characters now go only on ordinary character-key codes (47 of them, plus Space), and Return, Control and Shift sit on their real codes. Text with more distinct characters is split across keymaps.

**Paste is not working yet.** `wl-copy` sets the clipboard correctly (`wl-paste` reads it back). Ctrl+V does nothing in any of the four apps, including with real Control key presses on the correct code. A Ctrl+A test with `wtype` lost the keys sent after the modifier. Paste was the plan's fallback, not the main path. On the evidence so far, keymap typing is the method that works, and paste needs its own investigation.

**Verdict: GO.** The pass bar is "both sentences correct in all four applications by at least one method". Keymap typing meets it in all four. Paste, the plan's fallback, does not work yet and needs its own investigation if it is wanted.

```bash
spikes/input/harness/text_matrix.sh   # full matrix, about 3 minutes; run with nobody at the keyboard
```

## Spike 4: script runtime — GO

mlua 0.12.2 with Luau, sandbox on, 16 MiB VM limit, 50 ms hard budget per turn (interrupt). A process is a Luau function run as an mlua async task on a hand-written single-threaded executor (about 200 lines, no async runtime). `sleep` uses a timer heap, `wait(event)` waits for a named event, and `http_get` runs on at most two lazily started worker threads.

**Process model.** The demo passes in 161 ms with a longest turn of 0.31 ms. It covers sleep, HTTP on a worker, waiting for an event another task emits, sleeping inside `pcall`, scoped cleanup running on error, and `std.wait_for` timing out.

**Attacks.** Each runs in its own process, so a host abort would show up as a signal. All ended, and the VM stayed usable afterwards:

| Attack | Ended by | Time |
|---|---|---|
| `while true do end` | 50 ms budget | 50.1 ms |
| endless loop inside `pcall`, inside a loop | budget (pcall cannot swallow it) | 50.1 ms |
| memory bomb (1 KiB strings) | memory limit | 29.9 ms |
| memory bomb inside `pcall`, repeated | budget | 50.1 ms |
| string doubling | memory limit | 5.8 ms |
| deep recursion | stack overflow | 9.7 ms |
| deep recursion with memory already at the limit | memory limit | 30.3 ms |
| `pcall` recursion (C stack) | unwound by itself (Luau's C-call limit) | 3.3 ms |
| error object of 10⁷ entries | memory limit | 35.6 ms |
| `sleep(0)` loop | budget | 50.1 ms |

**1,000 reloads per variant.** Each reload drops the VM, then builds a fresh one with the standard library and an init script:

| Standard library | p50 | p95 | max | RSS growth after reload 100 |
|---|---|---|---|---|
| 143 lines, source | 0.71 ms | 0.93 ms | 1.53 ms | +100 KiB |
| 143 lines, bytecode | 0.58 ms | 0.66 ms | 1.13 ms | 0 |
| 3,143 lines, source | 9.01 ms | 12.45 ms | 20.33 ms | 0 |
| 3,143 lines, bytecode | 0.95 ms | 1.35 ms | 2.00 ms | 0 |

Bar: 50 ms or less, memory flat. Both met with a large margin; precompiled bytecode keeps reloads near 1 ms even for a large standard library.

**Findings for M2:**
- `sleep(0)` does not yield, because its timer is already due, so a `sleep(0)` loop is one long turn until the budget stops it. A real yield-to-scheduler is needed.
- Task slots must not be reused while their task is being polled. That bug made spawned tasks vanish in the first run; it is fixed in the spike.
- Interrupt errors are not swallowed by `pcall`: the next check after the catch fires again.

Commands:

```bash
cargo run --release -p luau-spike -- report --json results/luau-report.json
cargo test --release -p luau-spike
```
