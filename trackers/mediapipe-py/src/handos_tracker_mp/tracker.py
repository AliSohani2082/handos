import math
import select
import socket
import time
from contextlib import ExitStack

from .model import ensure_model
from .protocol import MAX_PACKET, PROTOCOL, Hand, decode_control, encode_control, encode_frame
from .tracks import Associate


def anatomical_handedness(category: str) -> int:
    # The live camera check confirmed anatomical labels for raw images.
    return {"Left": 0, "Right": 1}[category]


def send(sock: socket.socket, packet: bytes) -> bool:
    try:
        return sock.send(packet) == len(packet)
    except BlockingIOError:
        return False


def run(path: str, device: str) -> int:
    import cv2
    import mediapipe as mp

    from .camera import FPS, Camera

    with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as sock, ExitStack() as resources:
        sock.connect(path)
        sock.setblocking(False)
        camera = None
        mode, fps = "active", FPS
        seq, timestamp_ms, dropped = 0, -1, 0
        tracks = Associate()
        next_frame = last_report = time.monotonic()
        frames = infer_ms = 0
        try:
            options = mp.tasks.vision.HandLandmarkerOptions(
                base_options=mp.tasks.BaseOptions(model_asset_path=str(ensure_model())),
                running_mode=mp.tasks.vision.RunningMode.VIDEO,
                num_hands=2,
            )
            detector = resources.enter_context(mp.tasks.vision.HandLandmarker.create_from_options(options))
            camera = Camera(device)
            send(sock, encode_control({"type": "hello", "protocol": PROTOCOL, "name": "mediapipe-py",
                                       "max_hands": 2, "world": True, "camera": camera.info}))

            while True:
                try:
                    packet = sock.recv(MAX_PACKET + 1)
                except BlockingIOError:
                    packet = None
                if packet == b"":
                    return 0
                if packet:
                    msg = decode_control(packet)
                    if msg.get("type") == "shutdown":
                        return 0
                    if msg.get("type") != "set" or msg.get("mode") not in ("active", "idle", "off"):
                        raise ValueError("invalid tracker command")
                    mode = msg["mode"]
                    fps = msg.get("fps", 5 if mode == "idle" else FPS)
                    if isinstance(fps, bool) or not isinstance(fps, (int, float)) or not math.isfinite(fps):
                        raise ValueError("invalid tracker rate")
                    if not 0 < fps <= FPS:
                        raise ValueError(f"tracker rate must be in (0, {FPS}]")
                    next_frame = time.monotonic()

                now = time.monotonic()
                if now - last_report >= 1.0:
                    send(sock, encode_control({"type": "status", "fps": frames / (now - last_report),
                                               "infer_ms": infer_ms, "dropped": dropped + (
                                                   camera.dropped if camera is not None else 0)}))
                    last_report, frames = now, 0

                if mode == "off":
                    if camera is not None:
                        camera.close()
                        dropped += camera.dropped
                        camera = None
                        tracks.tracks.clear()
                    select.select([sock], [], [], 0.1)
                    continue
                if now < next_frame:
                    select.select([sock], [], [], min(0.1, next_frame - now))
                    continue
                if camera is None:
                    camera = Camera(device)
                capture = camera.take(0.1)
                if capture is None:
                    continue
                capture_us, bgr = capture
                next_frame = time.monotonic() + 1 / fps
                timestamp_ms = max(timestamp_ms + 1, capture_us // 1000)
                rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
                start = time.perf_counter()
                result = detector.detect_for_video(mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb), timestamp_ms)
                infer_ms = (time.perf_counter() - start) * 1000
                points = [[[p.x, p.y, p.z] for p in hand] for hand in result.hand_landmarks]
                hands = [
                    Hand(track_id, anatomical_handedness(label[0].category_name), label[0].score, image,
                         [[p.x, p.y, p.z] for p in world])
                    for track_id, label, image, world in zip(
                        tracks.update(points), result.handedness, points, result.hand_world_landmarks, strict=True)
                ]
                emit_us = time.monotonic_ns() // 1000
                if not send(sock, encode_frame(seq, capture_us, emit_us, hands)):
                    dropped += 1
                frames += 1
                seq = (seq + 1) & 0xFFFFFFFF
        except (OSError, ValueError, TypeError, RuntimeError, cv2.error, KeyError) as exc:
            try:
                send(sock, encode_control({"type": "status", "fps": 0, "infer_ms": 0,
                                           "dropped": dropped, "error": str(exc)}))
            except OSError:
                pass
            raise
        finally:
            if camera is not None:
                camera.close()
