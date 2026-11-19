import argparse
import json
import os
import sys
from pathlib import Path


def main() -> int:
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    parser = argparse.ArgumentParser(description="MediaPipe tracker for handOs")
    parser.add_argument("--socket", default=Path(runtime) / "handos/tracker.sock" if runtime else None)
    parser.add_argument("--camera", default="/dev/video0")
    parser.add_argument("--probe", action="store_true")
    args = parser.parse_args()
    if not args.probe and args.socket is None:
        parser.error("set XDG_RUNTIME_DIR or pass --socket")
    try:
        if args.probe:
            from .camera import probe

            print(json.dumps(probe(args.camera)))
            return 0
        from .tracker import run

        return run(str(args.socket), args.camera)
    except Exception as exc:  # noqa: BLE001 - report camera and inference errors at the CLI boundary
        print(f"handos-tracker-mp: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
