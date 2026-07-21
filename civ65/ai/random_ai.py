"""Uniform-random baseline agent."""
import random

from ..game import IllegalAction


class RandomAI:
    def __init__(self, seed=0):
        self.rng = random.Random(seed)

    def take_turn(self, game, pid):
        for _ in range(60):
            acts = [a for a in game.legal_actions(pid) if a["type"] != "end_turn"]
            if not acts:
                break
            try:
                game.apply(pid, self.rng.choice(acts))
            except IllegalAction:
                pass
            if game.winner is not None:
                break
        if game.winner is None and game.current == pid:
            game.apply(pid, {"type": "end_turn"})
