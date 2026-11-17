# Plan: M0 extra spike — GPU overlay (ADR 5 fallback)

**Source PRD**: `.claude/prds/handos.prd.md`
**Selected Milestone**: M0 Spikes (follow-up to spike 2, which was NO-GO on CPU)
**Complexity**: Small

## Summary
Spike 2 drew the skeletons with tiny-skia on the CPU and used 24% of a core, against a 3% bar.
This spike draws the same two moving skeletons on the same click-through layer-shell overlay with wgpu, and measures CPU, frame time and RSS.
It also measures the compositor's CPU for both spikes, because a GPU surface is damaged in full every frame and Hyprland may pick up the cost.

## Tasks
1. `spikes/overlay-gpu`: a layer-shell overlay with an empty input region on the first output, driven by frame callbacks.
2. Each line and each joint is one instanced quad, anti-aliased with a signed-distance function (one draw call per frame).
3. It reuses `overlay/src/replay.rs` (synthetic and recorded traces) and the same CPU/RSS measurement and pass bar.
4. Flags `--backend vulkan|gl`, `--adapter low|high`, `--seconds`, `--json`.
5. `spikes/overlay-gpu/measure.sh`: runs any command and reports Hyprland's CPU over the same window.
6. Run the synthetic case with both backends and rerun the CPU spike under the same script, then write the results into `spikes/RESULTS.md`.

## Validation
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --release --workspace
spikes/overlay-gpu/measure.sh cargo run --release -p overlay-gpu -- --synthetic --seconds 20
```

## Pass bar (same as spike 2)
RSS ≤ 40 MB, CPU ≤ 3% of a core while animating, CPU time per frame ≤ 2 ms (p99).

## Risks
| Risk | Likelihood | Mitigation |
|---|---|---|
| Vulkan driver RSS pushes the daemon over 40 MB | Medium | Measure the GL backend too |
| A full-surface present moves the cost into Hyprland | Medium | Measure Hyprland's CPU as well |
| The hybrid laptop picks the NVIDIA GPU | Medium | Ask for a low-power adapter and report its name |
