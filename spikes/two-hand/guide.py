"""Animated ghost-hand guide for record.py (green wire skeletons in the mirrored preview).

Two tasks, both with one hand staying still (the receiver) and the other moving (the mover):

  palm   Mover: fist with ONLY the index finger out. Its index tip travels to the CENTRE of the
         receiver's open palm, rests there ~0.7 s, then pulls away.
         Scored as: index tip within 0.6 palm-sizes of the receiver's palm centre, 2+ frames.
  pluck  Mover: thumb and index finger open like a pinch, travels to the receiver's INDEX
         FINGERTIP, closes the pinch on it, holds ~0.5 s, then pulls away still pinched.
         Scored as: thumb-index gap under 0.3 palm-sizes while that pinch point is within 0.5
         palm-sizes of a fingertip of the other hand, 2+ frames.

Rep timeline (3 s): 0-1 s READY (hands apart), then a 2 s window with the motion above.

  uv run guide.py --task palm  [--mover right|left]   loop the animation in a window (no camera)
  uv run guide.py --task pluck --png out/             write a contact sheet of key moments
"""

from __future__ import annotations

import argparse
import math
from pathlib import Path

import cv2
import numpy as np

from handtrace import CONNECTIONS

W, H = 640, 480
SIZE = 80.0                      # wrist to middle knuckle, in pixels
LEFT_C, RIGHT_C = (190.0, 280.0), (450.0, 280.0)
GHOST = (80, 255, 80)

INSTRUCTIONS = {
    "palm": [
        "PALM TOUCH  (50 reps; one hand stays still, the other moves)",
        "  Still hand : open palm to the camera, fingers spread, held steady.",
        "  Moving hand: make a fist, then point with ONLY the index finger.",
        "  Each rep   : 1 s ready (hands apart), then in the next 2 s put your index FINGERTIP",
        "               on the CENTRE of the other palm, rest it there about 0.7 s, pull away.",
    ],
    "pluck": [
        "FINGERTIP PLUCK  (50 reps; one hand stays still, the other moves)",
        "  Still hand : open hand to the camera, index finger pointing up and clearly visible.",
        "  Moving hand: thumb and index held open like a C, other fingers curled.",
        "  Each rep   : 1 s ready (hands apart), then in the next 2 s bring the C to the other",
        "               INDEX FINGERTIP, close thumb onto index to pinch it, hold ~0.5 s,",
        "               then pull away with the pinch still closed.",
    ],
}


# One prompt per phase, shown on record.py's top line. No sub-step captions: they changed every
# 0.3-0.7 s and could not be read while moving.
PROMPTS = {
    "palm": {"ready": "ready: hands apart, index pointing", "go": "TOUCH the centre of your palm"},
    "pluck": {"ready": "ready: hands apart, thumb+index open", "go": "PINCH the other index tip"},
}
REP_SECONDS = 3.0  # the animation is authored for a 3 s rep and scaled to the real rep length


def smooth(x: float) -> float:
    x = min(max(x, 0.0), 1.0)
    return x * x * (3 - 2 * x)


def hand_unit(side: str, index_curl=0.0, other_curl=0.0, thumb_tuck=0.0, pinch=0.0) -> np.ndarray:
    """21 landmarks (MediaPipe order) in hand units: palm centre near (0, 0), wrist at (0, 1)."""
    s = 1.0 if side == "left" else -1.0       # palm to camera: a left hand has its thumb on the +x side
    pts = np.zeros((21, 2))
    pts[0] = (0.0, 1.0)
    mcp = {5: (0.33, 0.0), 9: (0.08, -0.05), 13: (-0.17, 0.0), 17: (-0.40, 0.12)}
    drift = {5: 0.08, 9: 0.0, 13: -0.05, 17: -0.12}
    lens = {5: (0.38, 0.25, 0.22), 9: (0.42, 0.27, 0.24), 13: (0.38, 0.25, 0.22), 17: (0.30, 0.20, 0.18)}
    for base in (5, 9, 13, 17):
        curl = index_curl if base == 5 else other_curl
        p = np.array(mcp[base]) * (s, 1.0)
        pts[base] = p
        d = np.array((drift[base] * s, -1.0))
        d /= np.linalg.norm(d)
        for k, length in enumerate(lens[base]):
            p = p + d * length * math.cos(curl * 0.85 * (k + 1))   # foreshortening as the finger folds
            pts[base + 1 + k] = p
    thumb = np.array([(0.25, 0.75), (0.5, 0.55), (0.72, 0.38), (0.88, 0.22)]) * (s, 1.0)
    thumb[1:] = thumb[1:] * (1 - thumb_tuck) + np.array((0.12 * s, 0.35)) * thumb_tuck
    if pinch > 0:
        base, tip = thumb[0], pts[8]
        for k in (1, 2, 3):
            thumb[k] = thumb[k] * (1 - pinch) + (base + (tip - base) * k / 3) * pinch
    pts[1:5] = thumb
    return pts


