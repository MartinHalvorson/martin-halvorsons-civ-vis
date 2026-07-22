# Civ 6 mechanics coverage

Tracked against the [Civilization Wiki](https://civilization.fandom.com/wiki/Civilization_VI).
Status: ✅ implemented · 🟡 simplified · ❌ not yet.

> **In progress** (claimed by parallel sessions — check before starting a batch):
> - (none currently claimed by session B)

| System | Status | Notes |
|---|---|---|
| Hex map, fog of war, terrain/features/resources | ✅ | tile-based rivers (🟡 vs edge rivers) |
| Leaders & civ uniques | ✅ | all 8 civs: Trajan, Cleopatra, Pericles, Qin Shi Huang, Gilgamesh, Montezuma, Amanitore, Tomyris — signature ability each + 8 unique units (legion, hoplite, eagle warrior, war cart, pítati archer, maryannu chariot archer, saka horse archer, crouching tiger) replacing/blocking their base units |
| Rivers & fresh water housing (5/3/2) | ✅ | wiki values; oasis counts as fresh |
| City growth curve, border expansion | ✅ | |
| Housing & amenities | ✅ | water/building/policy sources plus +0.5 from owned Farms, Pastures, Plantations, Camps, and Fishing Boats within 3 tiles; luxuries global (🟡 vs 4-city rationing) |
| Districts + adjacency (incl. river) | ✅ | 8 district types |
| Wonders | 🟡 | 9 wonders, world-unique, effect engine (growth %, builder charges, unit levels); no tile placement |
| Tech + civics trees, Eureka/Inspiration | ✅ | 28 real Civ VI techs / 14 civics, through renaissance; Ancient starts have no phantom Agriculture tech |
| Governments + policy cards | ✅ | wiki slot configs (chiefdom M1E1 … merchant republic M1E2D2W1); 20 cards thru guilds; typed slots + wildcard overflow; obsoletion (Agoge→Feudal Contract); slot/unslot actions; effects: yields, prod-toward-item %, housing, amenities, maintenance, builder charges, city def/ranged, vs-barb CS, recon XP |
| Combat math (Civ 6 formula), XP/promotions | 🟡 | flat +5/level vs promotion trees |
| Fortify, city ranged strikes, walls | ✅ | wall HP pool (50/level), melee 15% / ranged 50% / siege 100% vs walls, strike = best ranged unit, walls razed on capture |
| Embarkation (Shipbuilding) | ✅ | embarked strength 10, cannot attack |
| Barbarians (camps, raiders, rewards) | ✅ | no scout-alert mechanic |
| City-states + envoys | ✅ | influence by gov tier (100 pts = envoy); type bonuses at 1/3/6 (capital +2, +2 per matching district); suzerain = 6+ & strict lead; war clears envoys |
| Great People | 🟡 | GPP per district/building (+1 each), Classical Republic +15%, Strategos/Inspiration/Revelation wildcards; doubling thresholds (60/120/...); auto-claim generic GPs with instant effects (eurekas, production, gold+envoy, faith, unit levels) — no named individuals/patronage |
| Religion | 🟡 | pantheon at 25 faith (6 beliefs, exclusive); prophet + holy site founds (max 4, classic names); follower/founder beliefs; missionaries (faith-buy, 3 spreads, +200 pressure); passive pressure ±9 tiles; majority at 50; no theological combat/apostles |
| Trade routes & roads | ✅ | wiki capacity (Foreign Trade +1/hub-or-harbor city, +2 merchant republic); vanilla per-district yield table; traders lay roads (cost 1, bridge rivers 🟡); 30-turn duration; war/capture cancels |
| Diplomacy (deals, alliances, grievances) | 🟡 | war/peace only |
| Loyalty + governors (R&F) | 🟡 | population pressure ±9 tiles, capitals immune, defection to strongest neighbor at 0; governor titles from civic milestones (3), +8 loyalty +1 amenity; no promotions/named governors |
| Natural wonders + goody huts | 🟡 | 3 wonders (reef/crater lake/pantanal), feature-based, impassable/crossable, tile yields, discovery era score (+3 first finder); huts ~1/40 land tiles with gold/faith/eureka/inspiration rewards |
| Zone of control | ✅ | melee exerts (same domain, not over river banks), cities/encampments exert all-adjacent, cavalry ignores, civilians drop all MP |
| Movement: MP paid up front, min-1-tile, river +2 MP | ✅ | river surcharge on entering channel tile (tile-model 🟡) |
| Support units; corps/armies | 🟡 | battering ram (full melee dmg vs ancient walls) + siege tower (melee bypasses walls thru medieval), support stacking class; corps/armies n/a — unlock at Nationalism, beyond current renaissance-era content |
| Victory: domination, science, score, religious, culture, diplomatic | ✅ | culture = tourism vs domestic tourists (🟡); diplomatic = world congress every 30 turns from medieval era, most envoys+suzerainties gains 2 DVP, 6 wins (🟡 vs GS resolutions) |
| Eras, era score, golden/dark ages | 🟡 | world era from leader's tech+civic count; era score from wonders/GPs/camps/captures/religion/pantheon; golden +10% / dark -5% yields on transition (R&F-style, simplified thresholds) |

Next batch: policy cards + government slots, trade routes, great people,
city-state envoys, culture victory.
