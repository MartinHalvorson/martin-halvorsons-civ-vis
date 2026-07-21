from .basic_ai import BasicAI
from .random_ai import RandomAI


def make_ai(name, seed=0):
    if name == "random":
        return RandomAI(seed=seed)
    return BasicAI(seed=seed)
