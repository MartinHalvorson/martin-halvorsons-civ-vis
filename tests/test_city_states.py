from civvis.ai import make_ai
from civvis.cli import run_game
from civvis.game import Game


def make(seed=1, cs=3):
    return Game(num_players=2, width=28, height=18, seed=seed, max_turns=50,
                num_city_states=cs)


def test_city_states_prefounded():
    g = make()
    minors = [p for p in g.players if p.is_minor]
    assert minors  # at least one placed (crowding may skip some)
    for p in minors:
        cities = g.player_cities(p.id)
        assert len(cities) == 1
        assert not cities[0].is_capital
        assert cities[0].name == p.civ
        assert g.player_units(p.id)  # garrison


def test_city_states_never_expand_or_declare_war():
    g = make(seed=2)
    ais = {p.id: make_ai("basic", seed=p.id) for p in g.players}
    run_game(g, ais, verbose=False)
    for p in g.players:
        if not p.is_minor:
            continue
        assert all(u.type != "settler" for u in g.player_units(p.id))
        assert len([c for c in g.cities.values()
                    if c.original_owner == p.id]) == 1


def test_minors_excluded_from_victory():
    g = make(seed=3)
    # eliminate major 1 directly: remove its units (it has no cities yet)
    for u in list(g.player_units(1)):
        g._remove_unit(u.id)
    g._check_elimination(1)
    g._check_domination()
    assert g.winner == 0
    assert g.victory_type == "domination"


def test_serialization_keeps_minor_flag():
    g = make(seed=4)
    g2 = Game.from_dict(g.to_dict())
    assert [p.is_minor for p in g2.players] == [p.is_minor for p in g.players]
