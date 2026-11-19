import time
from threading import Event, Lock, Thread

import cv2

WIDTH, HEIGHT, FPS = 640, 480, 30


class Camera:
    def __init__(self, device: str):
        self.cap = cv2.VideoCapture(device, cv2.CAP_V4L2)
        if not self.cap.isOpened():
            self.cap.release()
            raise OSError(f"cannot open camera {device}")
        self.cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
        self.cap.set(cv2.CAP_PROP_FRAME_WIDTH, WIDTH)
        self.cap.set(cv2.CAP_PROP_FRAME_HEIGHT, HEIGHT)
        self.cap.set(cv2.CAP_PROP_FPS, FPS)
        self.cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
        self.info = {"width": int(self.cap.get(cv2.CAP_PROP_FRAME_WIDTH)),
                     "height": int(self.cap.get(cv2.CAP_PROP_FRAME_HEIGHT)),
                     "fps": self.cap.get(cv2.CAP_PROP_FPS)}
        self.stop = Event()
        self.ready = Event()
        self.lock = Lock()
        self.latest = None
        self.error = None
        self.dropped = 0
        self.thread = Thread(target=self._capture, daemon=True)
        self.thread.start()

    def _capture(self):
        try:
            while not self.stop.is_set():
                ok, image = self.cap.read()
                capture_us = time.monotonic_ns() // 1000
                if not ok:
                    raise OSError("camera stopped delivering frames")
                with self.lock:
                    self.dropped += self.latest is not None
                    self.latest = capture_us, image
                    self.ready.set()
        except (OSError, cv2.error) as exc:
            with self.lock:
                self.error = exc
                self.ready.set()
        finally:
            self.cap.release()

    def take(self, timeout: float):
        if not self.ready.wait(timeout):
            return None
        with self.lock:
            if self.error is not None:
                raise self.error
            latest, self.latest = self.latest, None
            self.ready.clear()
            return latest

    def close(self):
        # A stalled read() returns within the V4L select timeout (1 s, see __init__.py), after
        # which the worker sees `stop` and releases the camera.
        self.stop.set()
        self.thread.join(timeout=3.0)
        if self.thread.is_alive():
            raise OSError("camera capture thread did not stop")


def probe(device: str) -> dict:
    camera = Camera(device)
    try:
        start = time.monotonic()
        count = 0
        brightness = 0.0
        while (remaining := 2.0 - (time.monotonic() - start)) > 0:
            capture = camera.take(remaining)
            if capture is not None:
                count += 1
                brightness += float(capture[1].mean())
        if not count:
            raise OSError("no camera frames in two seconds")
        return {"fps": round(count / (time.monotonic() - start), 2), "brightness": round(brightness / count, 2)}
    finally:
        camera.close()
