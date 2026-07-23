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

**Second wave (terrain layer):** the audit now also projects `Terrains`,
`Features`, `Resources` and `Improvements` — yields, movement, defense
modifiers, passability, housing, valid terrain/feature/resource placement,
reveal prerequisites, and the tech/civic-gated improvement upgrades of
`Improvement_BonusYieldChanges` against `tree_effects.json` — and loads the content packs a standard all-content
game enables (civilization, leader and landmark packs; scenario and
optional-mode data stays out). Where the game enumerates variant rows the
projection folds them onto CIVVIS' spelling: hills rows become the engine's
single hills rule, `*_MOUNTAIN` rows the one impassable mountain terrain,
lake plots the coast rows they really are, and an all-land enumeration means
"no terrain restriction". The wave surfaced 88 more real divergences, all
resolved. The largest:

| Fixed | Detail |
|---|---|
| Movement was max-based, the game's is additive | `move_cost = terrain + hills + feature`: Woods on Hills is 3 MP, not 2. Feature data now stores the database's `MovementChange` |
| Floodplains carried vanilla values | Desert floodplains 3→2 Food; grassland/plains floodplains add no yields at all; all three impose −2 defense like Marsh |
| Reef defense was on the wrong feature | The bonus Reef grants +3, Great Barrier Reef grants nothing |
| Pamukkale modeled as tile culture | Its real effect: +1 Amenity to the owning city, +1 more while its plot is adjacent to an Entertainment Complex |
| 32 resources missing | Every Gathering Storm luxury and bonus resource now exists with exact class, yields, placement and improvement — including the manufactured four (Toys, Jeans, Perfume, Cosmetics) no tile improvement works |
| Wrong valid-placement lists | Wheat is plains-only (plus floodplains), Stone grassland-only, Sheep spawn on grassland too, Uranium anywhere including snow; camps/mines/quarries/plantations/fishing boats accept their full resource sets |
| Oil Wells unlocked two eras early | Steel → Refining, as shipped |

The "Only in Civ VI" column measures scope rather than error — the units and
buildings CIVVIS does not model are almost all civilization uniques from DLC
packs, and the missing features are natural wonders plus the volcano system.
That column is the content backlog.

**District adjacency (parallel session):** `District_Adjacencies` joined to
`Adjacency_YieldChanges` against each district's per-source `adjacency` map,
dividing every rule's yield by its `TilesRequired`. It surfaced one wrong
Industrial Zone Mine rule, now fixed. This projection is the case that
justifies the whole approach: reading the XML by hand said Wonder adjacency
was +1 Culture, because the base row says `YieldChange="1"` and Rise and
Fall raises it to 2 through a separate `<Update>` element. The loader
applies overlays; eyes skimming a dump do not.

**Third wave (content layer):** `Wonders` (cost, prereqs, yields, housing,
amenities, regional ranges, great-work slots, great-person points, and the
whole placement predicate — terrain, required features, coast/lake/river,
adjacency), richer `Buildings` (housing, amenities, citizen slots, yields,
districts, great-work slots), `Policies` (slot + civic), `Governments`
(slots, influence, prereqs), `Beliefs` (classes), `UnitPromotions`
(class/tier/prerequisite trees), and `Projects` (costs, districts, GPP,
yield conversions, cost progressions). Policies, governments and beliefs
were already exact; the rest surfaced 62 divergences, all resolved:

| Fixed | Detail |
|---|---|
| Naval Raider and Carrier promotion trees rearranged | Loot is tier 1 with no prerequisite, Homing Torpedoes tier 2, Silent Running tier 3, Wolfpack tier 4 — plus five wrong prerequisite lists (Armor Piercing, Hangar Deck, Folding Wings, Observation, Swift Keel) |
| Reactor-era project costs | Coal/Oil/Uranium conversions 300/360/480 → 200/300/400, Recommission Reactor 200 → 400, Operation Ivy 1200 → 1000 |
| Gathering Storm building buffs missed | Palace grants 2 Amenities (not 1), Biosphère +8 Science, Prasat 2 Relic slots and 4 Faith, Sukiennice 3 Gold, Tlachtli 1 Culture |
| Jebel Barkal double-counted | Its +4 Faith reaches every city within 6 tiles including its host; CIVVIS carried a local copy on top of the regional effect |
| Estádio do Maracanã was local | The game gives its 6 Culture and 2 Amenities to every city in the empire (regional range 100000) |
| Improvement siting was intersection-based | Civ 6 sites improvements through any of three routes — valid terrain OR valid feature OR valid resource. Farms on desert Floodplains and flat resource mines now place exactly as shipped |

