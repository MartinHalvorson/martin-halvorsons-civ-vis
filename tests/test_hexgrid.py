from civ65 import hexgrid


def test_distance_symmetric():
    a, b = (0, 0), (3, -2)
    assert hexgrid.distance(a, b) == hexgrid.distance(b, a) == 3


def test_neighbors_are_distance_one():
    for n in hexgrid.neighbors((2, 5)):
        assert hexgrid.distance((2, 5), n) == 1
    assert len(set(hexgrid.neighbors((2, 5)))) == 6


def test_disk_sizes():
    assert len(hexgrid.disk((0, 0), 1)) == 7
    assert len(hexgrid.disk((4, -1), 2)) == 19


def test_offset_roundtrip():
    for col in range(6):
        for row in range(6):
            q, r = hexgrid.offset_to_axial(col, row)
            assert hexgrid.axial_to_offset(q, r) == (col, row)
