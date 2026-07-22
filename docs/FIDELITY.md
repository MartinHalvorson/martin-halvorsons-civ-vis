# Fidelity: making CIVVIS an exact match for Civilization VI

CIVVIS started as a Civ-6-*like* engine: rules transcribed by hand from the
Civilopedia and the wiki, tuned until games felt right. That is enough to train
an agent that is superhuman *at CIVVIS*. It is not enough for the goal this
project is actually aimed at — an agent whose policy transfers back to the real
game. A policy trained against wrong unit costs learns wrong build orders.

This document defines what "exact" means here, how it is measured, and what is
built next.

## What exactness can mean

Civilization VI's rules live in a closed DLL. Bit-identical random streams are
unattainable and unnecessary. The achievable contract has three clauses:

1. **Identical legal-action sets.** In any state both engines admit the same
   moves. This is the clause a trained policy is most sensitive to: an action
   CIVVIS allows and the real game forbids is a habit that transfers as a
   blunder.
2. **Bit-identical deterministic transitions.** Yields, costs, adjacency,
   movement, growth, research — every transition with no randomness in it
   agrees exactly, over a canonical state schema.
3. **Distribution-identical stochastic transitions**, plus forced-outcome
   replay: given a logged real game's random outcomes, CIVVIS reproduces it
   step by step.

Everything below is machinery for enforcing those three clauses. The order
matters — clause 2 is cheap to test statically and catches the largest bug
class, so it comes first.

## Phase 1 (shipped): rules data measured against the game database

The game ships nearly every rules constant as readable XML under
`Base/Assets/Gameplay/Data`, with expansion overlays in `DLC/Expansion1` and
`DLC/Expansion2`. Those files are the authority — the wiki is a secondary
source that lags balance patches, and CIVVIS' data was demonstrably carrying
pre-Gathering-Storm numbers because of it.

`tools/civ6_fidelity.py` loads that database in the game's own load order,
projects it onto CIVVIS' schema, and reports every divergence:

```sh
python tools/civ6_fidelity.py                      # markdown report
python tools/civ6_fidelity.py --max-divergences 0  # CI ratchet
```

It needs a local installation (auto-detected, or `--civ6` / `$CIV6_DIR`). It
reads the game files and never copies them into the repository; only the
divergence report is an artifact, which keeps the audit reproducible without
redistributing Firaxis data.

Two loader details are worth knowing, because getting either wrong silently
produces a clean-looking but false report:

- The XML uses attributes (`<Row Cost="80"/>`) and child elements
  (`<Set><Cost>730</Cost></Set>`) interchangeably. Handling only the first
  spelling drops every expansion rebalance and makes the base game look like
  the current ruleset.
- Each expansion ships a plain overlay and a `_Major` overlay per table; the
  `_Major` pass applies afterwards.

**Result of the first run:** 55 real divergences across units, technologies,
civics, buildings and districts. All are now resolved — 31 by correcting
CIVVIS' data, the rest by recording them as deliberate:

| Fixed | Examples |
|---|---|
| Gathering Storm rebalances CIVVIS had at vanilla values | Knight 180→220 production and 3→4 maintenance, Pikeman 200→180 and 3→2, Military Engineer maintenance 4→2, Shipyard maintenance 2→1 |
| Modern-era building costs, uniformly overstated | Factory/Stock Exchange/Military Academy 390→330, Research Lab/Seaport/Broadcast Center/Film Studio/Shopping Mall 580→440, Stadium 660→480, Airport 600→480, Zoo/Aquarium 445→360, Hangar/Food Market 465→380 |
| Sight radii left at the default | Spy, Naturalist, Helicopter, Rocket Artillery, Giant Death Robot all see 3 tiles, not 2 |
| Missing prerequisites | Cartography and Mass Production both require Shipbuilding |

`tools/fidelity_waivers.json` holds the accepted divergences, each with a
reason — Future-era techs and civics draw randomized prerequisites in
Gathering Storm, purchase-only units store a Faith price where the database
stores an unpayable production cost, the City Center is placed rather than
produced. **That file is the fidelity roadmap: shrinking it is the work.**
Anything not listed there counts against the ratchet, which now stands at
zero for these five tables.

### Next inside phase 1

