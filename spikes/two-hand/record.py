"""Spike 1 recorder: guided reps in front of the laptop webcam, saved as landmarks (and video).

  uv run record.py --task palm  --light good
  uv run record.py --task pluck --light good
  uv run record.py --task palm  --light dim
  uv run record.py --task pluck --light dim
  uv run record.py --task click --light good    # M1 Task B10: 50 pinch clicks, one hand

Each session: 3 s countdown, a hold phase (both hands still, palms to the camera), then N reps.
A rep is 1 s "READY" (hands apart) and 2 s "TOUCH"/"PLUCK" in which you do the gesture once.
Press q in the preview window to stop early. Output goes to data/<task>-<light>-<time>.jsonl (+ .mp4).
"""

from __future__ import annotations

import argparse
import sys
import time
import urllib.request
from pathlib import Path

import cv2
import mediapipe as mp

import guide
from handtrace import CONNECTIONS, Frame, Hand, Rep, TraceWriter

HERE = Path(__file__).parent
MODEL_URL = ("https://storage.googleapis.com/mediapipe-models/hand_landmarker/"
             "hand_landmarker/float16/latest/hand_landmarker.task")
MODEL_PATH = HERE / "models" / "hand_landmarker.task"
WIDTH, HEIGHT, FPS = 640, 480, 30
COUNTDOWN_S = 3.0


def ensure_model() -> Path:
    if not MODEL_PATH.exists():
        MODEL_PATH.parent.mkdir(parents=True, exist_ok=True)
        print(f"downloading {MODEL_URL}", file=sys.stderr)
        tmp = MODEL_PATH.with_suffix(".part")
        urllib.request.urlretrieve(MODEL_URL, tmp)
        tmp.rename(MODEL_PATH)
    return MODEL_PATH


def open_camera(device: str) -> cv2.VideoCapture:
    cap = cv2.VideoCapture(device, cv2.CAP_V4L2)
    cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, WIDTH)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, HEIGHT)
    cap.set(cv2.CAP_PROP_FPS, FPS)
    cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
    if not cap.isOpened():
        sys.exit(f"cannot open camera {device}")
    return cap