def to_px(unit: np.ndarray, centre, rot_deg: float = 0.0) -> np.ndarray:
    a = math.radians(rot_deg)
    r = np.array([[math.cos(a), -math.sin(a)], [math.sin(a), math.cos(a)]])
    return (unit @ r.T) * SIZE + np.array(centre)


def palm_centre(px: np.ndarray) -> np.ndarray:
    return px[[0, 5, 9, 13, 17]].mean(axis=0)


def pinch_point(px: np.ndarray) -> np.ndarray:
    return (px[4] + px[8]) / 2


def scene(task: str, t: float, mover: str = "right") -> dict:
    """Ghost hands at `t` seconds into a rep: {"hands": [receiver, mover], "target"}."""
    rec_side = "left" if mover == "right" else "right"
    rec_c, mov_c = (LEFT_C, RIGHT_C) if mover == "right" else (RIGHT_C, LEFT_C)
    rot = -50.0 if mover == "right" else 50.0          # the moving index points toward the other hand
    rec = to_px(hand_unit(rec_side), rec_c)

    g = t - 1.0
    pinch = 0.0
    if task == "palm":
        target = palm_centre(rec)
        if g < 0:
            e = 0.0
        elif g < 0.7:
            e = smooth(g / 0.7)
        elif g < 1.4:
            e = 1.0
        else:
            e = 1 - smooth((g - 1.4) / 0.6)
        unit = hand_unit(mover, 0.0, 1.0, 0.8)
        anchor = lambda px: px[8]
    else:
        target = rec[8]
        if g < 0:
            e = 0.0
        elif g < 0.6:
            e = smooth(g / 0.6)
        elif g < 0.9:
            e, pinch = 1.0, (g - 0.6) / 0.3
        elif g < 1.4:
            e, pinch = 1.0, 1.0
        else:
            e, pinch = 1 - smooth((g - 1.4) / 0.6), 1.0
        unit = hand_unit(mover, 0.45, 1.0, 0.0, pinch)
        anchor = pinch_point

    home = anchor(to_px(unit, mov_c, rot))
    pos = home + (np.asarray(target) - home) * e
    origin_anchor = anchor(to_px(unit, (0.0, 0.0), rot))
    mov = to_px(unit, pos - origin_anchor, rot)
    return {"hands": [rec, mov], "target": target if e > 0 else None}


def idle_scene() -> dict:
    left, right = to_px(hand_unit("left"), LEFT_C), to_px(hand_unit("right"), RIGHT_C)
    return {"hands": [left, right], "target": None}


def draw(img: np.ndarray, task: str, phase: str, rep_t: float, mover: str = "right", rep_seconds: float = REP_SECONDS) -> None:
    """Draw the ghost hands and target ring onto a BGR frame (in place). Text is record.py's job."""
    if phase == "done":
        return
    rep_t = rep_t * REP_SECONDS / rep_seconds
    sc = idle_scene() if phase in ("countdown", "hold") else scene(task, rep_t, mover)
    layer = img.copy()
    for hand in sc["hands"]:
        pts = [tuple(int(v) for v in p) for p in hand]
        for a, b in CONNECTIONS:
            cv2.line(layer, pts[a], pts[b], GHOST, 4, cv2.LINE_AA)
        for p in pts:
            cv2.circle(layer, p, 5, GHOST, -1, cv2.LINE_AA)
    cv2.addWeighted(layer, 0.55, img, 0.45, 0, dst=img)
    if sc["target"] is not None:
        r = int(20 + 5 * math.sin(rep_t * 4 * math.pi))
        cv2.circle(img, tuple(int(v) for v in sc["target"]), r, (60, 60, 255), 2, cv2.LINE_AA)


def main(argv=None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--task", choices=["palm", "pluck"], required=True)
    ap.add_argument("--mover", choices=["right", "left"], default="right")
    ap.add_argument("--png", type=Path, help="write a contact sheet here instead of opening a window")
    args = ap.parse_args(argv)
    if args.png:
        args.png.mkdir(parents=True, exist_ok=True)
        tiles = []
        for t in [0.5, 1.4, 1.75, 2.0, 2.4, 2.9]:
            img = np.full((H, W, 3), 40, np.uint8)
            draw(img, args.task, "go", t, args.mover)
            cv2.putText(img, f"t={t:.2f}s", (8, 24), cv2.FONT_HERSHEY_SIMPLEX, 0.7, (200, 200, 200), 2)
            tiles.append(cv2.resize(img, (W // 2, H // 2)))
        sheet = np.vstack([np.hstack(tiles[:3]), np.hstack(tiles[3:])])
        out = args.png / f"guide-{args.task}.png"
        cv2.imwrite(str(out), sheet)
        print(out)
        return
    print("\n".join(INSTRUCTIONS[args.task]))
    t0 = cv2.getTickCount()
    while True:
        t = ((cv2.getTickCount() - t0) / cv2.getTickFrequency()) % 3.0
        img = np.full((H, W, 3), 40, np.uint8)
        draw(img, args.task, "go", t, args.mover)
        cv2.imshow("handOs guide (q to quit)", img)
        if cv2.waitKey(16) & 0xFF == ord("q"):
            break
    cv2.destroyAllWindows()


if __name__ == "__main__":
    main()
