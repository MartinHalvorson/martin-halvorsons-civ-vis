# Architecture

## Layers

```
data (JSON rulesets)  ->  engine (Game)  ->  interfaces (CivEnv / CLI / AIs)
```

- **Ruleset** (`rules.py` + `data/*.json`): all content — terrains, features,
  resources, improvements, units, districts, buildings, projects, techs, civics — is
  data, not code. Pass a custom `data_dir` to `Ruleset` to mod the game
  (Unciv-style).
- **Game** (`game.py`): the authoritative state machine. Holds the map,
  players, units, cities, war state, RNG. All mutation goes through
  `Game.apply(pid, action)`; anything invalid raises `IllegalAction` and
  leaves state untouched.
- **Interfaces**: `CivEnv` (gym-style single-agent), scripted AIs
  (`ai/basic_ai.py`, `ai/random_ai.py`), and the CLI. All speak the same
  action-dict protocol, so a GUI or network client later needs no engine
  changes.

## Action protocol

`Game.legal_actions(pid)` enumerates every valid action as a JSON-able dict:

| type | fields | effect |
|---|---|---|
| `move` | `unit`, `to: [q,r]` | move one tile (cost from terrain) |
| `move_to` | `unit`, `to: [q,r]` | multi-step move along best path within remaining MP |
| `attack` | `unit`, `target` | melee attack unit/city (auto-declares war) |
| `ranged` | `unit`, `target` | ranged attack, no counterattack |
| `found_city` | `unit` | settler founds a city (min distance 4) |
| `improve` | `unit`, `improvement` | builder spends a charge |
| `produce` | `city`, `item` | set production: `{"unit": n}` / `{"building": n}` / `{"district": n, "pos": [q,r]}` |
| `buy` | `city`, `unit`, `currency` | instant purchase with gold (4x cost) or faith (2x) |
| `research` / `civic` | `tech` / `civic` | pick current research |
| `declare_war` / `make_peace` | `player` | diplomacy |
| `end_turn` | — | pass control to next player |

Positions are axial hex coordinates `[q, r]`; maps are generated from odd-r
offset rectangles (`hexgrid.py`).

## Turn lifecycle

Players act sequentially. On becoming current (`_begin_turn`): unit moves
reset + healing; each city collects yields, grows/starves (Civ 6 food curve),
advances production, expands borders; empire science/culture advance the
current tech/civic (overflow banked when none selected); gold/faith accrue
(unit maintenance beyond 3 free units). Victory checks follow the six Civ VI
paths: science requires a Spaceport, the ordered Earth Satellite, Moon Landing,
Mars Colony, and Exoplanet Expedition projects, then travel to 50 light-years;
domination requires every foreign original capital; religious victory requires
a strict city majority in every living major; culture compares visiting tourists
against the largest rival domestic-tourist total; diplomacy requires 20 victory
points; score is used only after `max_turns`.

## Combat

Civ 6 math: effective strength drops 1 per 10 HP lost; damage =
`30·e^(diff/25)·U(0.8,1.2)` clamped to [1,100]. Defenders get +3 on
hills/forest/jungle. Melee draws a counterattack; ranged does not. City strength
uses the strongest unit built (or a stronger garrison), walls, districts,
terrain, and capital/policy modifiers. Cities have 200 HP and can only be
captured by melee. Ordinary ranged attacks floor city HP at 1; Bombard attacks
may deplete it to 0 but cannot capture it. Melee capture converts the city (pop
-1, walls razed), destroys the garrison, and captures eligible civilians.

## Determinism & serialization

One serialized `Rng` drives map generation and combat; scripted AIs use their
own seeded RNGs. Same seed + same action sequence = the same game, and JSON
saves round-trip the complete state including RNG.

## Fidelity notes

The combat curve, ZOC, embarkation, XP/levels, fortification, and early-game
siege support follow Civ VI rules. Class-specific promotion choices/effects,
linked formations/corps/armies, and independent Encampment combat remain
unmodeled. See ROADMAP.md.