**Fourth wave (triggers, villages, eras, constants):** `Boosts`, `GoodyHuts`,
`Eras`, `GreatPersonIndividuals` and a curated `GlobalParameters` mirror are
audited — 22 tables at zero unwaived divergences. The finds this wave:

| Fixed | Detail |
|---|---|
| Nearly every Eureka/Inspiration was wrong | 109 of ~110 boost entries corrected to the shipped trigger, count and target; the engine trigger vocabulary grew from 16 to 45 forms (meet a civilization, improve a specific resource, government slot counts, continents, alliances, promotions-class kills…), several data triggers that silently never fired now do, and the boost grant is per-row data (40%, Near Future Governance 90%, +10 points for China) |
| Tribal villages used a lean 4-outcome table | The shipped seven-category, 22-reward table with exact weights, turn gates, city gates and amounts now drives rewards (`data/goody_huts.json`); Gilgamesh's Epic Quest rolls the same table |
| Embarked strength was flat 10 | It climbs the shipped era ladder (10/15/15/30/35/50/55/55) via `data/eras.json` |
| Border growth used a homebrew curve | Borders now grow on the city's Culture against the shipped cost curve, 10 + 6 × plots^1.3 (was 15 + 8 × plots fed by 1 + Culture/2) |
| Great Person costs | Recruit costs follow the shipped per-era ladder (30…1320); two prophets carried invented prices |
| Constants verified | Growth curve/thresholds, housing bands, fresh-water housing, amenity demand (GS zeroes the free Amenity), city spacing, corps/army bonuses, amphibious/river combat modifiers, barbarian XP caps — and the damage roll: CIVVIS' 30·e^(Δ/25)·U(0.8, 1.2) is the same distribution as the shipped 24 base with its 1.0–1.5 spread |

**Fifth wave (the unit lifecycle):** `UnitUpgrades` and the `Units` column
`MandatoryObsoleteTech` are now projected alongside every other unit field,
which closed the largest remaining behavioural hole in the audit: CIVVIS
carried neither, so no unit ever retired and no unit ever upgraded. An
Information-era empire still fielded — and still trained — Slingers.

| Fixed | Detail |
|---|---|
| No unit ever became obsolete | 33 units carry the shipped `MandatoryObsoleteTech`; researching it removes the unit from every production and purchase menu and from every queue |
| No unit could ever be upgraded | 52 units carry their shipped `UnitUpgrades` successor, reachable through the new `upgrade_unit` action |

The Gold price is the one number this wave could not read from the database:
`UPGRADE_BASE_COST` (10) and `UPGRADE_MINIMUM_COST` (15) are shipped
GlobalParameters, but the per-Production factor lives in the executable. The
engine charges the community-documented `10 + 2 × Production difference`,
which reproduces the in-game prices those two parameters bracket.

### Next inside phase 1

Known simplifications not yet expressed in data: hills-only resource spawns
(Sheep), civic-gated valid terrain (farms on Hills at Civil Engineering),
feature-based map placement of resources (Fish on Reef rows), wonders'
widening `Building_ValidFeatures` rows (Petra on Floodplains), theming (the
Cultural Heritage Inspiration trigger is data-correct but inert), barbarian
camp spawn cadence (odds/boldness model), and the meteor-strike goody site.

## Phase 2 (measured): the modifier engine

The size of this phase is no longer a guess. `tools/civ6_modifiers.py`
censuses the shipped `Modifiers` tables and reports 3,405 rows across 698
distinct effects, of which CIVVIS covers 825 rows. Crucially the tail is long:
32 effects reach half the rows, and the other half needs 666 more. See
[MODIFIERS.md](MODIFIERS.md) for the ranked backlog and the order of work.

### Why an interpreter

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
