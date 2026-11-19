"""MediaPipe tracker for handOs."""
import os

# V4L2 ignores CAP_PROP_READ_TIMEOUT_MSEC (set() returns False on OpenCV 5.0), so bound a stalled
# camera read with the backend's select timeout instead: 1 s rather than the default 10 s. Set
# here so it is in place before any submodule imports cv2.
os.environ.setdefault("OPENCV_VIDEOIO_V4L_SELECT_TIMEOUT", "1")
