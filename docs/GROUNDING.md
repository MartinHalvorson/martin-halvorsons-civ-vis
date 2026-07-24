# Grounding CIVVIS against a running Civilization VI

`docs/FIDELITY.md` measures CIVVIS' rules *data* against the game's shipped
database, and that audit now stands at **0 divergent fields across 27 tables**
against a real Gathering Storm installation. That is a strong result and a
narrow one: it proves the constants match. It says nothing about what either
engine *does* with them.

This document covers the other half — measuring CIVVIS against the game while
the game is running. The question it answers is not "is this number right"
but "does this rule behave the same way", and the evidence is a real game's
own logs.

## What the game gives you for free

Civilization VI is far more instrumented than it looks. With the options in
`tools/civ6_setup.py` turned on, an ordinary game writes a structured
telemetry suite to its `Logs/` directory, no mod involved:

| File | What it carries |
|---|---|
| `CombatLog.csv` | every resolved combat: both strengths, both modifiers, damage each side took |
| `Player_Stats.csv` | per turn, per player: cities, population, techs, civics, units, tiles owned/improved, every yield |
| `Game_PlayerScores.csv` | per-turn score, decomposed into the categories that produce it |
| `AI_CityBuild.csv`, `City_BuildQueue.csv` | what each city chose to build, and why |
| `AI_Research.csv`, `Game_Boosts.csv` | research choices and every Eureka/Inspiration as it fires |
| `AI_Behavior_Trees.csv`, `AI_Tactical.csv`, `AI_Operation.csv` | the shipped AI's own decision trace |
| `RandCalls.csv` | every draw from the random stream |

This is the ground truth. It is exact, it is per-turn, and it needs no screen
reading — which matters, because a pixel-scraped number that is wrong 1% of
the time is worse than no number at all when the thing being measured is a
1% divergence.

## Running the game unattended

Grounding needs many games, so a human cannot be in the loop. Three tools:

```sh
python tools/civ6_setup.py --apply          # turn on the logging channels
python tools/civ6_run.py --install --turns 250 --tag run1
python tools/civ6_launch.py --start --play-now
```

`civ6_run.py` installs `tools/civ6_mod`, a mod that drives the engine's own
**autoplay manager** — the same mechanism Firaxis' automated tests use. The
game then plays every player with its own AI for a set number of turns and
writes a per-turn JSON record of every major player: score, cities and their
positions, districts built, what each city is producing, current research and
civic, unit counts, treasury.

`civ6_launch.py` handles the rest: stop the game and confirm it stopped,
launch it, click through the Aspyr launcher and the main menu, and wait for
the game core to prove it is up.

### Things about this build that fail silently

Each of these was found by measurement, and each one produces a *working-
looking* failure — which is why they are written down rather than fixed once
and forgotten.

- **Two user directories, nested, same name.** The live one is
  `.../Sid Meier's Civilization VI/Firaxis Games/Sid Meier's Civilization VI/`;
  the outer one is a leftover from older versions and is fully populated.
  Options written to the outer one parse cleanly and are ignored. This is what
  makes `EnableTuner 1` look like "the Mac build has no tuner". It does —
  FireTuner listens on ports 4318/4319 once the flag is set in the right file.
- **No user Mods directory is scanned.** Only the install's `DLC` tree and the
  Steam Workshop directory. A mod in `Mods/` is never discovered and nothing
  logs why. `civ6_run.py` installs into the DLC tree; `--uninstall` reverts it
  completely, and no shipped file is ever modified.
- **The mod database is an mtime cache.** A newly created mod folder is not
  noticed until `Mods.sqlite` is dropped, which reads as "Discovered 0 mods".
- **`<Context/>` vs `<Context></Context>`.** Both are valid XML; only the
  second binds a script.
- **The Lua sandbox has no `_G`.** `rawget(_G, ...)` raises at load and kills
  the whole script before it can report anything.
- **This build writes no `Lua.log`.** `print` from a mod goes nowhere;
  `Automation.Log` is the channel that survives, and its output lands in
  `Logs/Automation.log`.
