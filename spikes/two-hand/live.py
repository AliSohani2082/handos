"""Live tracker view: webcam + MediaPipe with a stats panel (records nothing).

  uv run live.py                  # default camera, 2 hands
  uv run live.py --camera /dev/video2 --hands 1

Panel: camera fps, inference ms (last / avg / p95), hands seen with handedness score,
this process's CPU % (of one core) and RSS memory, system load.
Keys: s toggles the stabilizer (stabilizer.py) on/off, q quits.
"""

from __future__ import annotations

import argparse
import collections
import contextlib
import math
import os
import sys
import time

import cv2
import mediapipe as mp
import numpy as np

from handtrace import CONNECTIONS
from record import ensure_model, open_camera
from stabilizer import HandStabilizer, from_mediapipe

TICK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")


def cpu_seconds() -> float:
    with open("/proc/self/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / TICK  # utime + stime


def rss_mib() -> float:
    with open("/proc/self/statm") as f:
        return int(f.read().split()[1]) * PAGE / 2**20


EDGE_ON = 0.25  # |cos| of yaw below this (hand within ~15 deg of edge-on) reads as "edge"


def hand_angles(world, label: str) -> tuple[float, float, float, str]:
    """(pitch, roll, yaw) in degrees from world landmarks (metres, y down). 0,0,0 = hand upright, palm flat to camera.

    roll  = fingers tilting left/right in the image plane (+ = clockwise as seen)
    pitch = fingers tilting toward (+) / away from the camera
    yaw   = hand turning about its own long axis, full -180..180: index side toward (+) / away (-);
            +-180 = back of the hand to the camera. Needs the Left/Right label to know which way is palm.
    facing = "palm", "back" or "edge".
    """
    v = lambda i: np.array(world[i], dtype=float)
    up = v(9) - v(0)                      # wrist -> middle knuckle
    up /= np.linalg.norm(up)
    across = v(5) - v(17)                 # pinky knuckle -> index knuckle
    across -= up * across.dot(up)         # make it perpendicular to `up`
    across /= np.linalg.norm(across)
    roll = math.degrees(math.atan2(up[0], -up[1]))
    pitch = math.degrees(math.atan2(-up[2], math.hypot(up[0], up[1])))
    # In the mirrored image a palm-on Right hand has its index on the left, a Left hand on the right.
    perp = np.array([-up[1], up[0], 0.0])  # in-image direction 90 deg clockwise from `up`
    ref = -perp if label == "Right" else perp
    front = float(across.dot(ref))
    yaw = math.degrees(math.atan2(-across[2], front))
    facing = "edge" if abs(front) < EDGE_ON else ("palm" if front > 0 else "back")
    return pitch, roll, yaw, facing


def panel(img, lines: list[str]) -> None:
    cv2.rectangle(img, (0, 0), (420, 22 * len(lines) + 12), (0, 0, 0), -1)
    for i, s in enumerate(lines):
        cv2.putText(img, s, (10, 26 + 22 * i), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 0), 1, cv2.LINE_AA)


AXES = (("pitch", (80, 80, 255)), ("roll", (80, 255, 80)), ("yaw", (255, 160, 60)))  # BGR
HISTORY = 300  # samples kept per hand (~10-20 s)


def graph(hist: dict, w: int, h: int) -> np.ndarray:
    """One strip per hand (Left, Right): pitch/roll/yaw over time, -180..+180 deg."""
    img = np.full((h, w, 3), 25, np.uint8)
    strip = h // 2
    for row, label in enumerate(("Left", "Right")):
        y0 = row * strip
        mid = y0 + strip // 2
        for deg in (-180, -90, 0, 90, 180):
            y = int(mid - deg / 180 * (strip / 2 - 8))
            cv2.line(img, (40, y), (w, y), (70, 70, 70) if deg else (130, 130, 130), 1)
            cv2.putText(img, f"{deg:+d}", (2, y + 4), cv2.FONT_HERSHEY_SIMPLEX, 0.35, (150, 150, 150), 1)
        cv2.putText(img, label, (48, y0 + 18), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (230, 230, 230), 1, cv2.LINE_AA)
        samples = hist[label]
        facing = next((s[3] for s in reversed(samples) if s is not None), "")
        cv2.putText(img, facing.upper(), (48, y0 + 40), cv2.FONT_HERSHEY_SIMPLEX, 0.5,
                    {"palm": (80, 255, 80), "back": (80, 80, 255)}.get(facing, (200, 200, 200)), 1, cv2.LINE_AA)
        for k, (name, colour) in enumerate(AXES):
            last = next((s[k] for s in reversed(samples) if s is not None), None)
            txt = f"{name} {last:+4.0f}" if last is not None else f"{name}  --"
            cv2.putText(img, txt, (130 + 120 * k, y0 + 18), cv2.FONT_HERSHEY_SIMPLEX, 0.55, colour, 1, cv2.LINE_AA)
            prev = None
            for i, s in enumerate(samples):
                if s is None:
                    prev = None
                    continue
                x = 40 + int(i * (w - 41) / (HISTORY - 1))
                y = int(mid - s[k] / 180 * (strip / 2 - 8))
                if prev is not None and abs(y - prev[1]) < (strip - 16) / 2:  # don't draw the +-180 wrap
                    cv2.line(img, prev, (x, y), colour, 2, cv2.LINE_AA)
                prev = (x, y)
        cv2.line(img, (0, y0 + strip - 1), (w, y0 + strip - 1), (90, 90, 90), 1)
    return img


