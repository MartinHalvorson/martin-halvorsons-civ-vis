# Civ 6 mechanics coverage

Tracked against the [Civilization Wiki](https://civilization.fandom.com/wiki/Civilization_VI).
Status: ✅ implemented · 🟡 simplified · ❌ not yet.

> **In progress** (claimed by parallel sessions — check before starting a batch):
> - (none currently claimed by session B; GUI wiring for religion/GP/trade panels is open)

| System | Status | Notes |
|---|---|---|
| Hex map, fog of war, terrain/features/resources | ✅ | tile-based rivers (🟡 vs edge rivers) |
| Rivers & fresh water housing (5/3/2) | ✅ | wiki values; oasis counts as fresh |
| City growth curve, border expansion | ✅ | |
| Housing & amenities | ✅ | luxuries global (🟡 vs 4-city rationing) |
| Districts + adjacency (incl. river) | ✅ | 8 district types |
| Wonders | 🟡 | 9 wonders, world-unique, effect engine (growth %, builder charges, unit levels); no tile placement |
| Tech + civics trees, Eureka/Inspiration | ✅ | 29 techs / 14 civics, through renaissance |
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
| Loyalty, governors (R&F/GS) | ❌ | |
| Natural wonders, goody huts | ❌ | |
| Zone of control | ✅ | melee exerts (same domain, not over river banks), cities/encampments exert all-adjacent, cavalry ignores, civilians drop all MP |
| Movement: MP paid up front, min-1-tile, river +2 MP | ✅ | river surcharge on entering channel tile (tile-model 🟡) |
| Formations/corps, support units | ❌ | |
| Victory: domination, science, score, religious | ✅ | religious = majority in >half of every civ's cities; culture/diplomatic ❌ |
| Eras, era score, golden ages | ❌ | |

Next batch: policy cards + government slots, trade routes, great people,
city-state envoys, culture victory.
