# Competitive Civ VI baseline

CIVVIS treats "tournament rules" as two distinct layers:

1. **Official Gathering Storm gameplay.** Every ordinary game mechanic remains
   in scope, including pre-game team rules. This is the deterministic engine
   baseline tracked in [MECHANICS.md](MECHANICS.md) and audited in
   [FIDELITY.md](FIDELITY.md).
2. **Community tournament packages.** Current competitive events commonly use
   Better Balanced Game (BBG), Better Balanced Starts (BBS), Multiplayer Helper
   (MPH), and a spectator/map package. These are versioned mods and lobby tools,
   not one permanent Firaxis ruleset. CPL describes them as community-maintained
   tools used to create a level playing field, while its World Cup uses 4v4 teams
   and a rotating map pool.

## Implemented competitive behavior

- Official free-for-all and pre-game team relationships and team victory rules.
- Deterministic seeded games, stock map sizes/speeds, selectable maps and
  victories, full save/restore, fog-filtered observations, and an omniscient
  spectator view.
- Headless agent tournaments, paired-seat evaluation, and replayable action
  logs.
- Ruleset overlays through `--mods`, with reference validation before play.
- The Gathering Storm world systems a tournament lobby actually configures:
  Ages with both halves of every Dedication, climate change and sea-level
  rise, and random natural disasters on the five-step intensity slider
  (`--disasters 0..4`). Events run their lobbies at a chosen intensity
  rather than off, so this is a pre-game setting a tournament preset must
  pin like any other; `--disasters 0` reproduces a no-disaster lobby
  exactly, leaving only the CO2-driven sea-level rise.

## The CPL lobby, setting by setting

The league publishes the lobby it plays, and every line of it is a pre-game
setting an engine either reproduces or does not. Where CIVVIS reproduces one,
this is the flag that pins it:

| CPL setting | CIVVIS |
|---|---|
| Game Speed: Online | `--speed online` |
| Limit Turns: By Game Speed | default (`data/speeds.json`; Online is 250) |
| Map Size: Firaxis Default for Number of Players | default (`MapSize::for_players`) |
| City States: Firaxis Default for Map Size | default (`--city-states` overrides) |
| All Victory Conditions: ENABLED | default (`--victories` pins a subset) |
| Barbarians ON for FFA, OFF for Teamers | `--barbarians on\|off` |
| Tribal Villages: ENABLED | default (`data/goody_huts.json`) |
| Teams Share Visibility: ENABLED | implied by `--teams` |
| Start Era: Ancient | default |
| All Game Modes: DISABLED | default; `--game-modes apocalypse,secret_societies` opts back in |
| Duplicate Civs and Leaders: ALLOWED | seats past the eighth reuse the roster |
| Start Position: Balanced | **not ported, and not a default** — see below |
| Temperature / Rainfall / Sea Level: Standard | not exposed by the generator; these three are at their defaults, so the gap is expressiveness rather than divergence |
| World Age: New | **not a default either**, and the setting no longer exists in current Civ VI — see below |
| Turn Timer: MPH Dynamic · Turn Mode: Simultaneous | out of scope: sequential engine |
| No Gold or strategic-resource trading, no military alliances | referee policy, not a rule |

### The two map lines are not defaults

Every other unreproduced line above sits at its stock value, so a CIVVIS game
differs from a CPL game only in what it cannot *say*. Two do not, and they are
the reason a CIVVIS map is not a CPL map:

