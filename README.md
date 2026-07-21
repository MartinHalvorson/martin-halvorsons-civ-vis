# Martin Halvorson's Civilization VIS

An open-source, **headless-first** 4X strategy engine inspired by the mechanics
of Civilization VI — aiming to be to Civ 6 what [Unciv](https://github.com/yairm210/Unciv)
is to Civ 5, with one twist: the engine is designed **AI-first**. Every game can
run without any UI, at thousands of turns per minute, behind a gym-style API,
so advanced AI strategies (RL, MCTS, LLM agents) can be developed against it.

Not affiliated with Firaxis or 2K. No assets, art, text, or code from
Civilization VI are used — this is an original implementation of similar
mechanics.

## What's implemented (v0.1)

- Hex map (axial coords), random continents, climate bands, features, resources
- Cities: population growth (Civ 6 food curve), border expansion, production
- **Districts with adjacency bonuses** (campus, holy site, commercial hub,
  harbor, encampment, theater square) — the signature Civ 6 mechanic
- Buildings, tile improvements, builders with charges
- Tech tree **and civics tree** (separate science/culture progress, overflow)
- Units, melee + ranged combat with Civ 6 damage math (`30·e^(Δ/25)`),
  city sieges, capture, civilian capture
- **City-states**: pre-founded minor civs that defend themselves, never expand
  or start wars, and can be conquered (excluded from victory conditions)
- Diplomacy: war/peace; victory by **domination, science, or score**
- Fog of war (per-player explored + visible sets in observations)
- Full JSON serialization (save/load), deterministic given a seed
- Moddable ruleset: all content lives in `civvis/data/*.json` (Unciv-style)
- Scripted AIs (`basic`, `random`) and a gym-style `CivEnv` for agents
- Zero runtime dependencies; pure Python

## Two engines, one game

- **`civvis/` (Python)** — the reference implementation and executable spec.
  Zero deps, easiest to iterate on rules, powers the gym-style env today.
- **`rust/` (Rust)** — the performance core for AI training. Same ruleset
  JSONs (embedded at compile time), same JSON action protocol, same
  mechanics; **~16x faster single-core (36k turns/sec) and parallelizes
  across cores with no GIL** (~100k+ games/hour on a workstation). PyO3
  bindings are the next step so Python agents drive the Rust core directly.

```bash
cd rust && cargo build --release
./target/release/civvisr simulate --players 4 --seed 17
./target/release/civvisr soak --games 10 --players 4 --turns 120
./target/release/civvisr benchmark --games 100
```

Each engine is deterministic per seed (RNG formats differ between the two).

## Quickstart

```bash
pip install -e .
civvis simulate --players 4 --seed 42          # AI self-play with ascii map
civvis soak --games 10 --players 4 --turns 120  # many full games, flag anomalies
civvis benchmark                                # engine speed
```

## Headless AI development

```python
from civvis import CivEnv

env = CivEnv(num_players=2, seed=0, opponent="basic", reward_mode="score")
obs = env.reset()
while not env.done:
    action = my_agent.choose(obs, env.legal_actions())   # plain dicts
    obs, reward, done, info = env.step(action)
```

Everything — observations, actions, saves — is a plain JSON-able dict, so the
engine drops straight into RL loops, LLM tool-calling, or a future network
protocol. See [docs/AI_GUIDE.md](docs/AI_GUIDE.md).

## Programmatic engine use

```python
from civvis import Game

g = Game(num_players=2, width=24, height=16, seed=7)
g.apply(0, {"type": "found_city", "unit": 1})
print(g.legal_actions(0))
g.save("save.json")
```

## Layout

```
civvis/           engine package (zero deps)
  data/          moddable ruleset JSONs (terrain, units, districts, techs...)
  game.py        core turn engine + action protocol
  env.py         gym-style headless environment
  ai/            scripted baseline AIs
  cli.py         simulate / benchmark / render
docs/            architecture, AI guide, roadmap
tests/           pytest suite
```

## Docs

- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — design, action protocol, turn lifecycle
- [AI_GUIDE.md](docs/AI_GUIDE.md) — building agents against the engine
- [ROADMAP.md](docs/ROADMAP.md) — path to full Civ 6 parity + GUI client

## License

MIT
