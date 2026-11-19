# handOs — PRD (brief)

**Source of truth**: full design doc (concept, PRD, architecture, 7 specs, roadmap, 11 ADRs) — https://claude.ai/artifact/NLXWfvV2Ra2f5kvzXcawBm
This file is the ECC-workflow index of that doc. When they disagree, the design doc wins; update this file.

## Summary
handOs is a scriptable framework for controlling a Linux desktop, and for creating and moving on-screen objects, with bare hands and an ordinary webcam. Rust host does the hard parts (tracking ingest, gesture evaluation, canvas, input, process runtime); behaviour lives in Luau scripts and packages.

## Decisions (accepted 16 Nov 2026, open to revision by spike results)
- **Core**: Rust daemon (`handos`), one static binary, no async runtime, GPU rendering (wgpu on Vulkan, integrated GPU, drawn only when a HandFrame arrives; tiny-skia kept as fallback) + cosmic-text on wlr layer-shell. Revised by spike 2.
- **Scripting**: Luau via mlua (sandbox, optional types, luau-lsp autocomplete). Behaviour only in Luau; settings and permissions in `handos.yaml`.
- **Tracking**: separate tracker process sending landmark-only HandFrames over a Unix socket. Python/MediaPipe tracker first, native Rust (ONNX Runtime) in M5. Adaptive rate: up to 30 fps during gestures, 15 fps otherwise, 5 fps idle.
- **Triggers**: declared in Luau, evaluated in Rust at tracking rate; animation at display rate. Claims make one gesture have one effect.
- **Processes**: Luau coroutine with an owned scope; automatic cleanup and cancellation; reloads swap whole generations atomically.
- **Desktop**: wlr virtual pointer/keyboard first (Hyprland's portal lacks RemoteDesktop). Text into apps by click-then-type through a generated keymap, never automatic (paste failed in spike 3).
- **Speech/LLM**: external JSON-RPC providers; mic capture is host-owned.
- **Packages**: one VM, per-package environment, capability-bound API, default deny.

## User inputs
Laptop webcam facing the user; outputs configurable in YAML; wake by gesture or hotkey; 10–20 h/week (plan assumes 15).

## v1 scenarios
| # | Scenario | Milestone |
|---|---|---|
| S1 | Hands-free pointer: wake, point, click, drag, scroll, sleep | M1 |
| S2 | First custom binding: write, preview, apply, tune in inspector | M2 |
| S3 | Objects on canvas: spawn note, grab, move, type into app, discard | M3 |
| S4 | Voice to five words: palm touch → placed words | M4 |
| S5 | Lasso screenshot → image item | M5 |
| S6 | Install a package from git, review grants, use it | M5 |

## Delivery Milestones
| Milestone | Scope | Exit criteria | Hours | Weeks | Status | Plan |
|---|---|---|---|---|---|---|
| M0 Spikes | Two-hand tracking, overlay cost, input/Persian text, Luau runtime limits | Each spike has a measured result and a go/no-go written into the ADRs | 60 | 1–4 | complete | `.claude/plans/handos-m0-spikes.plan.md` |
| M1 Pointer (v0.1) | Daemon skeleton, HandFrame protocol, Python tracker, filtering, overlay + skeleton, pinch click/drag in Rust, clutch, settings, `doctor`, `status`, trace record/replay | Latency/jitter budgets met or revised; pinch click 95/100; no false activation asleep in 2 h | 90 | 5–10 | in-progress | `.claude/PRPs/plans/handos-m1-pointer.plan.md` |
| M2 Scripting (v0.2) | Luau runtime, `h.hand`/`h.gesture`/`h.on`/`h.mode`, pointer/key fns, claims, generations, `check`/`preview`/`apply`/`inspect`, HUD, type defs | M1 Rust gestures replaced by `std.pointer`; 20 broken scripts never disturb session; budgets enforced | 120 | 11–18 | pending | — |
| M3 Canvas (v0.3) | Items, anchors, signals, springs, shaped text, grab/pluck/drop, targets, text entry, `insert_text`, clipboard, window queries | Pluck 45/50; Persian label renders and types; no visible item lag | 120 | 19–26 | pending | — |
| M4 Flows (v0.4) | Flows, processes, scopes, async calls, audio, providers (2 reference), HTTP, store, `std.voice` | No orphan item / held key after lost hand, provider timeout, or mid-flow reload | 90 | 27–32 | pending | — |
| M5 Native tracker + packages (v0.5) | Rust tracker on ONNX Runtime, packages, manifests, lock file, grants, strokes, lasso, screenshots | Native tracker ≤0.1 hu mean diff vs MediaPipe; memory/CPU budgets met; Python not needed | 180 | 33–44 | pending | — |
| 1.0 | API freeze, docs site, second compositor, 8 h soak, persistent items | All Musts met; all budgets met or revised; API stable | 120 | 45–52 | pending | — |

## Key quality budgets (revised after M0 spikes, 17 Nov 2026)
Motion-to-photon p50 ≤ 90 ms / p95 ≤ 130 ms (at risk: inference measured 27 ms vs 16 ms assumed) · frame→Luau handler p95 ≤ 5 ms · daemon RSS ≤ 40 MB · asleep CPU ≤ 3% core · two-hand CPU ≤ 35% core (tracker + daemon; at risk: Python tracker alone 74% at 15 fps, 111% at 30 fps; Python tracker exempt until M5) · compositor overhead ≤ 6% core · jitter ≤ 3 px RMS · 0 false activations asleep in 8 h · apply ≤ 300 ms.

## Open questions
- Licence (MIT OR Apache-2.0 proposed)
- Name check (GitHub, crates.io, AUR)
- Default gestures
- Pointer hand
- First providers (local assumed)
