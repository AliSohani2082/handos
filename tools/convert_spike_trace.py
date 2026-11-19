"""Convert a mirrored M0 landmark recording to an M1 trace and its windows."""
import argparse
import json
import struct
from pathlib import Path

from handos_tracker_mp.protocol import Hand, encode_frame
from handos_tracker_mp.tracks import Associate


def convert(source: Path, target: Path):
    for output in (target, target.with_suffix(".reps.json")):
        if output.exists() and output.samefile(source):  # also catches symlinks and hard links
            raise ValueError(f"{output}: would overwrite the source recording")
    associate = Associate()
    windows = {"hold": [], "reps": []}
    seq = 0
    with source.open() as old:
        header = json.loads(next(old))
        if header.get("format") != "handos-trace/0" or not header.get("mirrored"):
            raise ValueError(f"{source}: expected a mirrored M0 trace")
        with target.open("wb") as trace:
            trace.write(struct.pack("<4sHHQ", b"HTRC", 1, 0, 0))
            for line in old:
                row = json.loads(line)
                kind = row["type"]
                if kind in ("rep", "hold"):
                    window = {key: value for key, value in row.items() if key in ("i", "task")}
                    for edge in ("start", "end"):
                        window[f"{edge}_us"] = round(row[f"{edge}_ms"] * 1000)
                    windows["reps" if kind == "rep" else "hold"].append(window)
                elif kind == "frame":
                    capture = round(row["t_ms"] * 1000)
                    emit = capture + round((row.get("inf_ms") or 0) * 1000)
                    image = [[[1 - x, y, z] for x, y, z in hand["lm"]] for hand in row["hands"]]
                    hands = [
                        Hand(track_id, {"Left": 1, "Right": 0}[hand["label"]], hand["score"], points,
                             [[-x, y, z] for x, y, z in hand["wl"]])
                        for track_id, points, hand in zip(associate.update(image), image, row["hands"], strict=True)
                    ]
                    packet = encode_frame(seq, capture, emit, hands)
                    trace.write(struct.pack("<I", len(packet)))
                    trace.write(packet)
                    seq += 1
                else:
                    raise ValueError(f"{source}: unexpected record {kind!r}")
    target.with_suffix(".reps.json").write_text(json.dumps(windows, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("target", type=Path)
    args = parser.parse_args()
    convert(args.source, args.target)
