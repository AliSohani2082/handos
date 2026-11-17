"""Spike 1 analysis: dropouts, handedness swaps, gesture recognition, 1€ tuning, facing sign.

Usage: uv run analyze.py data/*.jsonl [--json out.json] [--reps]

Pass bar (palm task): 45 of 50 palm touches recognised with no swap and no dropout longer than 150 ms.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from dataclasses import asdict, dataclass
from itertools import permutations
from pathlib import Path

from handtrace import (INDEX_TIP, THUMB_TIP, TIPS, WRIST, Frame, Hand, Trace, dist, facing_z, load,
                       palm_centre, palm_size)

# Pixels for jitter and lag: the whole camera image mapped onto one 1080p output.
SCREEN_W, SCREEN_H = 1920, 1080
MAX_DROPOUT_MS = 150.0
PASS_FRACTION = 0.9  # 45 of 50


@dataclass
class Params:
    touch_ratio: float = 0.6     # index tip within this many palm sizes of the other palm's centre
    pinch_ratio: float = 0.3     # thumb tip to index tip, in palm sizes of the pinching hand
    near_ratio: float = 0.5      # pinch point to the nearest fingertip of the other hand, in its palm sizes
    min_frames: int = 2          # consecutive frames the gesture must hold to count


# ---------------------------------------------------------------- filtering

class OneEuro:
    """1€ filter (Casiez et al. 2012) on a vector of floats."""

    def __init__(self, min_cutoff: float, beta: float, d_cutoff: float = 1.0):
        self.min_cutoff, self.beta, self.d_cutoff = min_cutoff, beta, d_cutoff
        self.t: float | None = None
        self.x: list[float] = []
        self.dx: list[float] = []

    @staticmethod
    def _alpha(cutoff: float, dt: float) -> float:
        tau = 1.0 / (2 * math.pi * cutoff)
        return 1.0 / (1.0 + tau / dt)

    def reset(self) -> None:
        self.t = None

    def __call__(self, t_s: float, x: list[float]) -> list[float]:
        if self.t is None or t_s <= self.t:
            self.t, self.x, self.dx = t_s, list(x), [0.0] * len(x)
            return list(x)
        dt = t_s - self.t
        a_d = self._alpha(self.d_cutoff, dt)
        dx = [a_d * (xi - pi) / dt + (1 - a_d) * di for xi, pi, di in zip(x, self.x, self.dx)]
        speed = math.hypot(*dx)
        a = self._alpha(self.min_cutoff + self.beta * speed, dt)
        self.x = [a * xi + (1 - a) * pi for xi, pi in zip(x, self.x)]
        self.dx, self.t = dx, t_s
        return list(self.x)


# ---------------------------------------------------------------- identity

def assign_tracks(frames: list[Frame]) -> tuple[list[dict[int, Hand]], list[bool]]:
    """Follow up to two hands by wrist position.

    Returns, per frame, {track_id: hand} and whether a handedness swap happened in that frame:
    a track's label changed, or both hands carried the same label.
    """
    tracks: dict[int, tuple[tuple[float, float], str]] = {}
    out: list[dict[int, Hand]] = []
    swaps: list[bool] = []
    for f in frames:
        hands = f.hands[:2]
        free_ids = [i for i in (0, 1) if i not in tracks]
        candidates = list(tracks) + free_ids
        best, best_cost = None, math.inf
        for perm in permutations(candidates, len(hands)):
            cost = 0.0
            for h, tid in zip(hands, perm):
                cost += dist(h.lm[WRIST], tracks[tid][0]) if tid in tracks else 10.0
            if cost < best_cost:
                best, best_cost = perm, cost
        assigned: dict[int, Hand] = {}
        swapped = len(hands) == 2 and hands[0].label == hands[1].label
        for h, tid in zip(hands, best or ()):
            if tid in tracks and tracks[tid][1] != h.label:
                swapped = True
            tracks[tid] = ((h.lm[WRIST][0], h.lm[WRIST][1]), h.label)
            assigned[tid] = h
        out.append(assigned)
        swaps.append(swapped)
    return out, swaps


def longest_dropout_ms(frames: list[Frame], end_ms: float) -> float:
    """Longest time with fewer than two hands, from the first frame missing one to the first frame with both."""
    longest, gap_start = 0.0, None
    for f in frames:
        if len(f.hands) < 2:
            if gap_start is None:
                gap_start = f.t_ms
        elif gap_start is not None:
            longest = max(longest, f.t_ms - gap_start)
            gap_start = None
    if gap_start is not None:
        longest = max(longest, end_ms - gap_start)
    return longest


# ---------------------------------------------------------------- gestures

def palm_touch(hands: list[Hand], p: Params) -> bool:
    if len(hands) < 2:
        return False
    a, b = hands[0], hands[1]
    for x, y in ((a, b), (b, a)):
        if dist(x.lm[INDEX_TIP], palm_centre(y.lm)) < p.touch_ratio * palm_size(y.lm):
            return True
    return False


def pluck(hands: list[Hand], p: Params) -> bool:
    if len(hands) < 2:
        return False
    a, b = hands[0], hands[1]
    for x, y in ((a, b), (b, a)):
        if dist(x.lm[THUMB_TIP], x.lm[INDEX_TIP]) >= p.pinch_ratio * palm_size(x.lm):
            continue
        mid = ((x.lm[THUMB_TIP][0] + x.lm[INDEX_TIP][0]) / 2, (x.lm[THUMB_TIP][1] + x.lm[INDEX_TIP][1]) / 2)
        if min(dist(mid, y.lm[t]) for t in TIPS) < p.near_ratio * palm_size(y.lm):
            return True
    return False


def sustained(flags: list[bool], n: int) -> bool:
    run = 0
    for f in flags:
        run = run + 1 if f else 0
        if run >= n:
            return True
    return False


@dataclass
class RepResult:
    i: int
    recognised: bool
    swaps: int
    longest_dropout_ms: float
    two_hand_fraction: float
    passed: bool


def evaluate_reps(trace: Trace, p: Params) -> list[RepResult]:
    detect = palm_touch if trace.header.get("task") == "palm" else pluck
    _, swap_flags = assign_tracks(trace.frames)
    results = []
    for rep in trace.reps:
        idx = [k for k, f in enumerate(trace.frames) if rep.start_ms <= f.t_ms < rep.end_ms]
        frames = [trace.frames[k] for k in idx]
        if not frames:
            results.append(RepResult(rep.i, False, 0, rep.end_ms - rep.start_ms, 0.0, False))
            continue
        recognised = sustained([detect(f.hands, p) for f in frames], p.min_frames)
        swaps = sum(swap_flags[k] for k in idx)
        drop = longest_dropout_ms(frames, rep.end_ms)
        two = sum(len(f.hands) >= 2 for f in frames) / len(frames)
        results.append(RepResult(rep.i, recognised, swaps, drop, two,
                                 recognised and swaps == 0 and drop <= MAX_DROPOUT_MS))
    return results


# ---------------------------------------------------------------- 1€ tuning

def _tip_px(h: Hand) -> list[float]:
    return [h.lm[INDEX_TIP][0] * SCREEN_W, h.lm[INDEX_TIP][1] * SCREEN_H]


def filter_series(trace: Trace, min_cutoff: float, beta: float) -> dict[int, list[tuple[float, list[float], list[float]]]]:
    """Per track: (t_ms, raw_px, filtered_px) for the index tip. The filter resets after a 100 ms gap."""
    tracks, _ = assign_tracks(trace.frames)
    filters: dict[int, OneEuro] = {}
    last_t: dict[int, float] = {}
    series: dict[int, list] = {}
    for f, assigned in zip(trace.frames, tracks):
        for tid, h in assigned.items():
            flt = filters.setdefault(tid, OneEuro(min_cutoff, beta))
            if tid in last_t and f.t_ms - last_t[tid] > 100:
                flt.reset()
            last_t[tid] = f.t_ms
            raw = _tip_px(h)
            series.setdefault(tid, []).append((f.t_ms, raw, flt(f.t_ms / 1000.0, raw)))
    return series


def rms_deviation(points: list[list[float]]) -> float:
    if len(points) < 2:
        return math.nan
    mx = statistics.fmean(p[0] for p in points)
    my = statistics.fmean(p[1] for p in points)
    return math.sqrt(statistics.fmean((p[0] - mx) ** 2 + (p[1] - my) ** 2 for p in points))


def tune_filter(trace: Trace) -> list[dict]:
    """Jitter while still (hold phase) against lag while moving (rep windows), over a parameter grid."""
    if trace.hold is None:
        return []
    h0, h1 = trace.hold
    rows = []
    for min_cutoff in (0.3, 0.5, 1.0, 2.0, 4.0):
        for beta in (0.0, 0.001, 0.005, 0.02, 0.1):
            series = filter_series(trace, min_cutoff, beta)
            jitters, lags = [], []
            for pts in series.values():
                still = [flt for t, _, flt in pts if h0 + 500 <= t < h1]  # skip the settle-in
                if len(still) > 10:
                    jitters.append(rms_deviation(still))
                moving = [math.dist(raw, flt) for t, raw, flt in pts
                          if any(r.start_ms <= t < r.end_ms for r in trace.reps)]
                lags.extend(moving)
            rows.append({
                "min_cutoff": min_cutoff, "beta": beta,
                "jitter_px": max(jitters) if jitters else math.nan,
                "lag_px": statistics.fmean(lags) if lags else math.nan,
            })
    return rows


def raw_jitter(trace: Trace) -> float:
    if trace.hold is None:
        return math.nan
    h0, h1 = trace.hold
    series = filter_series(trace, 1.0, 0.0)
    vals = [rms_deviation([raw for t, raw, _ in pts if h0 + 500 <= t < h1]) for pts in series.values()]
    vals = [v for v in vals if not math.isnan(v)]
    return max(vals) if vals else math.nan


def facing_by_label(trace: Trace) -> dict[str, float]:
    """Mean sign of the palm-normal z per handedness label during the hold (palms to camera)."""
    if trace.hold is None:
        return {}
    h0, h1 = trace.hold
    signs: dict[str, list[float]] = {}
    for f in trace.frames:
        if h0 <= f.t_ms < h1:
            for h in f.hands:
                signs.setdefault(h.label, []).append(math.copysign(1.0, facing_z(h.wl)))
    return {k: statistics.fmean(v) for k, v in signs.items()}


# ---------------------------------------------------------------- report

def pct(values: list[float], q: float) -> float:
    if not values:
        return math.nan
    s = sorted(values)
    return s[min(len(s) - 1, int(q * len(s)))]


def summarise(path: Path, p: Params) -> dict:
    trace = load(path)
    frames = trace.frames
    dur_s = (frames[-1].t_ms - frames[0].t_ms) / 1000 if len(frames) > 1 else 0.0
    inf = [f.inf_ms for f in frames if f.inf_ms is not None]
    reps = evaluate_reps(trace, p)
    passed = sum(r.passed for r in reps)
    task = trace.header.get("task")
    grid = tune_filter(trace)
    ok = [r for r in grid if r["jitter_px"] <= 3.0]
    best = min(ok, key=lambda r: r["lag_px"]) if ok else (min(grid, key=lambda r: r["jitter_px"]) if grid else None)
    return {
        "file": str(path),
        "task": task,
        "light": trace.header.get("light"),
        "frames": len(frames),
        "fps": round(len(frames) / dur_s, 1) if dur_s else None,
        "inference_ms_p50": round(pct(inf, 0.5), 1),
        "inference_ms_p95": round(pct(inf, 0.95), 1),
        "reps": len(reps),
        "recognised": sum(r.recognised for r in reps),
        "reps_with_swap": sum(r.swaps > 0 for r in reps),
        "reps_with_long_dropout": sum(r.longest_dropout_ms > MAX_DROPOUT_MS for r in reps),
        "passed": passed,
        "pass_bar": math.ceil(PASS_FRACTION * len(reps)) if reps else None,
        "verdict": None if task != "palm" or not reps else ("GO" if passed >= math.ceil(PASS_FRACTION * len(reps)) else "NO-GO"),
        "raw_jitter_px": round(raw_jitter(trace), 2),
        "filter_best": best,
        "facing_sign_by_label": facing_by_label(trace),
        "rep_detail": [asdict(r) for r in reps],
        "filter_grid": grid,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("traces", nargs="+", type=Path)
    ap.add_argument("--json", type=Path, help="write the full summary here")
    ap.add_argument("--reps", action="store_true", help="print every rep")
    ap.add_argument("--touch-ratio", type=float, default=Params.touch_ratio)
    ap.add_argument("--pinch-ratio", type=float, default=Params.pinch_ratio)
    ap.add_argument("--near-ratio", type=float, default=Params.near_ratio)
    ap.add_argument("--min-frames", type=int, default=Params.min_frames)
    args = ap.parse_args(argv)
    p = Params(args.touch_ratio, args.pinch_ratio, args.near_ratio, args.min_frames)

    paths = [q for t in args.traces for q in (sorted(t.glob("*.jsonl")) if t.is_dir() else [t])]
    summaries = [summarise(q, p) for q in paths]
    for s in summaries:
        print(f"\n== {s['file']}  ({s['task']}, {s['light']} light)")
        print(f"   frames {s['frames']} at {s['fps']} fps; inference p50 {s['inference_ms_p50']} ms, p95 {s['inference_ms_p95']} ms")
        print(f"   reps {s['reps']}: recognised {s['recognised']}, with swap {s['reps_with_swap']}, "
              f"dropout >{MAX_DROPOUT_MS:.0f} ms {s['reps_with_long_dropout']}")
        print(f"   PASSED {s['passed']} of {s['reps']} (bar {s['pass_bar']})" + (f"  -> {s['verdict']}" if s["verdict"] else ""))
        print(f"   jitter while still: raw {s['raw_jitter_px']} px RMS; best 1€ {s['filter_best']}")
        print(f"   palm-normal sign by label (palms to camera): {s['facing_sign_by_label']}")
        if args.reps:
            for r in s["rep_detail"]:
                print("     ", r)
    if args.json:
        args.json.write_text(json.dumps(summaries, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