The audit currently covers costs, maintenance, movement, sight, combat
strengths, ranges, charges and tree prerequisites. The tables it does not yet
project are where the remaining divergence lives: `Building_YieldChanges`,
`District_Adjacencies`, `Improvement_YieldChanges`, `Terrains`, `Features`,
`Resources`, `UnitPromotions`, `Governments`, `Policies`, `Beliefs`. Each is a
mechanical extension of the same projection pattern.

The "Only in Civ VI" column measures scope rather than error — 51 units and 62
buildings CIVVIS does not model, almost all of them civilization uniques from
DLC packs. That column is the content backlog.

## Phase 2 (next): the modifier engine

Nearly all Civ 6 *content* — leader and civilization abilities, wonders,
beliefs, policies, governors — is not code. It is rows in the `Modifiers`
table: an `EffectType`, arguments, and a `RequirementSet`, attached to a
collection of game objects. CIVVIS hardcodes these effects one at a time in
Rust, which is why every new civilization is an engineering task.

Implementing the interpreter instead of the content inverts that:

- Content correctness collapses into engine correctness. A few hundred effect
  and requirement types cover thousands of rows.
- Remaining scope becomes measurable. Log every unimplemented `EffectType` with
  the number of rows that reference it, and implement in frequency × impact
  order instead of guessing.
- Balance mods become a database swap. The competitive-multiplayer ruleset
  (BBG) is mostly SQL edits, so a modifier-driven CIVVIS gets it nearly free.

## Phase 3: the ground-truth bridge

The real game cannot run headless or fast, so it can never be a training
environment — but it is an excellent *oracle*, and the same bridge doubles as
the evaluation and exhibition path that makes a CIVVIS-trained policy a Civ 6
policy.

Three components, all reusable:

1. **Logger mod** (Lua, gameplay context): serialize full state each turn plus
   the ordered `GameEvents` stream into the canonical schema. Two modes —
   omniscient for golden tests, `PlayerVisibility`-filtered for fair play.
2. **Action injector**: FireTuner speaks a local TCP console protocol, so Lua
   can be driven remotely and unit/city/player operations issued through the
   same request path a human client uses.
3. **Turn-0 import**: fix the map and game seeds, export via WorldBuilder
   (`.Civ6Map` is SQLite) and load the same start directly in CIVVIS.

The property that makes this work: with fixed seeds and an identical action
stream, a real game is reproducible. Every combat roll and goody hut replays,
so real games become perfect oracles without touching the DLL. The event stream
is also the empirical specification of the between-turns phase machine —
healing versus growth versus border expansion, barbarian and city-state
ordering — and ordering bugs are where near-clones die.

## Phase 4: the differential test stack

Escalating, each stage cheaper per bug than the next:

1. **Derived-value differ (static).** From any dumped real state, recompute
   every derived quantity — yields, adjacencies, combat previews, movement
   costs, housing, amenities, loyalty — and diff. Requires no dynamics.
2. **Action-replay differ (dynamic).** Replay logged action streams from turn
   0 with stochastic outcomes forced from the log; require per-phase state-hash
   equality; report first divergence by subsystem.
3. **Distribution tests.** KS-test each stochastic node — combat rolls,
   barbarian spawns, goody huts, climate events — against thousands of real
   samples.
4. **Fuzzing.** Random-but-legal action sequences injected into both engines,
   with delta-debugging minimization down to a small repro.
5. **CI freeze.** The golden corpus reruns on every rules change, so fidelity
   only ratchets up.

Oracle throughput is the bottleneck: real games run near real time, roughly one
game per hour per instance. Plan for a small fleet or weeks of soak, and lean
on distribution tests and fuzzing where oracle samples are scarce.

## Determinism rules for engine code

These already hold in CIVVIS and must keep holding, because clause 2 depends on
them: integer/fixed-point arithmetic in rules code (the game's own values are
integers, often ×100); no unordered iteration in game logic; per-subsystem RNG
streams with seeded, forced and recording modes; serialization to the same
canonical schema the logger emits, so diffing is trivial; a state hash per
phase; cheap snapshots so search stays affordable.

## Baseline configuration

Gathering Storm ruleset, standard DLC civilizations, NFP game modes off,
sequential turns. Simultaneous-turn multiplayer changes action interleaving
rather than transition rules, so the competitive ruleset layers on after the
sequential clone is exact.
