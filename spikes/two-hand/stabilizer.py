"""Hand stabilizer: turns raw HandLandmarker output into steady, identity-tracked hands.

Design
  Adapter   `from_mediapipe()` converts a HandLandmarkerResult into plain `HandObs` records, so
            nothing downstream depends on MediaPipe types (a native tracker can feed the same type).
  Pipeline  `HandStabilizer` runs every frame's observations through an ordered list of stages.
            Each stage is a small class with `process(tracks, t) -> None` that edits the tracks
            in place. Add a new correction by writing one stage and appending it to `stages`.

Stages (in order)
  1. Associate   matches observations to existing tracks by palm position (one hand cannot jump
                 across the image between frames), creates tracks for new hands, and drops tracks
                 unseen for `max_missed` frames.
  2. StickyLabel a track keeps the Left/Right label it was born with. The raw label only wins after
                 disagreeing confidently for `flip_frames` frames in a row, and two tracks never
                 share a label.
  3. OneEuro     One Euro filter on image and world landmarks: heavy smoothing when the hand is
                 still, little lag when it moves fast.

  stab = HandStabilizer()
  hands = stab.update(from_mediapipe(result), t_seconds)   # list[StableHand]
"""

from __future__ import annotations

import itertools
import math
from dataclasses import dataclass, field

import numpy as np

PALM = (0, 5, 9, 13, 17)


@dataclass
class HandObs:
    """One detected hand in one frame (tracker-agnostic)."""
    label: str           # "Left" / "Right" as the model reported it
    score: float         # handedness confidence
    lm: np.ndarray       # (21, 3) image-normalised x, y, z
    world: np.ndarray    # (21, 3) metres, hand-centred


@dataclass
class StableHand:
    """Output of the stabilizer: what the rest of handOs should consume."""
    id: int
    label: str
    score: float
    lm: np.ndarray
    world: np.ndarray
    raw_label: str


def from_mediapipe(res) -> list[HandObs]:
    """Adapter: HandLandmarkerResult -> list[HandObs]."""
    return [
        HandObs(hd[0].category_name, hd[0].score,
                np.array([[p.x, p.y, p.z] for p in lm], dtype=float),
                np.array([[p.x, p.y, p.z] for p in wl], dtype=float))
        for lm, wl, hd in zip(res.hand_landmarks, res.hand_world_landmarks, res.handedness)
    ]


def palm_xy(lm: np.ndarray) -> np.ndarray:
    return lm[list(PALM), :2].mean(axis=0)


def other(label: str) -> str:
    return "Left" if label == "Right" else "Right"


class OneEuro:
    """Vectorised One Euro filter (Casiez et al. 2012) over an array of any shape."""

    def __init__(self, min_cutoff: float, beta: float, d_cutoff: float = 1.0):
        self.min_cutoff, self.beta, self.d_cutoff = min_cutoff, beta, d_cutoff
        self.x = self.dx = None
        self.t = 0.0

    @staticmethod
    def _alpha(cutoff, dt):
        r = 2 * math.pi * cutoff * dt
        return r / (r + 1)

    def __call__(self, x: np.ndarray, t: float) -> np.ndarray:
        if self.x is None:
            self.x, self.dx, self.t = x.copy(), np.zeros_like(x), t
            return x.copy()
        dt = max(t - self.t, 1e-3)
        self.t = t
        dx = (x - self.x) / dt
        self.dx += self._alpha(self.d_cutoff, dt) * (dx - self.dx)
        a = self._alpha(self.min_cutoff + self.beta * np.abs(self.dx), dt)
        self.x = self.x + a * (x - self.x)
        return self.x.copy()


@dataclass
class Track:
    id: int
    label: str
    obs: HandObs | None = None        # this frame's observation (None = not seen this frame)
    lm: np.ndarray | None = None      # stabilised output
    world: np.ndarray | None = None
    missed: int = 0
    disagree: int = 0
    filters: dict = field(default_factory=dict)


class Associate:
    def __init__(self, max_jump: float = 0.25, max_missed: int = 10):
        self.max_jump, self.max_missed = max_jump, max_missed
        self.ids = itertools.count(1)

    def process(self, tracks: list[Track], obs: list[HandObs]) -> None:
        for tr in tracks:
            tr.obs = None
        pairs = sorted(
            (float(np.linalg.norm(palm_xy(o.lm) - palm_xy(tr.lm))), i, j)
            for i, o in enumerate(obs) for j, tr in enumerate(tracks)
        )
        used_o, used_t = set(), set()
        for d, i, j in pairs:  # greedy nearest-first matching
            if d > self.max_jump or i in used_o or j in used_t:
                continue
            tracks[j].obs = obs[i]
            used_o.add(i)
            used_t.add(j)
        for i, o in enumerate(obs):
            if i not in used_o:
                taken = {t.label for t in tracks}
                label = o.label if o.label not in taken else other(o.label)
                tracks.append(Track(next(self.ids), label, obs=o, lm=o.lm, world=o.world))
        for tr in tracks:
            tr.missed = 0 if tr.obs is not None else tr.missed + 1
        tracks[:] = [t for t in tracks if t.missed <= self.max_missed]


class StickyLabel:
    def __init__(self, flip_frames: int = 30, min_score: float = 0.85):
        self.flip_frames, self.min_score = flip_frames, min_score

    def process(self, tracks: list[Track], t: float) -> None:
        for tr in tracks:
            o = tr.obs
            if o is None:
                continue
            if o.label != tr.label and o.score >= self.min_score:
                tr.disagree += 1
            else:
                tr.disagree = 0
            if tr.disagree >= self.flip_frames and all(x.label != o.label for x in tracks if x is not tr):
                tr.label, tr.disagree = o.label, 0


class OneEuroStage:
    def __init__(self, lm=(1.5, 8.0), world=(1.5, 15.0)):
        self.params = {"lm": lm, "world": world}

    def process(self, tracks: list[Track], t: float) -> None:
        for tr in tracks:
            if tr.obs is None:
                continue
            for key, (mc, beta) in self.params.items():
                f = tr.filters.setdefault(key, OneEuro(mc, beta))
                setattr(tr, key, f(getattr(tr.obs, key), t))


class HandStabilizer:
    def __init__(self, stages: list | None = None):
        self.assoc = Associate()
        self.stages = stages if stages is not None else [StickyLabel(), OneEuroStage()]
        self.tracks: list[Track] = []

    def update(self, obs: list[HandObs], t: float) -> list[StableHand]:
        self.assoc.process(self.tracks, obs)
        for stage in self.stages:
            stage.process(self.tracks, t)
        return [StableHand(tr.id, tr.label, tr.obs.score, tr.lm, tr.world, tr.obs.label)
                for tr in self.tracks if tr.obs is not None]
