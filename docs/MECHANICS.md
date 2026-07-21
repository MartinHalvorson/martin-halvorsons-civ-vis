# Civ 6 mechanics coverage

Tracked against the [Civilization Wiki](https://civilization.fandom.com/wiki/Civilization_VI).
Status: ✅ implemented · 🟡 simplified · ❌ not yet.

| System | Status | Notes |
|---|---|---|
| Hex map, fog of war, terrain/features/resources | ✅ | tile-based rivers (🟡 vs edge rivers) |
| Rivers & fresh water housing (5/3/2) | ✅ | wiki values; oasis counts as fresh |
| City growth curve, border expansion | ✅ | |
| Housing & amenities | ✅ | luxuries global (🟡 vs 4-city rationing) |
| Districts + adjacency (incl. river) | ✅ | 8 district types |
| Wonders | 🟡 | 9 wonders, world-unique, effect engine (growth %, builder charges, unit levels); no tile placement |
| Tech + civics trees, Eureka/Inspiration | ✅ | 29 techs / 14 civics, through renaissance |
| Governments | 🟡 | flat effects; policy cards ❌ (next up) |
| Combat math (Civ 6 formula), XP/promotions | 🟡 | flat +5/level vs promotion trees |
| Fortify, city ranged strikes, walls | ✅ | wall HP not a separate pool |
| Embarkation (Shipbuilding) | ✅ | embarked strength 10, cannot attack |
| Barbarians (camps, raiders, rewards) | ✅ | no scout-alert mechanic |
| City-states | 🟡 | defensive minors; envoys/suzerain ❌ |
| Great People | ❌ | |
| Religion (pantheons, beliefs, units) | ❌ | faith yield + faith purchases only |
| Trade routes & roads | ❌ | |
| Diplomacy (deals, alliances, grievances) | 🟡 | war/peace only |
| Loyalty, governors (R&F/GS) | ❌ | |
| Natural wonders, goody huts | ❌ | |
| Zone of control, formations/corps | ❌ | |
| Victory: domination, science, score | ✅ | culture/religious/diplomatic ❌ |
| Eras, era score, golden ages | ❌ | |

Next batch: policy cards + government slots, trade routes, great people,
city-state envoys, culture victory.
