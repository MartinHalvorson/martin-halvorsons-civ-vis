import json

from civvis.game import Game


def test_map_dimensions_and_land_fraction():
    g = Game(num_players=2, width=20, height=14, seed=42)
    assert len(g.map.tiles) == 20 * 14
    land = sum(1 for t in g.map.tiles.values()
               if not g.rules.is_water(t))
    frac = land / len(g.map.tiles)
    assert 0.25 < frac < 0.75


def test_starting_units():
    g = Game(num_players=3, width=24, height=16, seed=7)
    for p in g.players:
        types = sorted(u.type for u in g.player_units(p.id))
        assert types == ["settler", "warrior"]


def test_determinism_same_seed():
    a = Game(num_players=2, width=18, height=12, seed=99)
    b = Game(num_players=2, width=18, height=12, seed=99)
    assert json.dumps(a.map.to_dict()) == json.dumps(b.map.to_dict())


def test_spawns_are_spread_out():
    from civvis import hexgrid
    g = Game(num_players=2, width=24, height=16, seed=3)
    spawns = [u.pos for u in g.units.values() if u.type == "settler"]
    assert len(spawns) == 2
    assert hexgrid.distance(spawns[0], spawns[1]) >= 4
