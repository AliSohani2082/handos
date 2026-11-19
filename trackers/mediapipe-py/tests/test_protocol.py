from pathlib import Path

import pytest

from handos_tracker_mp.protocol import Hand, decode_control, encode_control, encode_frame

GOLDEN = Path(__file__).resolve().parents[3] / "crates/handos-proto/tests/golden/two_hands.bin"


def test_matches_rust_golden_packet():
    points = [[(i * 3 + axis) / 100 for axis in range(3)] for i in range(21)]
    world = [[-v for v in point] for point in points]
    hands = [Hand(10 + side, side, 0.875, points, world) for side in range(2)]
    assert encode_frame(7, 1_000_000, 1_027_000, hands) == GOLDEN.read_bytes()


def test_control_message():
    msg = {"type": "set", "mode": "idle", "fps": 5}
    assert decode_control(encode_control(msg)) == msg
    with pytest.raises(TypeError):
        decode_control(b"\x02[]")
