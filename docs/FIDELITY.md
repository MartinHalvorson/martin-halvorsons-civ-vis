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

**Sixth wave (bands, maps, routes, spawns):** `Happinesses`, `Maps`, `WMDs`
and more of `GlobalParameters` join the audit — 25 tables at zero unwaived
divergences. The finds: the amenity bands were wrong twice over (thresholds
from an invented ≥5 tier, values from the base game the expansions
rebalance — the shipped ladder is +20/+10/0/−15/−30/−100 growth and
+20/+10/0/−10/−20/−30/−40 non-food); roads now follow the shipped route
ladder (1 MP until Industrial-era routes at 0.75, Modern at 0.5) and bridge
rivers with Classical-era (medieval) routes instead of a technology;
resource map placement follows the union rule with hills-only (Sheep,
Copper, Iron, Coal, Diamonds) and flat-only (grains) forms honored — a
regression where feature-listed resources became feature-*only* is fixed;
barbarian camps keep the shipped placement floors (4 from cities, 7 from
camps); nuclear device stats live in `data/wmds.json` with maintenance
charged from data; the Cultural Heritage Inspiration now fires via a
full-museum theming proxy; and Gilgamesh aside, the six map-size profiles
and the alliance-leveling timeline verified exact.

**Seventh wave (routes per tile):** `Routes` joins the audit — 27 tables
at zero unwaived divergences. Roads are leveled per tile on the shipped
PlacementValue ladder: traders lay the best route their civilization's
era allows (Medieval from Classical, Industrial and Modern from their
namesake eras), each step is priced by the destination tile's route
(1 / 0.75 / 0.5 / 0.25 MP), river bridging is a route property (Medieval
and later, `SupportsBridges`) instead of a world-era check, and Military
Engineers lay Railroads for 1 Iron and 1 Coal once Steam Power is in —
no build charge, exactly as `Routes_XP2` and `Route_ResourceCosts`
price them.

### Data the engine never reads

`tools/civvis_inert.py` joins the other direction: every effect key in
`data/*.json` against the engine that should consume it. Nothing enforces that
join, so a key can sit in the data doing nothing -- mistyped, refactored away,
or dropped by a rebase in a shared checkout. The last is not hypothetical: the
Sphinx's Floodplains Culture and Wonder-adjacency Faith survived in data for
fourteen iterations after their engine arm was lost, and no test noticed
because none covered them.

It reports zero unwaived keys across 629, with five waived in
`tools/inert_waivers.json` for consumption the string join cannot see. Run it
after any refactor that moves yield code.

**Eighth wave (the meteor):** the Apocalypse pack's meteor shower lands in
the engine — about six strikes per game (the shipped Moderate frequency)
on the shipped strike terrains outside anyone's territory, each leaving
the `METEOR_GOODIES` site whose one-entry table grants the most advanced
Heavy Cavalry the finder can field, in their nearest city, exempt from
resource upkeep (the shipped refund modifier). The tribal-village roll
keeps its seven categories — the meteor's table is its own goody type,
popped only by its own site.

**Ninth wave (the weather):** Gathering Storm's random disasters now
happen rather than merely being resolvable. Every class — volcanic
eruptions, river floods, droughts, and the four terrain-bound storm
systems — rolls each turn against a per-game budget, scaled by the lobby's
disaster-intensity setting and again by the warming already banked, and
resolves through the protection rules that were already modelled (Dams,
the Great Bath, Egypt's Iteru, Aqueducts, Flood Barriers, and Governors'
Reinforced Materials). Volcanoes now have the shipped active/dormant
split, eruptions bury the ring they reach and leave Volcanic Soil, storms
drift for three turns and pay for their damage with fertility, and
droughts hold their tiles for a severity-scaled span before lifting.

Two honest boundaries around it:

