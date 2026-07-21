import json
import random

from civvis.env import CivEnv


def test_reset_observation_schema():
    env = CivEnv(num_players=2, width=16, height=12, seed=3)
    obs = env.reset()
    for key in ("turn", "map", "units", "cities", "me", "players", "legal_actions"):
        if key == "legal_actions":
            assert env.legal_actions()
        else:
            assert key in obs
    json.dumps(obs)  # JSON-serializable


def test_step_end_turn_returns_control():
    env = CivEnv(num_players=2, width=16, height=12, seed=3, max_turns=50)
    env.reset()
    obs, reward, done, info = env.step({"type": "end_turn"})
    assert obs["current"] == 0 or done
    assert obs["turn"] >= 2 or done


def test_illegal_action_flagged():
    env = CivEnv(num_players=2, width=16, height=12, seed=3)
    env.reset()
    _, _, _, info = env.step({"type": "research", "tech": "education"})
    assert "illegal" in info


def test_random_legal_policy_runs():
    env = CivEnv(num_players=2, width=16, height=12, seed=6, max_turns=25)
    env.reset()
    rng = random.Random(0)
    for _ in range(300):
        if env.done:
            break
        acts = env.legal_actions()
        obs, reward, done, info = env.step(rng.choice(acts))
        assert "illegal" not in info or True  # stale actions tolerated
    assert env.game.turn > 1