def phase_at(el: float, hold_s: float, rep_s: float, reps: int) -> tuple[str, int]:
    """Which prompt to show `el` seconds after start: (phase, rep index)."""
    if el < COUNTDOWN_S:
        return "countdown", -1
    el -= COUNTDOWN_S
    if el < hold_s:
        return "hold", -1
    el -= hold_s
    k = int(el // rep_s)
    if k >= reps:
        return "done", reps
    return ("ready" if (el - k * rep_s) < rep_s / 3 else "go"), k


def draw(frame, hands: list[Hand], prompt: str, colour) -> None:
    h, w = frame.shape[:2]
    for hand in hands:
        pts = [(int(p[0] * w), int(p[1] * h)) for p in hand.lm]
        for a, b in CONNECTIONS:
            cv2.line(frame, pts[a], pts[b], (255, 255, 255), 2)
        for p in pts:
            cv2.circle(frame, p, 3, (0, 200, 255), -1)
        cv2.putText(frame, hand.label, (pts[0][0] - 20, pts[0][1] + 25), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (255, 255, 0), 2)
    cv2.rectangle(frame, (0, 0), (w, 60), (0, 0, 0), -1)
    cv2.putText(frame, prompt, (12, 42), cv2.FONT_HERSHEY_SIMPLEX, 1.1, colour, 3)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--task", choices=["palm", "pluck", "click"], required=True)
    ap.add_argument("--light", choices=["good", "dim"], required=True)
    ap.add_argument("--reps", type=int, default=50)
    ap.add_argument("--rep-seconds", type=float, default=3.0)
    ap.add_argument("--hold-seconds", type=float, default=4.0)
    ap.add_argument("--mover", choices=["right", "left"], default="right", help="which hand moves; the other stays still")
    ap.add_argument("--camera", default="/dev/video0")
    ap.add_argument("--out", type=Path, default=HERE / "data")
    ap.add_argument("--no-preview", action="store_true")
    ap.add_argument("--no-video", action="store_true")
    args = ap.parse_args(argv)

    print("\n".join(guide.INSTRUCTIONS[args.task]))
    print("Preview the animation first with: uv run guide.py --task", args.task, "\n")
    model = ensure_model()
    args.out.mkdir(parents=True, exist_ok=True)
    stem = args.out / f"{args.task}-{args.light}-{time.strftime('%Y%m%d-%H%M%S')}"
    cap = open_camera(args.camera)
    real_w, real_h = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH)), int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    video = None if args.no_video else cv2.VideoWriter(str(stem) + ".mp4", cv2.VideoWriter_fourcc(*"mp4v"), FPS, (real_w, real_h))

    options = mp.tasks.vision.HandLandmarkerOptions(
        base_options=mp.tasks.BaseOptions(model_asset_path=str(model)),
        running_mode=mp.tasks.vision.RunningMode.VIDEO,
        num_hands=2,
    )
    with open(str(stem) + ".jsonl", "w") as fp, mp.tasks.vision.HandLandmarker.create_from_options(options) as lmk:
        out = TraceWriter(fp)
        out.header(task=args.task, light=args.light, width=real_w, height=real_h, fps=FPS, mirrored=True,
                   camera=args.camera, reps=args.reps, rep_seconds=args.rep_seconds,
                   hold_seconds=args.hold_seconds, model=MODEL_URL, recorded=time.strftime("%Y-%m-%dT%H:%M:%S"))
        t0 = time.monotonic()
        last_ts = -1
        written_hold = False
        rep_k = -1
        while True:
            ok, bgr = cap.read()
            now = time.monotonic()
            if not ok:
                print("camera read failed", file=sys.stderr)
                break
            bgr = cv2.flip(bgr, 1)  # selfie view: MediaPipe's handedness assumes a mirrored image
            t_ms = (now - t0) * 1000.0
            ts = max(int(t_ms), last_ts + 1)
            last_ts = ts
            rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
            ti = time.perf_counter()
            res = lmk.detect_for_video(mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb), ts)
            inf_ms = (time.perf_counter() - ti) * 1000.0
            hands = [
                Hand(res.handedness[i][0].category_name, res.handedness[i][0].score,
                     [[p.x, p.y, p.z] for p in res.hand_landmarks[i]],
                     [[p.x, p.y, p.z] for p in res.hand_world_landmarks[i]])
                for i in range(len(res.hand_landmarks))
            ]
            out.frame(Frame(t_ms, hands, inf_ms))
            if video is not None:
                video.write(bgr)

            el = now - t0
            phase, k = phase_at(el, args.hold_seconds, args.rep_seconds, args.reps)
            if phase not in ("countdown", "hold") and not written_hold:
                out.hold(COUNTDOWN_S * 1000, (COUNTDOWN_S + args.hold_seconds) * 1000)
                written_hold = True
            if k != rep_k and rep_k >= 0:  # the previous rep has closed
                start = (COUNTDOWN_S + args.hold_seconds + rep_k * args.rep_seconds + args.rep_seconds / 3) * 1000
                out.rep(Rep(rep_k, args.task, start, start + args.rep_seconds * 2 / 3 * 1000))
            rep_k = k
            if phase == "done":
                break

            if not args.no_preview:
                prompt, colour = {
                    "countdown": (f"Get ready... {COUNTDOWN_S - el:.0f}", (200, 200, 200)),
                    "hold": ("HOLD STILL, palms to camera", (0, 255, 255)),
                    "ready": (f"{k + 1}/{args.reps}  {guide.PROMPTS[args.task]['ready']}", (200, 200, 200)),
                    "go": (f"{k + 1}/{args.reps}  {guide.PROMPTS[args.task]['go']}", (0, 255, 0)),
                }[phase]
                rep_t = el - COUNTDOWN_S - args.hold_seconds - max(k, 0) * args.rep_seconds
                guide.draw(bgr, args.task, phase, rep_t, args.mover, args.rep_seconds)
                draw(bgr, hands, prompt, colour)
                cv2.imshow("handOs spike 1", bgr)
                if cv2.waitKey(1) & 0xFF == ord("q"):
                    break
    cap.release()
    if video is not None:
        video.release()
    cv2.destroyAllWindows()
    print(f"wrote {stem}.jsonl" + ("" if args.no_video else f" and {stem}.mp4"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
