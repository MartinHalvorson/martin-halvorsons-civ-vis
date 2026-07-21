"""Civ 6 style combat math."""
import math


def effective_strength(base, hp):
    """Strength drops 1 point per 10 HP lost (Civ 6 rule)."""
    return max(1.0, base - (100 - hp) / 10.0)


def damage(att_str, def_str, rng):
    """Civ 6 damage curve: 30 * e^(diff/25), +-20% random."""
    diff = att_str - def_str
    dmg = 30.0 * math.exp(diff / 25.0) * rng.uniform(0.8, 1.2)
    return max(1, min(100, int(round(dmg))))