class NullProbe:
    """Instrumentation hooks. live.py runs with this no-op; bench.py passes a measuring probe."""
    _ctx = contextlib.nullcontext()

    def stage(self, name: str):
        return self._ctx

    def frame(self, view: np.ndarray, hands: int, cam: tuple[int, int]) -> bool:
        return True  # False stops the loop


def main(argv=None, probe=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--camera", default="/dev/video0")
    ap.add_argument("--hands", type=int, default=2)
    ap.add_argument("--scale", type=float, default=1.6, help="window size multiplier")
    args = ap.parse_args(argv)
    probe = probe or NullProbe()
    cv2.namedWindow("handOs live", cv2.WINDOW_NORMAL)
    stab, stable_on = HandStabilizer(), True
    hist = {"Left": collections.deque(maxlen=HISTORY), "Right": collections.deque(maxlen=HISTORY)}

    cap = open_camera(args.camera)
    opts = mp.tasks.vision.HandLandmarkerOptions(
        base_options=mp.tasks.BaseOptions(model_asset_path=str(ensure_model())),
        running_mode=mp.tasks.vision.RunningMode.VIDEO, num_hands=args.hands)
    inf = collections.deque(maxlen=120)
    stamps = collections.deque(maxlen=60)
    cpu_pct, last_cpu, last_wall = 0.0, cpu_seconds(), time.monotonic()
    t0, last_ts = time.monotonic(), -1
    with mp.tasks.vision.HandLandmarker.create_from_options(opts) as lmk:
        while True:
            with probe.stage("camera"):
                ok, bgr = cap.read()
            if not ok:
                print("camera read failed", file=sys.stderr)
                return 1
            now = time.monotonic()
            stamps.append(now)
            with probe.stage("preprocess"):
                bgr = cv2.flip(bgr, 1)
                rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
                image = mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb)
            ts = max(int((now - t0) * 1000), last_ts + 1)
            last_ts = ts
            ti = time.perf_counter()
            with probe.stage("inference"):
                res = lmk.detect_for_video(image, ts)
            inf.append((time.perf_counter() - ti) * 1000)

            with probe.stage("stabilizer"):
                raw = from_mediapipe(res)
                stable = stab.update(raw, now)  # always run, so tracks stay warm when toggled back on
                hands = ([(hd.label, hd.score, hd.lm, hd.world, f"#{hd.id} raw {hd.raw_label}") for hd in stable]
                         if stable_on else [(o.label, o.score, o.lm, o.world, "") for o in raw])
            with probe.stage("angles"):
                angles = [hand_angles(wl, label) for label, _, _, wl, _ in hands]

            with probe.stage("overlay"):
                if now - last_wall >= 1.0:  # refresh CPU once a second
                    c = cpu_seconds()
                    cpu_pct = (c - last_cpu) / (now - last_wall) * 100
                    last_cpu, last_wall = c, now
                h, w = bgr.shape[:2]
                hand_lines = []
                seen = {}
                for (label, score, lm, _, extra), (pitch, roll, yaw, facing) in zip(hands, angles):
                    pts = [(int(p[0] * w), int(p[1] * h)) for p in lm]
                    for a, b in CONNECTIONS:
                        cv2.line(bgr, pts[a], pts[b], (255, 255, 255), 2)
                    for p in pts:
                        cv2.circle(bgr, p, 3, (0, 200, 255), -1)
                    seen[label] = (pitch, roll, yaw, facing)
                    hand_lines.append(f"  {label:<5} score {score:.2f}  {facing.upper()}  {extra}")
                    hand_lines.append(f"    pitch {pitch:+4.0f}  roll {roll:+4.0f}  yaw {yaw:+4.0f}")

                fps = (len(stamps) - 1) / (stamps[-1] - stamps[0]) if len(stamps) > 1 else 0.0
                p95 = sorted(inf)[int(len(inf) * 0.95) - 1] if len(inf) > 1 else inf[-1]
                panel(bgr, [
                    f"camera {fps:5.1f} fps  ({w}x{h})",
                    f"infer  {inf[-1]:5.1f} ms  avg {sum(inf) / len(inf):.1f}  p95 {p95:.1f}",
                    f"cpu    {cpu_pct:5.1f} % of one core ({os.cpu_count()} cores)",
                    f"ram    {rss_mib():5.0f} MiB RSS",
                    f"load   {os.getloadavg()[0]:.2f}",
                    f"mode   {'STABILIZED' if stable_on else 'RAW'}   (s toggles, q quits)",
                    f"hands  {len(hands)}",
                    *hand_lines,
                ])
            with probe.stage("graph"):
                for label, dq in hist.items():
                    dq.append(seen.get(label))
                strip = graph(hist, w, 300)
            with probe.stage("compose"):
                view = np.vstack([bgr, strip])
                view = cv2.resize(view, None, fx=args.scale, fy=args.scale, interpolation=cv2.INTER_LINEAR)
            if not probe.frame(view, len(hands), (w, h)):
                break
            with probe.stage("display"):
                cv2.imshow("handOs live", view)
                key = cv2.waitKey(1) & 0xFF
            if key == ord("q"):
                break
            if key == ord("s"):
                stable_on = not stable_on
    cap.release()
    cv2.destroyAllWindows()
    return 0


if __name__ == "__main__":
    sys.exit(main())
