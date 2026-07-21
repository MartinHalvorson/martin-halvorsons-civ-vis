# AI Development Guide

The engine exists so you can develop advanced AI strategies against a
Civ-6-like game without a UI in the loop.

## CivEnv (gym-style)

```python
from civvis import CivEnv

env = CivEnv(num_players=2, width=20, height=14, seed=0,
             opponent="basic",      # scripted AI for players 1..n
             max_turns=300,
             reward_mode="win")     # "win" (+1/-1 terminal) or "score" (shaped)
obs = env.reset()
while not env.done:
    acts = env.legal_actions()          # list of action dicts, always non-empty
    obs, reward, done, info = env.step(acts[0])
```

- The agent is **player 0**; `step` on `end_turn` runs all opponents and
  returns when it is your turn again (or the game ended).
- Illegal actions never crash: state is unchanged and `info["illegal"]`
  explains why (useful for LLM agents).
- Observations are JSON-able dicts with fog of war applied: explored tiles,
  visible enemy units, own cities in full detail, public per-player scores.
  See `env.observe()` for the exact schema.

## Multi-agent / self-play

Drive `Game` directly — one `take_turn`-style controller per player:

```python
from civvis import Game
from civvis.ai import make_ai

g = Game(num_players=4, seed=1, max_turns=250)
ais = {p.id: make_ai("basic", seed=p.id) for p in g.players}
while g.winner is None:
    ais[g.current].take_turn(g, g.current)
    if g.winner is None:
        g.apply(g.current, {"type": "end_turn"})
```

A custom AI is any object with `take_turn(game, pid)` that ends with
`game.apply(pid, {"type": "end_turn"})`. The scripted `BasicAI` reads full
state (it cheats on fog) — fair-play agents should restrict themselves to
`env.observe(pid)`.

## Determinism, speed, evaluation

- Same seed + same actions = identical game (RNG serialized in saves), so
  experiments reproduce exactly.
- `civvis benchmark` reports turns/sec; pure-Python engine, no deps, so it
  parallelizes trivially across processes for self-play data generation.
- Suggested evaluation: fixed seed set, win-rate vs `basic` + mean score at
  turn N; keep `random` as a sanity baseline.

## Ideas that fit this engine

- LLM agents: feed `observe()` + `legal_actions()` straight into tool calls.
- RL: `reward_mode="score"` gives dense shaping; mask actions by type first.
- MCTS: `Game.to_dict()`/`from_dict()` is a fast copy mechanism for rollouts.
- Curriculum: shrink `width/height/max_turns` for early training.