- **The rates are calibrated, not shipped.** The tuning that lives in
  `Expansion2_RandomEvents.xml` — occurrences per game, severity weights,
  per-severity damage — is not published outside an installation, and the
  only figures that are public are the band a volcano's activity sits in
  (45%–95% across the five intensities) and the fact that intensities 3
  and 4 widen an eruption to two rings. Both of those are exact here. The
  rest lives in `data/disasters.json` precisely so it is visible, tunable
  and moddable rather than buried in Rust: the per-class `per_game`
  budgets, the intensity ladder, and the climate scaling are CIVVIS
  numbers chosen to land in the documented range, and a pinned tournament
  ruleset can replace the file wholesale. `validate` checks the file's
  shape, and a test asserts a full game lands near the rates it asks for.
- **Flood fertilisation stays off.** Gathering Storm gives a flooded
  Floodplains tile a chance at permanent extra Food and Production, and
  that probability is one of the numbers only an installation carries.
  The mechanism is implemented — `disaster_food`/`disaster_production` are
  real tile yields, and storms use them — but `river_flood`'s
  `fertility_chance` is zero until the shipped table can be read, because
  a guessed fertility rate changes what Floodplains are worth for the
  whole game.

**Both halves of every Dedication.** A Dedication in Civ VI is two rules,
not one: the Normal-Age half that turns the behaviour it names into Era
Score, and the Golden-Age half that only a Golden or Heroic Age switches
on. CIVVIS had the Golden halves and none of the Normal ones, which meant
choosing a Dedication in a Normal or Dark Age did nothing at all — and
since that Era Score is exactly what a Dark Age civilization climbs out on,
the ages below Golden had no engine behind them. All twelve Dedications
now carry both halves in `data/dedications.json`, including the two that
were missing entirely (Wish You Were Here, Bodyguard of Lies), with their
triggers wired to the seventeen moments that pay them. The per-Dedication
era spans are exact where the Civilopedia states them (Exodus of the
Evangelists through the Renaissance, Automaton Warfare and Wish You Were
Here in the last two eras) and era-appropriate where it does not.

**Dark Age policy cards.** A Dark Age also opens a Wildcard slot to cards
no civic unlocks: strong effects bought with a real drawback. The seven
with published effects are implemented and execute both halves — Twilight
Valor (+5 melee attack Strength, no healing outside your own territory),
Isolationism (+1 Trade Route capacity and +2 Food/+2 Production at home,
but no Settlers trained, bought or settled), Monasticism (Science doubled
in Holy Site cities, -25% Culture), Inquisition (+15 Religious Combat
Strength at home, -25% Science), Letters of Marque (+100% naval-raider
Production, +2 Movement, doubled plunder, -2 Trade Route capacity), Elite
Forces (+100% unit experience, +2 Gold per military unit) and Robber
Barons (+50% Gold with a Stock Exchange, +25% Production with a Factory,
-2 Amenities everywhere). They are offered only while the civilization is
actually in a Dark Age and inside the card's own eras, and an age change
takes them back out of their slot.

The ten cards Gathering Storm added (Collectivism, Cyber Warfare,
Decentralization, Despotic Paternalism, Disinformation Campaign, Flower
Power, Rogue State, Samoderzhaviye, Soft Targets, Automated Workforce) are
not modelled: their effects are not published in a form worth copying, and
guessing at a card's numbers is worse than not shipping it.

### Next inside phase 1

Known simplifications not yet expressed in data: civic-gated valid terrain
(farms on Hills at Civil Engineering, already era-exact through
`tree_effects`' hill_farms), wonders' widening `Building_ValidFeatures`
rows (CIVVIS is the permissive side there), theming (complete: works are
era-and-creator pieces, museums theme on the shipped rules — three
artists for art, three origin civilizations for artifacts — with the
+100% bonus), barbarian camp spawn cadence (the odds/boldness
model is DLL-side; placement floors and distances are exact), WMD delivery detail
(the `WmdStrike` action launches on the shipped ranges, radii and fallout;
and the per-ring unit damage is the one number the database does not carry).

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
sequential turns. Free-for-all and official pre-game team relations and victory
rules are both supported. Simultaneous-turn multiplayer changes action
interleaving rather than transition rules, so the rest of the competitive
ruleset layers on after the sequential clone is exact. See
[COMPETITIVE.md](COMPETITIVE.md) for the tournament-specific boundary.
