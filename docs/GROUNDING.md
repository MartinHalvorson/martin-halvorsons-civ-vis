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

## First pacing comparison (indicative, not yet controlled)

A 31-turn Civilization VI autoplay game (6 major players, default settings)
against `civvis simulate --players 6 --turns 31 --difficulty prince
--speed standard`:

| At turn 31 | Civilization VI | CIVVIS |
|---|---|---|
| cities | 1–2 | 1–2 |
| techs | 3 | 1–3 |
| units | 2–4 | 1–3 |
| score | 25–42 | 16–27 |

Expansion, tech pace and army size land in the same place. Score does not:
CIVVIS' leader scores 27 where the real game's leads 42, and the whole band
sits low.

This is one game a side, on different maps with different civilizations, so it
is a lead rather than a result. It is a *cheap* lead to chase, because
`Game_PlayerScores.csv` decomposes every player's score, every turn, into the
categories that produced it — civics, empire, tech, great people, religion,
wonders, trade — so the gap can be attributed to a category rather than
guessed at. That decomposition against CIVVIS' own score model is the next
measurement.

## Playing league genomes in the real game

`tools/civ6_strategy.py` exports a league strategy's economic policy — the
scripted opening (`open0..open3`), `city_target`, `settler_stop_turn`,
`builder_per_city`, `mil_per_city`, and the district priorities — and the mod
plays it for one player while the shipped AI handles everything else. That
scopes the test to the part of the genome that can transfer: in Civilization VI
the game's AI moves the units, so the tactical two-thirds of the genome has
nothing to drive.

Orders are verified against the game's own `City_BuildQueue.csv`, not against
the agent's own claims. Driving Maverick2, the capital built builder, monument,
settler, settler — matching the orders issued, each taking effect as the
previous item completed.

### First A/B: the league's top two

60 turns each, six major players, default settings, the genome driving
player 0:

| genome | league elo | league games | score | field mean | margin | cities |
|---|---:|---:|---:|---:|---:|---:|
| Maverick2 | 1791 ± 31 | 216 | 112 | 66.0 | **+46.0** | 2 |
| WildCard10 | 1823 ± 50 | 21 | 40 | 62.2 | **−22.2** | 1 |

The league's **top-rated** strategy lost to its own field. The
second-rated one beat its field by a wide margin.

One game each, on different maps, is an anecdote — Civilization's variance
across starts is larger than these gaps. What makes it worth acting on is that
the league's own data says the same thing independently.

### What that points at — and a correction

My first reading of this was that the ratings were unsupported by the records
behind them: WildCard10 tops the league on a 19% winrate over 21 games, where
random in a six-player game is 16.7%, and Maverick6 sits at 1707 on 13%.

**That reading was wrong, and the yardstick was the mistake.** This league does
not rate wins. `src/league.rs` decomposes every finished game into *pairwise
results by placement*, so a strategy that reliably finishes second of six earns
a high rating with a low winrate, entirely legitimately. Measured against what
the system actually rates, it behaves better than against wins — on the games
that could be matched to the roster, Spearman correlation with mean placement
is **+0.68** against **+0.56** for winrate. Correlating a placement rating with
winrate and calling the gap a defect was a category error.

Two things do survive, both smaller:

**The standings rank on point estimates and ignore their own uncertainty.**
WildCard10 is 1823 ± 50 on 21 games; Maverick2 is 1791 ± 31 on 216. Those
intervals overlap heavily, so "WildCard10 is the top strategy" is not something
its own rating deviation supports. This is not cosmetic: `civvis play --league`
seats each civ with its *best-rated* strategy and the exhibition HUD converts
those ratings into win chances, so a newcomer whose rating has not converged
changes which strategies play and what a spectator is told. Ranking on a
conservative bound (rating − k·RD) rather than the point estimate would fix it,
and is the standard treatment.

**The committed snapshot and the run log do not reconcile.** Of the 54
strategies in `data/league/league.json`, 33 — including the entire top nine —
appear in *no* game in the `matches.csv` produced by the league worktree, even
though the snapshot credits them with 21 to 216 games each. The likeliest
explanation is simply that the two files come from different runs (the memory
of that run records a different leader and rating than the snapshot carries).
But until that is established, the provenance of the committed snapshot is
unverified, and it is the file every league-seated game reads. That is worth
settling before any of these ratings are trusted.

### What the Civ 6 runs actually established

Not that WildCard10 is weak — two games on different maps cannot show that. What
they established is that the harness works: a league genome drives production in
a real game, verifiably, and produces a measurable outcome against a field. The
+46 / −22 split is the first datapoint, not a result. A fixed map seed and
several games per genome is what turns it into one.
