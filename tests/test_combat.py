import random

from civvis.combat import damage, effective_strength


def test_effective_strength_penalty():
    assert effective_strength(20, 100) == 20
    assert effective_strength(20, 50) == 15
    assert effective_strength(5, 10) == 1.0  # floor


def test_damage_monotonic_in_strength_diff():
    rng = random.Random(0)
    strong = sum(damage(35, 20, rng) for _ in range(200)) / 200
    rng = random.Random(0)
    weak = sum(damage(20, 35, rng) for _ in range(200)) / 200
    assert strong > weak
    assert 1 <= weak <= 100


def test_damage_bounds():
    rng = random.Random(1)
    for _ in range(100):
        assert 1 <= damage(100, 1, rng) <= 100
        assert 1 <= damage(1, 100, rng) <= 100
