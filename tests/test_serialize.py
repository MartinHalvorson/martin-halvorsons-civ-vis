import json

from civ65.ai import make_ai
from civ65.cli import run_game
from civ65.game import Game


def test_roundtrip_after_play():
    g = Game(num_players=2, width=18, height=12, seed=4, max_turns=25)
    ais = {p.id: make_ai("basic", seed=p.id) for p in g.players}
    run_game(g, ais, verbose=False)
    blob = json.dumps(g.to_dict(), sort_keys=True)
    g2 = Game.from_dict(json.loads(blob))
    assert json.dumps(g2.to_dict(), sort_keys=True) == blob


def test_save_load(tmp_path):
    g = Game(num_players=2, width=16, height=12, seed=1)
    path = tmp_path / "save.json"
    g.save(str(path))
    g2 = Game.load(str(path))
    assert g2.turn == g.turn
    assert len(g2.units) == len(g.units)
    assert g2.legal_actions(0)  # still playable
