import hashlib
import shutil
import tempfile
import urllib.request
from pathlib import Path

MODEL_URL = ("https://storage.googleapis.com/mediapipe-models/hand_landmarker/"
             "hand_landmarker/float16/latest/hand_landmarker.task")
MODEL_SHA256 = "fbc2a30080c3c557093b5ddfc334698132eb341044ccee322ccf8bcf3607cde1"


def verify(path: Path) -> None:
    with path.open("rb") as model:
        if hashlib.file_digest(model, "sha256").hexdigest() != MODEL_SHA256:
            raise ValueError(f"model SHA-256 mismatch at {path}; check the source and pinned hash")


def ensure_model() -> Path:
    path = Path.home() / ".local/share/handos/models/hand_landmarker.task"
    if path.exists():
        verify(path)
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, suffix=".part", delete=False) as target:
        part = Path(target.name)
        try:
            with urllib.request.urlopen(MODEL_URL, timeout=15) as source:
                shutil.copyfileobj(source, target)
            target.close()
            verify(part)
            part.replace(path)
        finally:
            part.unlink(missing_ok=True)
    return path
