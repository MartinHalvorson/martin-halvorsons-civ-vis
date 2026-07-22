# Civ 6 mechanics coverage

Tracked against the [Civilization Wiki](https://civilization.fandom.com/wiki/Civilization_VI).
Status: ✅ implemented · 🟡 simplified · ❌ not yet.

> **In progress** (claimed by parallel sessions — check before starting a batch):
> - AI barbarian pursuit/attack fix (src/ai.rs exchange/nearest_enemy/military_step, src/ai/advanced.rs military step) — session F

| System | Status | Notes |
|---|---|---|
| Hex map, fog of war, terrain/features/resources | ✅ | all six stock Civ VI map-size profiles; tile-based rivers (🟡 vs edge rivers) |
| Leaders & civ uniques | ✅ | all 8 civs: Trajan, Cleopatra, Pericles, Qin Shi Huang, Gilgamesh, Montezuma, Amanitore, Tomyris — signature ability each + 8 unique units (legion, hoplite, eagle warrior, war cart, pítati archer, maryannu chariot archer, saka horse archer, crouching tiger) replacing/blocking their base units |
| Rivers & fresh water housing (5/3/2) | ✅ | water values plus Palace, Aqueduct normalization, coastal Lighthouse bonus, and +0.5 from owned Farms, Pastures, Plantations, Camps, and Fishing Boats within 3 tiles |
| City growth curve, border expansion | ✅ | exact Housing headroom bands: 100% at +2, 50% at +1, 25% through -4, and no growth at -5 |
| Housing & amenities | ✅ | final Gathering Storm demand, satisfaction thresholds, yield/growth modifiers, Palace Amenity, connected unique luxuries allocated to the four neediest cities (six for Aztec), and local district/building/policy sources |
| Districts + adjacency (incl. river) | ✅ | 8 district types; specialty-district capacity unlocks at population 1/4/7/... while Spaceports are exempt |
| Wonders | 🟡 | 9 wonders, world-unique, effect engine (growth %, builder charges, unit levels); no tile placement |
| Tech + civics trees, Eureka/Inspiration | ✅ | 28 early-game Civ VI techs + 5 future-era space-race techs / 14 civics; Ancient starts have no phantom Agriculture tech |
| Governments + policy cards | ✅ | wiki slot configs (chiefdom M1E1 … merchant republic M1E2D2W1); 20 cards thru guilds; typed slots + wildcard overflow; obsoletion (Agoge→Feudal Contract); slot/unslot actions; effects: yields, prod-toward-item %, housing, amenities, maintenance, builder charges, city def/ranged, vs-barb CS, recon XP |
| Combat math (Civ 6 formula), XP/promotions | 🟡 | exact damage/wounded-strength curve, terrain, rivers, amphibious attacks, class matchups, ranged/bombard penalties, flanking/support, healing, XP and fortification; promotion choices/effects are not yet modeled |
| Fortify, city ranged strikes, walls | 🟡 | Gathering Storm wall pool (100/level), explicit Repair Outer Defenses project, melee 15% / ranged 50% / siege 100% vs walls, strike = best ranged unit, ordinary ranged floors city HP at 1 while Bombard may deplete but not capture, walls razed on capture; Encampments do not yet have independent HP/strikes |
| Embarkation (Shipbuilding) | ✅ | embarked strength 10, cannot attack |
| Barbarians (camps, raiders, rewards) | ✅ | no scout-alert mechanic |
| City-states + envoys | ✅ | influence by gov tier (100 pts = envoy); type bonuses at 1/3/6 (capital +2, +2 per matching district); suzerain = 6+ & strict lead; war clears envoys |
| Great People | 🟡 | GPP per district/building (+1 each), Classical Republic +15%, Strategos/Inspiration/Revelation wildcards; doubling thresholds (60/120/...); auto-claim generic GPs with instant effects (eurekas, production, gold+envoy, faith, unit levels) — no named individuals/patronage |
| Religion | 🟡 | pantheon at 25 faith (6 beliefs, exclusive); prophet + holy site founds (map-scaled max 2/3/4/5/6/7, classic names); follower/founder beliefs; missionaries (faith-buy, 3 spreads, +200 pressure); passive pressure ±9 tiles; majority at 50; no theological combat/apostles |
| Trade routes & roads | ✅ | wiki capacity (Foreign Trade +1/hub-or-harbor city, +2 merchant republic); vanilla per-district yield table; traders lay roads (cost 1, bridge rivers 🟡); 30-turn duration; war/capture cancels |
| Diplomacy (deals, alliances, grievances) | 🟡 | war/peace only |
| Loyalty + governors (R&F) | 🟡 | population pressure ±9 tiles, capitals immune, defection to strongest neighbor at 0; governor titles from civic milestones (3), +8 loyalty; no promotions/named governors |
| Natural wonders + goody huts | 🟡 | map-scaled 2–7 unique single-tile wonders, feature-based, impassable/crossable, tile yields, discovery era score (+3 first finder); huts ~1/40 land tiles with gold/faith/eureka/inspiration rewards |
| Zone of control | ✅ | innate from turn 1 for the modeled roster; explicit per-unit capability, native domains, river blocking, defensible districts, religious ZOC, cavalry immunity, and class-specific stop behavior |
| Movement: MP paid up front, min-1-tile, river +2 MP | ✅ | river surcharge on entering channel tile (tile-model 🟡) |
| Support units; corps/armies | 🟡 | battering ram (full melee dmg vs ancient walls) + siege tower (melee bypasses walls thru medieval), support stacking class; corps/armies n/a — unlock at Nationalism, beyond current renaissance-era content |
| Victory: domination, science, score, religious, culture, diplomatic | ✅ | science = Spaceport + Satellite → Moon → Mars → Exoplanet, then 50 light-years (repeatable laser projects add speed); domination = every foreign original capital; religious = strict majority in every living major; culture = visiting tourists exceed the best rival domestic total; diplomatic = 20 DVP (congress resolution model 🟡); score = highest score only at the turn limit |
| Eras, era score, golden/dark ages | 🟡 | world era from leader's tech+civic count; era score from wonders/GPs/camps/captures/religion/pantheon; golden +10% / dark -5% yields on transition (R&F-style, simplified thresholds) |

Next batch: policy cards + government slots, trade routes, great people,
city-state envoys, culture victory.

## Combat and zone of control

Zone of control does **not** unlock through a technology or civic; it applies
from turn 1. The Military Tradition civic unlocks the separate +2-per-unit
flanking and support bonuses.

The current units that exert ZOC are Scout, Warrior, Spearman, Swordsman,
Horseman, Pikeman, Knight, Musketman, Galley, Legion, Hoplite, Eagle Warrior,
and War-Cart. Horsemen, Knights, War-Carts, Maryannu Chariot Archers, and Saka
Horse Archers ignore incoming ZOC as cavalry; the latter two do not exert it
because they are ranged units. City Centers and Encampments exert ZOC in every
adjacent domain. Missionaries exert religious ZOC only against units of a
different religion.

Military and religious units that enter ZOC keep unused Movement and may still
attack or perform another legal action, but cannot move to another tile that
turn. Civilian and support units lose all remaining Movement. Unit ZOC is
limited to the unit's native domain and cannot cross a river; defensible
district ZOC is not blocked by rivers.
