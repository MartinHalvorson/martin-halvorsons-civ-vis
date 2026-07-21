"""Axial hex grid math (pointy-top hexes, odd-r offset for rectangular maps)."""

AXIAL_DIRS = ((1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1))


def neighbors(pos):
    q, r = pos
    return [(q + dq, r + dr) for dq, dr in AXIAL_DIRS]


def distance(a, b):
    dq = a[0] - b[0]
    dr = a[1] - b[1]
    return max(abs(dq), abs(dr), abs(dq + dr))


def disk(center, radius):
    """All axial positions within `radius` of center (inclusive)."""
    q, r = center
    out = []
    for dq in range(-radius, radius + 1):
        for dr in range(max(-radius, -dq - radius), min(radius, -dq + radius) + 1):
            out.append((q + dq, r + dr))
    return out


def offset_to_axial(col, row):
    return (col - ((row - (row & 1)) // 2), row)


def axial_to_offset(q, r):
    return (q + ((r - (r & 1)) // 2), r)
