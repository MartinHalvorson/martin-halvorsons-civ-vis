# Roadmap

## v0.1 (shipped)

Headless engine: hex map + mapgen, cities/growth/borders, districts with
adjacency, buildings/improvements, tech + civic trees, melee/ranged/city
combat, war/peace, three victory types, fog of war, JSON saves, gym-style
env, scripted AIs, CLI, tests.

## v0.2 (shipped)

City-states (pre-founded defensive minors, conquerable, excluded from
victory); `soak` command playing many full AI games across seeds with anomaly
flags — end-to-end games verified at 2-8 players, 100-200 turns.

## v0.3 (shipped) — Rust performance core

`rust/` crate ports the full engine (map/cities/districts/tech/combat/
city-states/AI/CLI) with the same embedded ruleset JSONs and action protocol.
~16x single-core over Python (36k vs 2.3k turns/sec), parallel across cores
with no GIL. Python engine remains the reference spec.

Next for the Rust core:
- PyO3 bindings (maturin) so Python agents/env drive the Rust engine
- Ruleset ID interning + yield caching (est. several-fold further speedup)
- Observation builder + fog in Rust for RL feature extraction

## v0.2 — rules depth

- Housing & amenities (growth caps), rivers + fresh water
- Eureka/Inspiration boosts
- Unit promotions, XP, zone of control, fortify
- Embarkation; ocean crossing gated by tech
- City ranged strikes; wall HP as separate pool
- Policy cards + governments (civics currently only unlock content)
- Barbarian camps

## v0.3 — systems breadth

- Religion (pantheons, beliefs, religious combat)
- Great people; trade routes; city-states + envoys
- Deeper diplomacy (deals, denouncements, negotiated peace)
- Per-civ unique abilities/units (data-driven, like everything else)
- Era score / golden ages

## v0.4 — clients

- Web client (canvas hex renderer) speaking the JSON action protocol to a
  local server wrapper around `Game`
- Terminal TUI client
- Multiplayer via the same protocol (engine is already lockstep-friendly)

## v0.5 — mod ecosystem

- Ruleset validation + mod loader (multiple data dirs, overrides)
- Full Civ 6 base-game content pass in `data/`

## AI track (parallel)

- PettingZoo-style multi-agent wrapper
- Action-masked observation tensors for RL
- MCTS baseline using dict-state cloning
- Seeded tournament harness + Elo for agent evaluation
