"""Load benchmark for live.py: mean CPU, RAM and per-stage cost with no hands, then with hands.

  uv run bench.py                                   # 20 s idle + 20 s hands, then save JSON
  uv run bench.py --label "skip graph redraw" --seconds 30
  uv run bench.py --phases idle                     # idle only (no one needs to sit at the camera)
  uv run bench.py --report ../results/perf/X.json                   # print the summary again
  uv run bench.py --report ../results/perf/B.json --against A.json  # summary with deltas vs A

This runs the real live.py loop (same window, same work) with timing hooks around each stage. It
walks you through: warm-up (not measured) -> IDLE (keep hands out of view) -> get ready -> HANDS
(show both hands and move them a little). Only 1 s windows where >=90% of frames match the phase
(0 hands for IDLE, --hands for HANDS) are counted. The run pauses if a hand wanders in or drops out.

Results go to ../results/perf/<time>-<git sha>.json, and a Markdown summary is printed for
OPTIMIZATION.md. CPU is "% of one core" (100 = one core fully busy).
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

import cv2
import numpy as np

HERE = Path(__file__).parent
OUT_DIR = HERE.parent / "results" / "perf"
WINDOW_S = 1.0
MATCH = 0.9  # fraction of a window's frames that must match the phase
STAGES = ("camera", "preprocess", "inference", "stabilizer", "angles", "overlay", "graph", "compose", "display")
PID = os.getpid()
with open("/proc/self/comm") as _f:
    MAIN_COMM = _f.read().strip()


# ---------- /proc readers ----------

def proc_cpu() -> float:
    with open("/proc/self/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def thread_group(tid: int, comm: str, pid: int = PID, main_comm: str = MAIN_COMM) -> str:
    """Pool threads share a name up to a trailing number; sum them as one group."""
    comm = comm.strip()
    if tid == pid:
        return "main thread"
    if comm == main_comm:
        return "unnamed threads"  # inherited the process name
    return re.sub(r"[-_:.#]?\d+$", "", comm) or "?"


def thread_cpu() -> dict[int, tuple[str, float]]:
    out, tick = {}, os.sysconf("SC_CLK_TCK")
    for name in os.listdir("/proc/self/task"):
        try:
            with open(f"/proc/self/task/{name}/comm") as f:
                comm = f.read()
            with open(f"/proc/self/task/{name}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError):
            continue  # thread exited between listdir and open
        tid = int(name)
        out[tid] = (thread_group(tid, comm), (int(fields[11]) + int(fields[12])) / tick)
    return out


def status() -> dict[str, int]:
    keys = ("VmRSS", "VmHWM", "RssAnon", "RssFile", "Threads",
            "voluntary_ctxt_switches", "nonvoluntary_ctxt_switches")
    out = {}
    with open("/proc/self/status") as f:
        for line in f:
            k, _, v = line.partition(":")
            if k in keys:
                out[k] = int(v.split()[0])
    return out


def mem_group(path: str) -> str:
    """Bucket one /proc/self/smaps mapping by what owns it."""
    if not path:
        return "anonymous (numpy/TFLite buffers, malloc arenas, thread stacks)"
    if path == "[heap]":
        return "[heap] (Python objects, main malloc arena)"
    if path.startswith("[stack"):
        return "main stack"
    if path.startswith("["):
        return "kernel ([vdso], [vvar], ...)"
    m = re.search(r"site-packages/([^/]+)", path)
    if m:
        pkg = m.group(1).split(".")[0].split("-")[0]
        pkg = {"opencv_python": "cv2"}.get(pkg, pkg)
        return f"{pkg} (wheel files)"
    if path.startswith("/dev/"):
        return f"device mapping {path}"
    if path.endswith((".task", ".tflite")):
        return "model file"
    if path.startswith("/memfd:") or "(deleted)" in path:
        return "memfd / deleted (shared buffers)"
    if "python" in Path(path).name or "/lib/python3" in path:
        return "python runtime"
    return "system libs"


def parse_smaps(lines) -> dict[str, float]:
    """RSS in MiB per owner, largest first."""
    groups: dict[str, int] = {}
    cur = ""
    for line in lines:
        head = line.split(" ", 1)[0]
        if "-" in head and all(c in "0123456789abcdef-" for c in head):
            parts = line.split(None, 5)
            cur = mem_group(parts[5].strip() if len(parts) > 5 else "")
        elif line.startswith("Rss:"):
            groups[cur] = groups.get(cur, 0) + int(line.split()[1])
    return {k: round(v / 1024, 2) for k, v in sorted(groups.items(), key=lambda kv: -kv[1]) if v}


def mem_breakdown() -> dict[str, float]:
    with open("/proc/self/smaps") as f:
        return parse_smaps(f)


# ---------- statistics ----------

def pct(xs: list[float], q: float) -> float:
    if not xs:
        return 0.0
    s = sorted(xs)
    return s[min(len(s) - 1, round(q * (len(s) - 1)))]


def mean(xs) -> float:
    xs = list(xs)
    return sum(xs) / len(xs) if xs else 0.0


@dataclass
class Phase:
    name: str
    seconds: float
    want: int | None          # hands required per frame; None = not measured
    prompt: str


@dataclass
class Acc:
    """Totals over the counted windows of one phase."""
    wall: float = 0.0
    cpu: float = 0.0
    frames: int = 0
    hands: int = 0
    frame_ms: list = field(default_factory=list)
    stage_wall: dict = field(default_factory=lambda: {s: [] for s in STAGES})
    stage_cpu: dict = field(default_factory=lambda: {s: [] for s in STAGES})
    main_cpu: float = 0.0
    hook_cpu: float = 0.0
    threads: dict = field(default_factory=dict)        # group -> cpu seconds
    thread_n: dict = field(default_factory=dict)       # group -> max thread count
    vol: int = 0
    invol: int = 0
    rss: list = field(default_factory=list)
    load: list = field(default_factory=list)
    mem: dict = field(default_factory=dict)
    status: dict = field(default_factory=dict)

    def summary(self) -> dict:
        w = self.wall or 1e-9
        stages = {}
        for s in STAGES:
            ws, cs = self.stage_wall[s], self.stage_cpu[s]
            stages[s] = {"wall_ms": round(mean(ws), 3), "wall_p95_ms": round(pct(ws, 0.95), 3),
                         "cpu_ms": round(mean(cs), 3),
                         "cpu_pct": round(sum(cs) / 1000 / w * 100, 2)}
        staged = sum(sum(v) for v in self.stage_cpu.values()) / 1000
        threads = {g: round(c / w * 100, 2) for g, c in sorted(self.threads.items(), key=lambda kv: -kv[1])}
        return {
            "seconds": round(self.wall, 1),
            "frames": self.frames,
            "fps": round(self.frames / w, 2),
            "mean_hands": round(self.hands / max(self.frames, 1), 2),
            "cpu_pct": round(self.cpu / w * 100, 1),
            "cpu_pct_of_machine": round(self.cpu / w * 100 / os.cpu_count(), 2),
            "frame_ms": {"mean": round(mean(self.frame_ms), 2), "p50": round(pct(self.frame_ms, 0.5), 2),
                         "p95": round(pct(self.frame_ms, 0.95), 2)},
            "stages": stages,
            "main_thread": {"cpu_pct": round(self.main_cpu / w * 100, 2),
                            "unstaged_cpu_pct": round(max(self.main_cpu - staged - self.hook_cpu, 0) / w * 100, 2),
                            "bench_hooks_cpu_pct": round(self.hook_cpu / w * 100, 2)},
            "threads_cpu_pct": threads,
            "thread_counts": dict(self.thread_n),
            "ctx_switches_per_s": {"voluntary": round(self.vol / w, 1), "involuntary": round(self.invol / w, 1)},
            "rss_mib": {"mean": round(mean(self.rss), 1), "max": round(max(self.rss, default=0), 1)},
            "status_kib": self.status,
            "memory_mib": self.mem,
            "loadavg_1m": round(mean(self.load), 2),
        }


class BenchProbe:
    """Plugs into live.main(probe=...). Times stages, samples /proc each second, drives the phases."""

    def __init__(self, phases: list[Phase]):
        self.phases, self.i = phases, 0
        self.acc = {p.name: Acc() for p in phases if p.want is not None}
        self.cur: dict[str, tuple[float, float]] = {}
        self.frames: list[tuple[int, float, dict]] = []   # (hands, frame wall ms, stages) in this window
        self.phase_t0 = time.monotonic()
        self.last_frame = None
        self.matching = False
        self.done = False
        self.cam = (0, 0)
        self._open_window()

    def stage(self, name: str):
        return _Stage(self, name)

    @property
    def phase(self) -> Phase:
        return self.phases[self.i]

    def _open_window(self) -> None:
        self.w_t0, self.w_cpu, self.w_thr, self.w_st = time.monotonic(), proc_cpu(), thread_cpu(), status()
        self.w_hook = 0.0
        self.frames = []

    def _close_window(self, now: float) -> None:
        ph = self.phase
        acc = self.acc.get(ph.name)
        n = len(self.frames)
        if acc is not None and n and sum(h == ph.want for h, _, _ in self.frames) / n >= MATCH:
            cpu, thr, st = proc_cpu(), thread_cpu(), status()
            acc.wall += now - self.w_t0
            acc.cpu += cpu - self.w_cpu
            acc.frames += n
            acc.hands += sum(h for h, _, _ in self.frames)
            counts: dict[str, int] = {}
            for tid, (g, c) in thr.items():
                d = c - self.w_thr.get(tid, (g, 0.0))[1]
                acc.threads[g] = acc.threads.get(g, 0.0) + d
                counts[g] = counts.get(g, 0) + 1
                if tid == PID:
                    acc.main_cpu += d
            for g, k in counts.items():
                acc.thread_n[g] = max(acc.thread_n.get(g, 0), k)
            acc.vol += st["voluntary_ctxt_switches"] - self.w_st["voluntary_ctxt_switches"]
            acc.invol += st["nonvoluntary_ctxt_switches"] - self.w_st["nonvoluntary_ctxt_switches"]
            acc.rss.append(st["VmRSS"] / 1024)
            acc.load.append(os.getloadavg()[0])
            acc.hook_cpu += self.w_hook
            for _, fm, stages in self.frames:
                acc.frame_ms.append(fm)
                for s in STAGES:
                    wall, cpu_ms = stages.get(s, (0.0, 0.0))
                    acc.stage_wall[s].append(wall)
                    acc.stage_cpu[s].append(cpu_ms)
        self._open_window()

    def progress(self) -> float:
        ph = self.phase
        if ph.want is None:
            return min((time.monotonic() - self.phase_t0) / ph.seconds, 1.0)
        return min(self.acc[ph.name].wall / ph.seconds, 1.0)

    def _advance(self) -> None:
        ph = self.phase
        if ph.name in self.acc:
            self.acc[ph.name].mem = mem_breakdown()
            self.acc[ph.name].status = status()
        self.i += 1
        self.phase_t0 = time.monotonic()
        if self.i >= len(self.phases):
            self.done = True
        else:
            self._open_window()

    def frame(self, view: np.ndarray, hands: int, cam: tuple[int, int]) -> bool:
        h0 = time.thread_time()
        now = time.monotonic()
        if self.last_frame is None:
            # Camera and model setup happen between construction and the first frame; start the
            # phase and window clocks here so setup does not eat into the warm-up.
            self.phase_t0 = now
            self._open_window()
        self.cam = cam
        fm = (now - self.last_frame) * 1000 if self.last_frame else 0.0
        self.last_frame = now
        ph = self.phase
        self.matching = ph.want is None or hands == ph.want
        if fm:
            self.frames.append((hands, fm, self.cur))
        self.cur = {}
        if now - self.w_t0 >= WINDOW_S:
            self._close_window(now)
        if self.progress() >= 1.0:
            self._advance()
        if not self.done:
            self._banner(view, hands)
        self.w_hook += time.thread_time() - h0
        return not self.done

    def _banner(self, view: np.ndarray, hands: int) -> None:
        ph = self.phase
        vh, vw = view.shape[:2]
        y0 = vh - 72
        colour = (60, 60, 60) if ph.want is None else ((40, 140, 40) if self.matching else (40, 40, 180))
        cv2.rectangle(view, (0, y0), (vw, vh), colour, -1)
        step = f"[{self.i + 1}/{len(self.phases)}] {ph.name.upper()}"
        msg = ph.prompt if self.matching else f"PAUSED: seeing {hands} hand(s), need {ph.want}. {ph.prompt}"
        cv2.putText(view, step, (12, y0 + 26), cv2.FONT_HERSHEY_SIMPLEX, 0.8, (255, 255, 255), 2, cv2.LINE_AA)
        cv2.putText(view, msg, (12, y0 + 52), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (255, 255, 255), 1, cv2.LINE_AA)
        bar = int((vw - 24) * self.progress())
        cv2.rectangle(view, (12, y0 + 60), (12 + bar, y0 + 66), (255, 255, 255), -1)


class _Stage:
    __slots__ = ("c", "name", "p", "w")

    def __init__(self, p: BenchProbe, name: str):
        self.p, self.name = p, name

    def __enter__(self):
        self.w, self.c = time.perf_counter(), time.thread_time()

    def __exit__(self, *exc):
        self.p.cur[self.name] = ((time.perf_counter() - self.w) * 1000, (time.thread_time() - self.c) * 1000)


# ---------- metadata / report ----------

def git(*args: str) -> str:
    try:
        return subprocess.run(["git", *args], cwd=HERE, capture_output=True, text=True, check=True).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return ""


def machine() -> dict:
    with open("/proc/cpuinfo") as f:
        cpu = next((ln.split(":", 1)[1].strip() for ln in f if ln.startswith("model name")), "?")
    try:
        gov = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read_text().strip()
    except OSError:
        gov = "?"
    import mediapipe
    return {"cpu": cpu, "cores": os.cpu_count(), "governor": gov, "kernel": platform.release(),
            "python": platform.python_version(), "mediapipe": mediapipe.__version__, "opencv": cv2.__version__,
            "numpy": np.__version__}


def fmt_delta(new: float, old: float | None) -> str:
    if old is None:
        return f"{new:g}"
    d = new - old
    if not d:
        return f"{new:g} (=)"
    rel = f", {d / old * 100:+.0f}%" if old else ""
    return f"{new:g} ({d:+.3g}{rel})"


def _get(r: dict | None, *path):
    cur = r
    for k in path:
        if not isinstance(cur, dict) or k not in cur:
            return None
        cur = cur[k]
    return cur


def report(res: dict, base: dict | None = None) -> str:
    """Markdown summary; with `base`, every number shows its change vs that run."""
    m = res["meta"]
    lines = [f"### {m['label'] or 'run'}: `{m['git']}`{' (dirty)' if m['dirty'] else ''}, {m['time']}", ""]
    if base:
        lines += [f"Changes are against `{base['meta']['git']}` ({base['meta']['label'] or 'run'}).", ""]
    names = [p for p in ("idle", "hands") if p in res["phases"]]
    head = "| | " + " | ".join(n.upper() for n in names) + " |"
    sep = "|---|" + "---|" * len(names)

    def row(title, *path):
        cells = []
        for p in names:
            v = _get(res, "phases", p, *path)
            cells.append("—" if v is None else fmt_delta(v, _get(base, "phases", p, *path)))
        return f"| {title} | " + " | ".join(cells) + " |"

    def union(key):
        out = []
        for p in names:
            out += [g for g in res["phases"][p][key] if g not in out]
        return out

    lines += ["**Totals**", "", head, sep,
              row("CPU, % of one core", "cpu_pct"),
              row("RSS mean, MiB", "rss_mib", "mean"),
              row("RSS max, MiB", "rss_mib", "max"),
              row("Frame rate, fps", "fps"),
              row("Frame time mean, ms", "frame_ms", "mean"),
              row("Frame time p95, ms", "frame_ms", "p95"),
              row("Inference mean, ms", "stages", "inference", "wall_ms"),
              row("Inference p95, ms", "stages", "inference", "wall_p95_ms"),
              row("Threads", "status_kib", "Threads"),
              row("Context switches/s (voluntary)", "ctx_switches_per_s", "voluntary"),
              row("Mean hands in view", "mean_hands"),
              "", "**CPU by main-loop stage** (% of one core, main thread only; wall ms per frame in brackets)",
              "", head, sep]
    for s in STAGES:
        cells = []
        for p in names:
            st = _get(res, "phases", p, "stages", s)
            b = _get(base, "phases", p, "stages", s, "cpu_pct")
            cells.append("—" if st is None else f"{fmt_delta(st['cpu_pct'], b)} [{st['wall_ms']:.2f} ms]")
        lines.append(f"| {s} | " + " | ".join(cells) + " |")
    lines.append(row("main thread, outside stages", "main_thread", "unstaged_cpu_pct"))
    lines.append(row("bench hooks (measurement overhead)", "main_thread", "bench_hooks_cpu_pct"))

    lines += ["", "**CPU by thread** (% of one core; thread count in brackets)", "", head, sep]
    for g in union("threads_cpu_pct"):
        cells = []
        for p in names:
            v = _get(res, "phases", p, "threads_cpu_pct", g)
            n = _get(res, "phases", p, "thread_counts", g)
            b = _get(base, "phases", p, "threads_cpu_pct", g)
            cells.append("—" if v is None else f"{fmt_delta(v, b)} [{n}]")
        lines.append(f"| {g} | " + " | ".join(cells) + " |")

    lines += ["", "**RSS by owner** (MiB, end of phase)", "", head, sep]
    for g in union("memory_mib"):
        if any((_get(res, "phases", p, "memory_mib", g) or 0) >= 0.5 for p in names):  # hide tiny groups
            lines.append(row(g, "memory_mib", g))
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--seconds", type=float, default=20.0, help="measured seconds per phase")
    ap.add_argument("--warmup", type=float, default=5.0)
    ap.add_argument("--phases", default="idle,hands", help="comma list of idle,hands")
    ap.add_argument("--label", default="", help="what this run measures, e.g. the optimization name")
    ap.add_argument("--camera", default="/dev/video0")
    ap.add_argument("--hands", type=int, default=2, help="hands to show in the HANDS phase")
    ap.add_argument("--scale", type=float, default=1.6)
    ap.add_argument("--out", type=Path, default=None, help="output JSON (default ../results/perf/...)")
    ap.add_argument("--report", type=Path, help="print the summary of a saved run and exit")
    ap.add_argument("--against", type=Path, help="with --report or a new run: show deltas vs this run")
    args = ap.parse_args(argv)
    base = json.loads(args.against.read_text()) if args.against else None
    if args.report:
        print(report(json.loads(args.report.read_text()), base))
        return 0

    phases = [Phase("warmup", args.warmup, None, "Warming up (not measured)...")]
    for name in args.phases.split(","):
        if name == "idle":
            phases.append(Phase("idle", args.seconds, 0, "Keep your hands OUT of view."))
        elif name == "hands":
            phases += [Phase("ready", 3.0, None, f"Get ready: raise {args.hands} hand(s) into view."),
                       Phase("hands", args.seconds, args.hands,
                             f"Keep {args.hands} hand(s) in view, move them a little.")]
        else:
            ap.error(f"unknown phase {name!r}")

    import live  # imported late so --report works without opening a camera
    probe = BenchProbe(phases)
    t0 = time.time()
    rc = live.main(["--camera", args.camera, "--hands", str(args.hands), "--scale", str(args.scale)], probe=probe)
    measured = {n: a.summary() for n, a in probe.acc.items() if a.frames}
    if not measured:
        print("nothing measured (quit before any phase collected data)", file=sys.stderr)
        return rc or 1
    sha = git("rev-parse", "--short", "HEAD")
    res = {
        "meta": {"label": args.label, "git": sha, "branch": git("rev-parse", "--abbrev-ref", "HEAD"),
                 "dirty": bool(git("status", "--porcelain", "--", ".")),
                 "time": time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t0)), "complete": probe.done,
                 "camera": {"device": args.camera, "size": list(probe.cam)}, "hands_wanted": args.hands,
                 "scale": args.scale, "machine": machine()},
        "phases": measured,
    }
    out = args.out or OUT_DIR / f"{time.strftime('%Y%m%d-%H%M%S', time.localtime(t0))}-{sha or 'nogit'}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(res, indent=1) + "\n")
    print(report(res, base))
    print(f"\nsaved {out}", file=sys.stderr)
    if not probe.done:
        print("warning: run stopped early; phases are incomplete", file=sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(main())