- **Start Position: Balanced.** "CIVVIS has fairness spacing" undersold this
  and is corrected here. `balanced_major_spawns` scores whole layouts, not
  spacing alone: minimum separation, landmass coverage, Voronoi territory,
  nearest-neighbour range, and `start_quality` per site — then hill-climbs each
  start over its neighbourhood to remove outliers the sampler never offered.
  Separation and coverage are ranked ahead of quality on purpose, so quality
  balance is what gives way when they conflict.

  What that leaves, measured over 24 standard 8-player maps against the twelve
  tiles a capital actually works (independently of the generator's own scorer):

  - **No seat bias.** Per-seat means run 39.3–41.7 against a grand mean of 40.6
    — under 3.2% drift, i.e. nothing. Which seat you are handed does not decide
    what land you get.
  - **Within-map best-worst spread is ~12.2 points, about 30% of the mean.**
    That is the cost of playing Start Position: Standard rather than Balanced,
    and it is the real remaining gap. It is a residue of the ranking above, not
    an oversight.

  Anyone closing this line should re-measure both numbers rather than trust
  these: the honest target is cutting the spread without disturbing separation
  or coverage, and the seat-bias figure is the guard that the fix did not
  introduce an ordering artefact.
- **World Age: New.** More Hills — community measurement puts it as high as +75%
  — with a mixed effect on Mountains. **Do not implement `--world-age`:** the
  setting was removed from Civ VI and replaced by separate Mountain Level and
  Hill Level controls, so porting it would be porting a feature the game no
  longer has. If this line is ever closed, close it as those two controls, and
  re-read CPL's published lobby first — it may simply predate the change.

For a stock 4v4 match, which is a teamers lobby and so plays without
barbarians:

```bash
civvis play --players 8 --teams 0,0,0,0,1,1,1,1 --speed online \
  --barbarians off --spectate
```

## Remaining tournament-specific gaps

| Layer | Current boundary |
|---|---|
| BBG balance/content | Overlay files can change existing data, but many BBG leader, civilization, policy, belief, and wonder changes require the general modifier interpreter in [MODIFIERS.md](MODIFIERS.md). CIVVIS currently ships 8 leaders rather than the full tournament roster. |
| Balanced starts and maps | The stock-style generator balances whole layouts — separation, coverage, territory, neighbour spacing and per-site start quality, with a hill-climb pass — across four map scripts, and leaves no seat bias. It still leaves a ~30% best-worst spread within a map, because separation and coverage outrank quality; see the measurement above. Exact BBS/BBM start normalization, remap tokens, and the World Cup map rotation (including Highlands, Seven Seas, and Tilted Axis) remain to be ported. |
| Multiplayer Helper | Lobby validation, dynamic turn timers, ready checks, pause/remap voting, reconnect administration, concede detection, and tournament result reporting are client/server work rather than simulation rules. |
| Turn mode | The authoritative engine is sequential. Simultaneous-turn ordering, dynamic/hybrid turns during war, and network lockstep remain separate protocol work. |
| Event policy | Drafts, civilization bans, disconnect/reload policy, no-quit enforcement, scheduling, and referee decisions belong to a tournament harness, not `Game` state transitions. |

The practical implementation order is therefore: finish the modifier
interpreter and full civilization roster; import a pinned BBG release as data;
port balanced-start/map algorithms; then add simultaneous multiplayer and the
lobby/referee workflow. A tournament preset should pin every mod version rather
than silently tracking latest releases, so old matches remain reproducible.

## Sources

- [Civilization Players League](https://cpl.gg/) — current competitive community
  and its maintained mod stack.
- [CPL in-game rules](https://cpl.gg/rules/in-game-rules/) — the published lobby
  settings tabulated above.
- [Civ VI World Cup](https://cpl.gg/civilization-world-cup/) — 4v4 format and map
  rotation.
- [Official team overview](https://www.civilopedia.net/en-US/standard-rules/concepts/teams_1/)
  and [team diplomacy](https://www.civilopedia.net/en-US/gathering-storm/concepts/teams_2/).
- Official team victory pages for
  [science](https://www.civilopedia.net/en-US/standard-rules/concepts/victory_3/),
  [culture](https://www.civilopedia.net/en-US/standard-rules/concepts/victory_4/),
  [domination](https://www.civilopedia.net/en-US/standard-rules/concepts/victory_2/),
  [religion](https://www.civilopedia.net/en-US/gathering-storm/concepts/victory_5/),
  and [score](https://www.civilopedia.net/en-US/gathering-storm/concepts/victory_6/).
