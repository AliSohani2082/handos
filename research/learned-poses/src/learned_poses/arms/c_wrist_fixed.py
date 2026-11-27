"""Spike 5, arm C: the wrist-fixed hand frame (ADR 12, superseded by ADR 13).

Plan: docs/experiments/wrist-fixed-frame.md.

Features are the 21 MediaPipe world landmarks in a frame fixed to the wrist:

- origin: landmark 0 (wrist);
- y: towards landmark 9 (middle knuckle);
- z: the palm normal (p5 - p0) x (p17 - p0);
- x: completes a right-handed frame;
- distances in world hand units (hu), the world distance between landmarks 5 and 17.

Left hands are mirrored in x first, so one recording serves both hands. Orientation drops out
by construction: any rotation, shift or scale of the hand gives the same features.

The classifier is an MLP of 63 -> 64 -> 64 -> (one output per pose + "none") with softmax,
retrained from scratch on every fit with a fixed seed.
"""

from __future__ import annotations

from typing import Protocol

import numpy as np
from sklearn.neural_network import MLPClassifier

NONE = "none"
N_LANDMARKS = 21
N_FEATURES = N_LANDMARKS * 3
_EPS = 1e-9


class _Sample(Protocol):
    """The fields of the harness's `HandSample` that this arm reads."""

    world: np.ndarray | None
    handedness: str


def wrist_frame(world: np.ndarray, handedness: str) -> np.ndarray:
    """The 21 landmarks (21x3, any units) in the wrist-fixed frame, in hand units.

    Raises ValueError for a degenerate hand (palm points on a line, or zero hand size).
    """
    p = np.asarray(world, dtype=np.float64)
    if p.shape != (N_LANDMARKS, 3):
        raise ValueError(f"expected 21x3 landmarks, got {p.shape}")
    if handedness == "left":
        # Mirror before building the frame, so the palm normal (and so the frame) follows.
        p = p * np.array([-1.0, 1.0, 1.0])
    elif handedness != "right":
        raise ValueError(f"handedness must be 'left' or 'right', got {handedness!r}")

    d = p - p[0]
    y = d[9]
    normal = np.cross(d[5], d[17])
    x = np.cross(y, normal)
    hu = np.linalg.norm(d[5] - d[17])
    ny, nx = np.linalg.norm(y), np.linalg.norm(x)
    if ny < _EPS or nx < _EPS or hu < _EPS:
        raise ValueError("degenerate hand: cannot build the wrist-fixed frame")
    y, x = y / ny, x / nx
    z = np.cross(x, y)
    basis = np.stack([x, y, z])  # rows are the frame's axes
    return (d @ basis.T) / hu


class WristFixedArm:
    """Arm C of spike 5."""

    name = "C_wrist_fixed"

    def __init__(self, max_iter: int = 2000) -> None:
        self.max_iter = max_iter
        self.classes_: list[str] = []
        self._model: MLPClassifier | None = None
        self._columns: np.ndarray | None = None

    def featurize(self, s: _Sample) -> np.ndarray:
        """One frame -> 63 features. Raises ValueError when the frame has no world landmarks:
        the frame is defined on world landmarks only."""
        if s.world is None:
            raise ValueError("arm C needs world landmarks; this frame has none")
        return wrist_frame(s.world, s.handedness).reshape(N_FEATURES).astype(np.float32)

    def fit(self, X: np.ndarray, y: np.ndarray, *, seed: int) -> None:
        """Retrain from scratch. Output columns: poses in the order they first appear in `y`,
        then "none" (always present, even if `y` has no "none" samples)."""
        y = np.asarray(y).astype(str)
        poses = [c for c in dict.fromkeys(y.tolist()) if c != NONE]
        if not poses:
            raise ValueError("need at least one pose besides 'none'")
        self.classes_ = [*poses, NONE]
        self._model = MLPClassifier(
            hidden_layer_sizes=(64, 64),
            activation="relu",
            max_iter=self.max_iter,
            random_state=seed,
        )
        self._model.fit(np.asarray(X, dtype=np.float32), y)
        # Map sklearn's sorted classes onto our column order; -1 marks a class never seen.
        seen = {c: i for i, c in enumerate(self._model.classes_)}
        self._columns = np.array([seen.get(c, -1) for c in self.classes_])

    def predict_proba(self, X: np.ndarray) -> np.ndarray:
        """Per-frame probabilities, columns as in `classes_` (poses in label order, then
        "none")."""
        if self._model is None or self._columns is None:
            raise RuntimeError("fit before predict_proba")
        raw = self._model.predict_proba(np.asarray(X, dtype=np.float32))
        out = np.zeros((raw.shape[0], len(self.classes_)))
        for col, src in enumerate(self._columns):
            if src >= 0:
                out[:, col] = raw[:, src]
        return out


Arm = WristFixedArm
