import pytest

from civ65.game import Game, IllegalAction, growth_threshold


def both_end_turn(g, times=1):
    for _ in range(times):
        for _ in range(len(g.players)):
            if g.winner is not None:
                return
            g.apply(g.current, {"type": "end_turn"})


def find_settler(g, pid):
    return next(u for u in g.player_units(pid) if u.type == "settler")


def test_growth_threshold_curve():
    assert growth_threshold(1) == 15
    assert growth_threshold(2) == 24
    assert growth_threshold(3) > growth_threshold(2)


def test_found_city_and_produce():
    g = Game(num_players=2, width=20, height=14, seed=5)
    s = find_settler(g, 0)
    g.apply(0, {"type": "found_city", "unit": s.id})
    cities = g.player_cities(0)
    assert len(cities) == 1
    city = cities[0]
    assert city.is_capital
    assert len(city.owned_tiles) >= 5
    g.apply(0, {"type": "produce", "city": city.id, "item": {"unit": "scout"}})
    g.apply(0, {"type": "research", "tech": "pottery"})
    city.production = 500  # fast-forward
    n_before = len(g.player_units(0))
    both_end_turn(g, 2)
    assert len(g.player_units(0)) > n_before


def test_illegal_actions_raise():
    g = Game(num_players=2, width=20, height=14, seed=5)
    with pytest.raises(IllegalAction):
        g.apply(1, {"type": "end_turn"})  # not their turn
    with pytest.raises(IllegalAction):
        g.apply(0, {"type": "research", "tech": "education"})  # prereqs missing
    s = find_settler(g, 0)
    with pytest.raises(IllegalAction):
        g.apply(0, {"type": "move", "unit": s.id, "to": [999, 999]})


def test_legal_actions_only_for_current_player():
    g = Game(num_players=2, width=20, height=14, seed=5)
    assert g.legal_actions(1) == []
    acts = g.legal_actions(0)
    assert {"type": "end_turn"} in acts
    assert any(a["type"] == "found_city" for a in acts)
    assert any(a["type"] == "research" for a in acts)
