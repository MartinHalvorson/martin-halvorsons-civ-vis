from civ65.ai import make_ai
from civ65.cli import run_game
from civ65.game import Game


def test_basic_ai_selfplay_progresses():
    g = Game(num_players=2, width=20, height=14, seed=11, max_turns=60)
    ais = {p.id: make_ai("basic", seed=p.id) for p in g.players}
    run_game(g, ais, verbose=False)
    assert g.winner is not None  # score victory at worst
    assert len(g.cities) >= 2
    assert all(len(p.techs) > 1 for p in g.players)


def test_random_ai_selfplay_no_crash():
    g = Game(num_players=2, width=16, height=12, seed=2, max_turns=30)
    ais = {p.id: make_ai("random", seed=p.id) for p in g.players}
    run_game(g, ais, verbose=False)
    assert g.winner is not None


def test_full_game_reaches_victory():
    g = Game(num_players=3, width=24, height=16, seed=8, max_turns=120)
    ais = {p.id: make_ai("basic", seed=p.id) for p in g.players}
    run_game(g, ais, verbose=False)
    assert g.winner is not None
    assert g.victory_type in ("domination", "science", "score")
