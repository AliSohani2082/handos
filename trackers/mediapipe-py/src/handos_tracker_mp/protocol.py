"""Byte layout shared with handos-proto."""
import json
import math
import struct
from dataclasses import dataclass
from itertools import chain

PROTOCOL, MSG_FRAME, MSG_CONTROL, FLAG_WORLD = 1, 1, 2, 1
MAX_PACKET = 65_536
HEADER = struct.Struct("<BBBBxIQQ")
HAND = struct.Struct("<IB3xff")
POINTS = struct.Struct("<63f")


@dataclass
class Hand:
    track_id: int
    handedness: int
    score: float
    landmarks: list
    world: list | None
    presence: float = 1.0


def encode_frame(seq: int, capture_us: int, emit_us: int, hands: list[Hand], *, world: bool = True) -> bytes:
    if len(hands) > 4:
        raise ValueError("maximum four hands per frame")
    packet = bytearray(HEADER.pack(MSG_FRAME, PROTOCOL, FLAG_WORLD if world else 0, len(hands),
                                   seq, capture_us, emit_us))
    for hand in hands:
        if hand.handedness not in (0, 1) or (hand.world is not None) != world:
            raise ValueError("invalid handedness or world landmarks")
        if not all(map(math.isfinite, (hand.score, hand.presence))):
            raise ValueError("invalid handedness score or presence")
        packet.extend(HAND.pack(hand.track_id, hand.handedness, hand.score, hand.presence))
        for points in (hand.landmarks, hand.world):
            if points is None:
                continue
            if len(points) != 21 or any(len(point) != 3 for point in points):
                raise ValueError("expected 21 three-dimensional landmarks")
            values = list(chain.from_iterable(points))
            if not all(map(math.isfinite, values)):
                raise ValueError("non-finite landmark")
            packet.extend(POINTS.pack(*values))
    return bytes(packet)


def encode_control(message: dict) -> bytes:
    packet = bytes([MSG_CONTROL]) + json.dumps(message, separators=(",", ":"), allow_nan=False).encode()
    if len(packet) > MAX_PACKET:
        raise ValueError("oversize control packet")
    return packet


def decode_control(packet: bytes) -> dict:
    if not 1 < len(packet) <= MAX_PACKET or packet[0] != MSG_CONTROL:
        raise ValueError("invalid control packet")
    message = json.loads(packet[1:])
    if not isinstance(message, dict):
        raise TypeError("expected a control message object")
    return message
