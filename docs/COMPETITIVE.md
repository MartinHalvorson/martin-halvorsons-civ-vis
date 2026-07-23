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

For a stock 4v4 match:

```bash
civvis play --players 8 --teams 0,0,0,0,1,1,1,1 --speed online --spectate
```

## Remaining tournament-specific gaps

| Layer | Current boundary |
|---|---|
| BBG balance/content | Overlay files can change existing data, but many BBG leader, civilization, policy, belief, and wonder changes require the general modifier interpreter in [MODIFIERS.md](MODIFIERS.md). CIVVIS currently ships 8 leaders rather than the full tournament roster. |
| Balanced starts and maps | The stock-style generator has fairness spacing and four map scripts. Exact BBS/BBM start normalization, remap tokens, and the World Cup map rotation (including Highlands, Seven Seas, and Tilted Axis) remain to be ported. |
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
