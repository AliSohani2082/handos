from itertools import count
from math import dist

PALM = (0, 5, 9, 13, 17)


class Associate:
    """Keep short-lived identities by matching palm centres to the previous ones.

    Picks the assignment that keeps the most tracks within `max_jump`, then the smallest total
    distance. Greedy nearest-first matching would let one hand steal the other's track when both
    move the same way. The search is exhaustive, which is cheap for the two hands the tracker reports.
    """

    def __init__(self, max_jump: float = 0.25, max_missed: int = 10):
        self.max_jump, self.max_missed = max_jump, max_missed
        self.ids = count(1)
        self.tracks = {}

    def _match(self, centres: list[tuple]) -> list[int]:
        near = [[(track_id, d) for track_id, (prev, _) in self.tracks.items()
                 if (d := dist(centre, prev)) <= self.max_jump]
                for centre in centres]
        best = ([0] * len(centres), 0, 0.0)  # assignment, matches, total distance

        def search(i, assigned, used, matches, total):
            nonlocal best
            if i == len(centres):
                if matches > best[1] or (matches == best[1] and total < best[2]):
                    best = (assigned.copy(), matches, total)
                return
            for track_id, d in near[i]:
                if track_id not in used:
                    assigned.append(track_id)
                    used.add(track_id)
                    search(i + 1, assigned, used, matches + 1, total + d)
                    used.remove(track_id)
                    assigned.pop()
            assigned.append(0)
            search(i + 1, assigned, used, matches, total)
            assigned.pop()

        search(0, [], set(), 0, 0.0)
        return best[0]

    def update(self, landmarks: list[list]) -> list[int]:
        centres = [tuple(sum(hand[i][axis] for i in PALM) / len(PALM) for axis in (0, 1))
                   for hand in landmarks]
        assigned = self._match(centres)
        used = set(assigned) - {0}
        self.tracks = {track_id: (centre, missed + 1)
                       for track_id, (centre, missed) in self.tracks.items()
                       if track_id not in used and missed < self.max_missed}
        for i, centre in enumerate(centres):
            if not assigned[i]:
                assigned[i] = next(self.ids)
            self.tracks[assigned[i]] = (centre, 0)
        return assigned
