# Civ 6 mechanics coverage

Tracked first against the official [Gathering Storm Civilopedia](https://www.civilopedia.net/en-US/gathering-storm/concepts/intro/),
with secondary references used only where the in-game documentation omits a
numeric rule.
Status: ✅ implemented · 🟡 intentionally scoped relative to the full commercial ruleset.

> **In progress** (claimed by parallel sessions — check before starting a batch):
> - (none currently claimed)

| System | Status | Notes |
|---|---|---|
| Hex map, fog of war, terrain/features/resources | ✅ | all six stock Civ VI map-size profiles; connected rivers follow shared hex edges |
| Leaders & civ uniques | ✅ | all 8 civs: Trajan, Cleopatra, Pericles, Qin Shi Huang, Gilgamesh, Montezuma, Amanitore, Tomyris — signature ability each + 8 unique units (legion, hoplite, eagle warrior, war cart, pítati archer, maryannu chariot archer, saka horse archer, crouching tiger) replacing/blocking their base units |
| Rivers & fresh water housing (5/3/2) | ✅ | water values plus Palace, Aqueduct normalization, coastal Lighthouse bonus, and +0.5 from owned Farms, Pastures, Plantations, Camps, and Fishing Boats within 3 tiles |
| City growth curve, border expansion | ✅ | exact Housing headroom bands: 100% at +2, 50% at +1, 25% through -4, and no growth at -5 |
| Housing & amenities | ✅ | final Gathering Storm demand, satisfaction thresholds, yield/growth modifiers, Palace Amenity, connected unique luxuries allocated to the four neediest cities (six for Aztec), and local district/building/policy sources |
| Districts + adjacency (incl. river) | ✅ | all 19 universal and 16 unique Gathering Storm districts; exact source-by-source adjacency rounding, replacement-family inheritance, placement/cap rules, adjacency policy cards, appeal Housing, district/unique Great Person rates, active unique bonuses, and one-time effects |
| Wonders | ✅ | all 53 Gathering Storm World Wonders are world-unique, use legal map-tile placement, preserve occupied wonder tiles, and execute their data-driven yield, production, combat, Great Person, policy, housing, amenity, tourism, power, route, governor, and one-time effects |
| Tech + civics trees, Eureka/Inspiration | ✅ | complete 77-tech / 61-civic Gathering Storm trees from Ancient through Future; Ancient starts have no phantom Agriculture tech |
| Governments + policy cards | ✅ | wiki slot configs (chiefdom M1E1 … merchant republic/theocracy with six slots); 20 cards thru guilds; typed slots + wildcard overflow; obsoletion (Agoge→Feudal Contract); slot/unslot actions; effects: yields, prod-toward-item %, housing, amenities, maintenance, builder charges, city def/ranged, vs-barb CS, recon XP, Theocracy religious strength |
| Combat math (Civ 6 formula), XP/promotions | ✅ | exact damage/wounded-strength curve, terrain, cliffs, rivers, amphibious attacks, class matchups, ranged/bombard penalties, flanking/support, healing, XP and fortification; explicit promotion choice/heal/XP pause and the complete modeled promotion trees, including pillage/coastal-raid modifiers, cliff scaling, aircraft slots, interception, anti-air defense, movement, sight, ZOC, range, kill rewards and extra attacks |
| Fortify, city/Encampment ranged strikes, walls | ✅ | Gathering Storm wall pools (100/level), explicit repair projects, melee 15% / ranged 50% / siege 100% vs walls, independent Encampment 100 HP/strike/ZOC/pillage state, ordinary ranged floors HP at 1 while Bombard may deplete but not capture, walls razed on capture |
| City conquest, occupation, razing & liberation | 🟡 | mandatory keep/raze/liberate decision after melee capture; Capitals cannot be razed; razing removes borders/districts and carries 3× capture Grievances; liberation restores the original owner (including eliminated civs/city-states) and grants Diplomatic Favor; occupied cities generate recurring Grievances, occupied original Capitals and world Grievances reduce Favor, and eliminating a city-state angers other majors. Peace-deal cession and Emergencies remain simplified |
| Naval units and embarkation | ✅ | 12 standard ships across four classes with their stock tech/civic unlocks; Builders embark at Sailing, Traders at Celestial Navigation, other land units at Shipbuilding, Ocean at Cartography, and +1 sea Movement at Mathematics; embarked strength 10 and land units may attack back onto land with the amphibious penalty |
| Barbarians (camps, raiders, rewards) | ✅ | Scouts search for major cities, report discoveries back to their home camp, trigger a 15-turn alert, and accelerate raider spawning; clearing a camp removes its alert state |
| City-states + envoys | ✅ | influence by gov tier (100 pts = envoy); type bonuses at 1/3/6 (capital +2, +2 per matching district); suzerain = 3+ & strict lead; city-states follow their suzerain into war and peace; direct war clears envoys |
| Great People | ✅ | 27 named Great People across all nine classes, era-aware market progression, district/building/policy GPP, escalating recruitment thresholds, Gold/Faith patronage, retirement tracking, Great Works, and individual data-driven effects |
| Religion | ✅ | 33 beliefs across all five belief classes; pantheons and map-scaled religion caps; Prophet founding, Holy Cities, Missionaries, Apostles, Gurus, Inquisitors and Warrior Monks; nine Apostle promotions; evangelization, inquisition, heathen conversion, health-scaled spreads, theological combat/ZOC, Guru healing, heresy removal, condemnation, relics and pressure changes |
| Trade routes & roads | ✅ | Foreign Trade grants the base route; Markets/Lighthouses and their unique replacements add capacity; Merchant Republic adds +2; unique districts inherit the vanilla per-district yield table; Traders lay roads, Military Engineering bridges roaded river crossings, routes last 30 turns, and war/capture cancels them |
| Diplomacy (deals, alliances, grievances) | 🟡 | bilateral Quick Deals support lump Gold, GPT, Diplomatic Favor, immediate strategic-stockpile quantities, temporary Luxury access, and directional Open Borders with mutual valuation, expiry, and war cancellation. Denouncements unlock formal wars after five turns; casus belli, capture/occupation grievances and decay, friendship, defensive-pact joins, five unique Alliance types with level thresholds/bonuses, Favor income, World Congress ballots, and keep/raze/liberate city decisions are active. City/Great Work trading and bespoke leader agendas remain outside this ruleset's content scope |
| Loyalty + governors (R&F) | ✅ | population pressure ±9 tiles, cultural-alliance suppression, capitals immune, and defection at zero Loyalty; all seven named Governors have appointment/assignment state, title costs, +8 Loyalty, complete five-promotion trees, and active city/empire effects |
| Natural wonders + goody huts | ✅ | map-scaled 2–7 unique Natural Wonders use their connected one-to-four-tile footprints, terrain/feature placement rules, passability, tile yields, and per-wonder discovery era score; huts use map-scaled placement and gold/faith/eureka/inspiration rewards |
| Zone of control | ✅ | innate from turn 1 for the modeled roster; explicit per-unit capability, native domains, river blocking, defensible districts, religious ZOC, cavalry immunity, and class-specific stop behavior |
| Movement: MP paid up front, min-1-tile, river +2 MP | ✅ | surcharge applies only when crossing the exact shared river edge |
| Support units; corps/armies | ✅ | battering ram + siege tower; same-tile military/civilian/support/religious links and naval escorts move together; Nationalism Corps/Fleets (+10) and Mobilization Armies/Armadas (+17) preserve the most experienced constituent's XP/promotions |
| Victory: domination, science, score, religious, culture, diplomatic | ✅ | science = Spaceport + Satellite → Moon → Mars → Exoplanet, then 50 light-years (repeatable laser projects add speed); domination = every foreign original capital after its keep/liberate decision; religious = strict majority in every living major; culture = visiting tourists exceed the best rival domestic total; diplomatic = 20 DVP through World Congress resolutions; score = highest score only at the turn limit |
| Eras, era score, golden/dark/heroic ages | ✅ | all nine world eras follow the leading tech/civic era; dynamic thresholds derive from the previous baseline and Historic Moments; Golden and Dark Ages apply their yield/loyalty rules, a Dark-to-Golden transition creates a Heroic Age, and era-appropriate Dedications expose one choice (three in a Heroic Age) with active movement, purchase, spread, production, yield, route and population effects |

Every system tracked by this coverage matrix has an executable engine path,
legal-action protocol, observation/save representation where applicable, and
Basic/Advanced AI handling. Rows marked 🟡 describe deliberate content-scope
boundaries rather than dormant or unsupported engine systems.

## Trade and Quick Deals

Press **D** in a human game to open Quick Deals. Sell, Buy, Luxury, and
Strategic filters compare every living major's current offer in one list,
sorted by the human player's net value. Each card exposes the exact terms and
both civilizations' positive equivalent-Gold gain; accepting revalidates the
same terms atomically, so a stale treasury, resource export, or declaration of
war cannot force a deal through.

Lump Gold, Diplomatic Favor, and strategic-resource stockpile quantities
transfer immediately. Gold per turn, usable Luxury access (including Amenity
allocation), and directional Open Borders last 30 turns. War terminates those
ongoing agreements and restores exported Luxury access, but does not reverse
an already completed strategic transfer. The engine rejects gifts and any
custom economic exchange for which either side's modeled gain is not positive.
AI civilizations periodically choose at most one of those same mutually
favorable offers.

Reference basis: the in-game [Trade, Demand, and Discuss Civilopedia entry](https://www.civilopedia.net/en-US/standard-rules/concepts/diplo_7/),
the directional [Open Borders Civilopedia entry](https://www.civilopedia.net/en-US/gathering-storm/concepts/diplo_9/),
and the compare-all-offers workflow of the [Quick Deals mod](https://steamcommunity.com/sharedfiles/filedetails/?id=2460661464).

## Combat and zone of control

Zone of control does **not** unlock through a technology or civic; it applies
from turn 1. The Military Tradition civic unlocks the separate +2-per-unit
flanking and support bonuses.

Every melee-capable land line in the roster exerts ZOC, including Scout,
cavalry, anti-cavalry, and Giant Death Robot units. Ranged and bombard land
units do not, unless a promotion such as Suppression grants it. Every naval
surface unit exerts ZOC, including naval ranged ships, Privateers, and Aircraft
Carriers; Submarines and Nuclear Submarines are the exceptions. City Centers,
the Encampment family, and Oppidums project into every adjacent land or water
tile until pillaged. Unit ZOC stays in the provider's native domain and cannot
cross a river, while defensible-district ZOC crosses rivers.

Cavalry (including ranged cavalry), Naval Raiders, and air units ignore
incoming ZOC. A linked civilian or support unit inherits that immunity from
its escort. Military and religious units that enter ZOC keep unused Movement
and may still attack, pillage, spread religion, or promote, but cannot move to
another tile that turn—even with move-after-attack. Civilian and support units
lose all remaining Movement and receive no follow-up actions. A unit that
begins its turn inside ZOC may leave as its first action, but cannot attack
first and then leave while the ZOC remains.

Religious units of different civilizations and different religions exert ZOC
against one another regardless of war; units of the same religion never do.
Defensible districts affect foreign religious units only while their owners
are at war.

Reference basis: the in-game [Civilopedia Zone of Control entry](https://www.civilopedia.net/en-US/standard-rules/concepts/movement_3/),
the detailed [Civilization VI ZOC rules](https://civilization.fandom.com/wiki/Zone_of_control_%28Civ6%29),
and the general hex-grid [zone-of-control definition](https://en.wikipedia.org/wiki/Zone_of_control).
