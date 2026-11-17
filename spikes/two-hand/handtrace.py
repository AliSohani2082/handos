"""Draft landmark trace format and shared hand geometry for spike 1.

A trace is JSON Lines. Each line is one record with a "type":
  header  {"type":"header","format":..., "task":..., "light":..., "width":..., "height":..., "mirrored":true}
  hold    {"type":"hold","start_ms":..., "end_ms":...}          still hands, palms to camera
  rep     {"type":"rep","i":..., "task":..., "start_ms":..., "end_ms":...}   one gesture window
  frame   {"type":"frame","t_ms":..., "inf_ms":..., "hands":[{"label","score","lm","wl"}]}

lm: 21 x [x, y, z] normalised to the (mirrored) image. wl: 21 x [x, y, z] in metres, hand-centred.
This is the first draft of the .htrace format the roadmap's trace replay will use.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO, Sequence

FORMAT = "handos-trace/0"

WRIST, THUMB_TIP, INDEX_TIP, MIDDLE_MCP = 0, 4, 8, 9
PALM = (0, 5, 9, 13, 17)
TIPS = (4, 8, 12, 16, 20)
CONNECTIONS = (
    (0, 1), (1, 2), (2, 3), (3, 4),
    (0, 5), (5, 6), (6, 7), (7, 8),
    (5, 9), (9, 10), (10, 11), (11, 12),
    (9, 13), (13, 14), (14, 15), (15, 16),
    (13, 17), (17, 18), (18, 19), (19, 20),
    (0, 17),
)

Point = Sequence[float]


@dataclass
class Hand:
    label: str
    score: float
    lm: list[list[float]]
    wl: list[list[float]]


@dataclass
class Frame:
    t_ms: float
    hands: list[Hand]
    inf_ms: float | None = None


@dataclass
class Rep:
    i: int
    task: str
    start_ms: float
    end_ms: float


@dataclass
class Trace:
    header: dict
    frames: list[Frame] = field(default_factory=list)
    reps: list[Rep] = field(default_factory=list)
    hold: tuple[float, float] | None = None


def _r(v: float) -> float:
    return round(float(v), 5)


class TraceWriter:
    def __init__(self, fp: IO[str]):
        self.fp = fp

    def _write(self, rec: dict) -> None:
        self.fp.write(json.dumps(rec, separators=(",", ":")) + "\n")

    def header(self, **meta) -> None:
        self._write({"type": "header", "format": FORMAT, **meta})

    def hold(self, start_ms: float, end_ms: float) -> None:
        self._write({"type": "hold", "start_ms": round(start_ms, 1), "end_ms": round(end_ms, 1)})

    def rep(self, rep: Rep) -> None:
        self._write({"type": "rep", "i": rep.i, "task": rep.task,
                     "start_ms": round(rep.start_ms, 1), "end_ms": round(rep.end_ms, 1)})

    def frame(self, frame: Frame) -> None:
        self._write({
            "type": "frame",
            "t_ms": round(frame.t_ms, 1),
            "inf_ms": None if frame.inf_ms is None else round(frame.inf_ms, 2),
            "hands": [
                {"label": h.label, "score": _r(h.score),
                 "lm": [[_r(c) for c in p] for p in h.lm],
                 "wl": [[_r(c) for c in p] for p in h.wl]}
                for h in frame.hands
            ],
        })


def load(path: Path) -> Trace:
    trace = Trace(header={})
    with open(path) as fp:
        for line in fp:
            rec = json.loads(line)
            kind = rec.pop("type")
            if kind == "header":
                trace.header = rec
            elif kind == "hold":
                trace.hold = (rec["start_ms"], rec["end_ms"])
            elif kind == "rep":
                trace.reps.append(Rep(rec["i"], rec["task"], rec["start_ms"], rec["end_ms"]))
            elif kind == "frame":
                trace.frames.append(Frame(rec["t_ms"], [Hand(**h) for h in rec["hands"]], rec.get("inf_ms")))
    return trace


def dist(a: Point, b: Point) -> float:
    """Distance in the image plane (x, y only)."""
    return math.hypot(a[0] - b[0], a[1] - b[1])


def palm_centre(lm: list[list[float]]) -> tuple[float, float]:
    return (sum(lm[i][0] for i in PALM) / len(PALM), sum(lm[i][1] for i in PALM) / len(PALM))


def palm_size(lm: list[list[float]]) -> float:
    """Wrist to middle-finger knuckle: a scale that stays stable as fingers move."""
    return dist(lm[WRIST], lm[MIDDLE_MCP])


def facing_z(wl: list[list[float]]) -> float:
    """z of the palm normal (wrist->index MCP) x (wrist->pinky MCP), from world landmarks."""
    w, a, b = wl[0], wl[5], wl[17]
    ax, ay = a[0] - w[0], a[1] - w[1]
    bx, by = b[0] - w[0], b[1] - w[1]
    return ax * by - ay * bx
