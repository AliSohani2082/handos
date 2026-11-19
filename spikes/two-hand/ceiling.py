"""Tracker throughput and CPU ceiling: replay a recorded session through HandLandmarker with no UI and no encode.

    uv run ceiling.py data/<session>.mp4

Reports frames/s and inference time with frames held in RAM, then process CPU when paced at max, 30 and 15 fps.
"""

import argparse
import os
import statistics
import time

import cv2
import mediapipe as mp

MODEL = "models/hand_landmarker.task"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("video")
    ap.add_argument("--start", type=int, default=1500, help="first frame (mid-session, both hands up)")
    ap.add_argument("--frames", type=int, default=450)
    args = ap.parse_args()

    cap = cv2.VideoCapture(args.video)
    cap.set(cv2.CAP_PROP_POS_FRAMES, args.start)
    rgb = []
    while len(rgb) < args.frames and (ok_frame := cap.read())[0]:
        rgb.append(cv2.cvtColor(ok_frame[1], cv2.COLOR_BGR2RGB))

    opts = mp.tasks.vision.HandLandmarkerOptions(
        base_options=mp.tasks.BaseOptions(model_asset_path=MODEL),
        running_mode=mp.tasks.vision.RunningMode.VIDEO, num_hands=2)
    for fps in (0, 30, 15):
        lmk = mp.tasks.vision.HandLandmarker.create_from_options(opts)
        inf, two = [], 0
        c0, t0 = sum(os.times()[:2]), time.perf_counter()
        for i, frame in enumerate(rgb):
            ts = time.perf_counter()
            res = lmk.detect_for_video(mp.Image(image_format=mp.ImageFormat.SRGB, data=frame), i * 33)
            inf.append((time.perf_counter() - ts) * 1000)
            two += len(res.hand_landmarks) == 2
            if fps:
                time.sleep(max(0.0, 1 / fps - (time.perf_counter() - ts)))
        wall, cpu = time.perf_counter() - t0, sum(os.times()[:2]) - c0
        p95 = statistics.quantiles(inf, n=20)[18]
        print(f"paced {fps or 'max':>3}: {len(rgb) / wall:5.1f} fps  infer p50 {statistics.median(inf):5.1f} ms"
              f"  p95 {p95:5.1f} ms  CPU {cpu / wall * 100:4.0f}% of one core  two hands {two / len(rgb):4.0%}")
        lmk.close()


if __name__ == "__main__":
    main()
