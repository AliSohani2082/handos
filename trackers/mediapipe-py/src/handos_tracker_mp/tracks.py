from itertools import count
from math import dist

PALM = (0, 5, 9, 13, 17)


class Associate:
    """Keep short-lived identities by matching the nearest pair of palm centres first."""

    def __init__(self, max_jump: float = 0.25, max_missed: int = 10):
        self.max_jump, self.max_missed = max_jump, max_missed
        self.ids = count(1)
        self.tracks = {}

    def update(self, landmarks: list[list]) -> list[int]:
        centres = [tuple(sum(hand[i][axis] for i in PALM) / len(PALM) for axis in (0, 1))
                   for hand in landmarks]
        pairs = sorted((dist(centre, prev), i, track_id)
                       for i, centre in enumerate(centres)
                       for track_id, (prev, _) in self.tracks.items())
        assigned = [0] * len(centres)
        used = set()
        for distance, i, track_id in pairs:
            if distance > self.max_jump:
                break
            if not assigned[i] and track_id not in used:
                assigned[i] = track_id
                used.add(track_id)
        self.tracks = {track_id: (centre, missed + 1)
                       for track_id, (centre, missed) in self.tracks.items()
                       if track_id not in used and missed < self.max_missed}
        for i, centre in enumerate(centres):
            if not assigned[i]:
                assigned[i] = next(self.ids)
            self.tracks[assigned[i]] = (centre, 0)
        return assigned
