# Roadmap

## v0.6 (shipped) — pure Rust

Python reference implementation removed (2026-07-21); the Rust crate is now
the single engine at full v0.5 rules parity, moved to the repo root, with the
GUI server, observation builder, and Elo harness all in Rust (serde-only
deps). External agents use the HTTP JSON protocol; in-process agents use the
`Ai` trait.

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

## v0.4 (shipped) — rules depth + browser GUI

Housing/amenities, eurekas & inspirations, unit XP/levels/fortify, city
ranged strikes, barbarian camps & raiders, governments, medieval/renaissance
content (29 techs, 14 civics), and `civvis play` — a zero-dep local web GUI
for human-vs-AI over the JSON action protocol. Rust core still at v0.3 rules;
batch-port these systems next.

## Later rules depth

- Class-specific promotion trees and promotion effects
- Independent Encampment health, defenses, and ranged strikes
- Corps, armies, and linked unit formations

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
