"""civ65 — open-source, headless-first strategy engine inspired by Civilization VI."""
from .game import Game, IllegalAction
from .rules import Ruleset
from .env import CivEnv

__version__ = "0.1.0"
__all__ = ["Game", "IllegalAction", "Ruleset", "CivEnv", "__version__"]
