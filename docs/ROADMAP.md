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

## Rust core port (next major milestone)

Once rules stabilize, port the engine core to Rust for AI-training throughput
(target 50-100x), keeping the identical JSON action/observation protocol and
PyO3 bindings so this Python engine remains the executable spec and all
agents/tests/saves transfer.

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