- **`LoadGameViewStateDone` never reaches a mod.** The event fires before the
  mod's context exists, so anything gated behind it silently never runs. Setup
  hangs off the first turn event instead.
- **The leader intro screen blocks forever** on a BEGIN GAME click, with the
  mod loaded and no turn started — a hang with no cause in any log.

### Known gaps in the per-turn record

- `building` (what each city is currently producing) comes back empty. The
  production hash does not resolve against `GameInfo.Units/Buildings/Districts/
  Projects` the way the read assumes. Until it is fixed, build orders have to
  come from the game's own `City_BuildQueue.csv`, which does carry them.
- Fields that cannot be read report `-1`, never a plausible default. An earlier
  version returned `0` for a failed tech count, which produced a flat zero line
  for every player on every turn — data that looks real and silently invalidates
  any comparison built on it.

## First measurement: the combat damage roll

`docs/FIDELITY.md` recorded the damage roll as verified by argument rather
than by measurement:

> the damage roll: CIVVIS' 30·e^(Δ/25)·U(0.8, 1.2) is the same distribution as
> the shipped 24 base with its 1.0–1.5 spread

Both have mean 30 and span 24–36, so the argument holds *if* those shipped
constants are right — and they came from community documentation, not from the
game. `CombatLog.csv` makes it measurable. For each logged combat,
`tools/civ6_combat_fit.py` divides the observed damage by CIVVIS'
deterministic part and collects the residual:

    multiplier = observed_damage / (30 * exp(delta / 25))

If CIVVIS' formula is exact, those multipliers are uniform on [0.8, 1.2].

**Result over 194 combats** (Ancient-era units, from a 40-turn autoplay game;
an earlier late-game sample of 20 agreed):

```
  min      0.770
  median   0.982
  mean     0.978        (uniform [0.8,1.2] -> 1.0)
  max      1.194
  inside   187/194  (96.4%)
  implied base if the spread is right: 29.3   (CIVVIS uses 30.0)
```

The shape is right: the formula is the correct *form*, the strength delta is
the right driver, and the spread is close to CIVVIS'. Two things are worth
following up rather than declaring:

1. The mean sits about 2% low. That is roughly 2.7 standard errors from 1.0 if
   the samples were independent — but they are not; the same matchups repeat
   within a game, so the real uncertainty is wider than that figure suggests.
2. 7 of 194 rolls (3.6%) fall *outside* [0.8, 1.2] — all of them below. Under
   an exact model that fraction should be zero, so either the base is slightly
   below 30, the spread is slightly wider, or there is a modifier the log
   folds into `StrMod` differently than CIVVIS applies it.

Neither is settled by one game. Both are now questions with a measurement
attached instead of a citation, which is the point.

Rows that cannot test the formula are excluded and the exclusions are
reported: kills (the roll is clipped by remaining hit points, so the observed
value is a lower bound), zero-damage rows, and combats involving a district or
city, which add fortification rules this formula does not model.

## What comes next

- **Trajectory diffs.** `Player_Stats.csv` and the mod's per-turn record give
  a full trajectory per player. The same map settings and seed run in CIVVIS
  give another. Diffing them turn by turn localises a divergence to the turn
  it first appears, which is far more useful than a divergence in the final
  score.
- **The self-play meta.** CIVVIS' league found that religious victory dominates
  at 250 turns and that games end around turn 183–243. Both are checkable
  against real games, and both are the kind of claim a simulator gets wrong in
  an interesting way.
- **Strategy transfer.** The league's top strategies are ~40 scalar policy
  weights (city target, settle scoring, district priorities, war thresholds,
  opening build order). Those are portable; the tactical layer is not. Testing
  a genome in the real game means driving those levers through the mod and
  measuring the same outcome CIVVIS measures.
- **Micro-scenarios.** WorldBuilder plus the debug menu construct an exact
  state on demand, which is what a deterministic-transition test needs: same
  state, same action, compare the numbers.
