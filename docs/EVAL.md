# Evaluation baselines

Recorded reference numbers so strength and health regressions are visible.
Re-run the battery after any AI or rules batch and compare against the most
recent entry; update this file (append, don't overwrite) when numbers move
for an understood reason. All commands are deterministic for a given build
and seed set.

```bash
civvis soak --games 12 --players 4 --turns 350 --start-seed 100
civvis tournament --ais advanced,basic,random --games 30 --players 4 --turns 250 --quiet
victory_eval --games 2 --players 2       # all six targets, stock turn limits
ai_eval advanced basic --pairs 100 --seed 4000   # paired, low-variance
ai_eval advanced basic --pairs 100 --difficulty emperor   # against the ladder
```

## 2026-07-22 — commit fba4785 (session F baseline)

**Test suite:** 421/421 lib tests pass.

**Soak** (12 × 4-player, 350 turns, seeds 100-111): 12/12 completed, no
panics, no anomaly flags.

| Victory | Games | Notes |
|---|---|---|
| religious | 8 | t170–t291 |
| score (turn cap) | 4 | no other victory landed by t350 |

**victory_eval** (2 seeds × 6 targets): 12/12 PASS — every victory type is
reachable end-to-end by the real game loop (science t689/t957, culture
t432/t458, religious t86/t159, diplomatic t425/t395, domination t583/t132,
score t301).

**Findings (exploit-hunt / balance):**

1. **Religious victory dominates advanced self-play** — 8/12 at four
   players when no AI is told to pursue a specific victory. Either faith
   output is over-tuned relative to Civ 6 pacing, or the AIs under-invest
   in religious defense (inquisitors/theological combat) relative to how
   hard they push their own religion. A strong optimizer will farm this
   lane; worth a targeted balance/defense pass.
2. **Nobody ever dies** — majors_alive was 4/4 in all twelve games and all
   six city-states survived every game. Wars happen but never conclude in
   elimination at 4p/350t. Real Civ 6 games kill civilizations; the
   military AI is likely too conservative about finishing sieges or picks
   unwinnable-but-safe postures. (`victory_eval --target domination`
   passes, so conquest works when explicitly pursued.)
3. Score-cap games (4/12) suggest the turn-350 horizon is short for
   science/culture lanes at 4p — consistent with victory_eval reaching
   science at ~t700-950 and culture at ~t430-460 on small 2p maps.

**Tournament** (30 × 4p × 250t, seed 0, K=24; 25% win rate = parity):

| AI | Elo | Games | Win rate |
|---|---|---|---|
| advanced | 1154.5 | 36 | 56% |
| basic | 1022.4 | 43 | 19% |
| random | 823.1 | 41 | 5% |

`random` winning at all (2 games) is worth an eye: at the 250-turn cap the
score ranking can crown a passive seat in a table with no advanced player.
The sanity floor holds, but sub-350-turn tables measure score racing as
much as victory play.

**ai_eval** (`advanced basic --pairs 25 --seed 4000`, mirrored 2p, avg
159t): advanced wins 39/50 (78%), ahead on every economic diagnostic
(score 194 vs 139, tech 15.3 vs 11.4, production 37 vs 25). Victory mix:
religious 27/50 across both seats — the same religious dominance the soak
shows at 4 players, now confirmed head-to-head (basic banks 452 faith it
never converts, advanced converts at less than a third of that).

## 2026-07-22 — religious balance batch (session F, after 311119a)

Rules fix (stock Civilopedia rule): faith-purchased religious units now
adopt their own city's majority religion, so non-founders can field
adopted-faith Missionaries; Missionaries spread the unit's faith and
reconvert home first. AdvancedAi: every strategy now runs a home religious
defense, triggered while conversion is in progress (any rival faith at 60%
of the city's strongest pressure), not after the majority already flipped.

**Soak, same seeds 100-111** (12 × 4p × 350t):

| Variant | religious | score-cap | other |
|---|---|---|---|
| before batch | 8 | 4 | 0 |
| majority-flip trigger only | 11 | 1 | 0 |
| 60%-pressure trigger (shipped) | **3** | 9 | 0 |

The majority-flip variant proved the timing thesis: by the time a rival
faith holds a city, the pressure race is lost and defense spending is
wasted. Triggering at 60% pressure turns religion from a near-free lane
(11/12) into a contested one (3/12); games now run long and 9/12 hit the
turn cap on score, consistent with the earlier finding that 350 turns is
short for the science/culture lanes at four players. 427/427 tests.

**Mirrored 2p is intentionally less affected** (advanced beat basic 39/50
with religious 27/50 both before and after): with only two faiths on the
map, a converted non-founder rarely has a third adopted faith to buy —
matching real Civ 6, where a duel against a committed religious player is
genuinely hard to defend without your own religion.

**StrategicAi first probe** (`ai_eval strategic advanced --pairs 8 --seed
5000`, mirrored 2p, avg 177t): strategic wins 9/16 (56%), all nine on
score, with markedly stronger empires (score 230 vs advanced's usual ~195,
military 257 vs ~103, tech 18.8 vs 15.3). Small sample — treat as "at
least parity"; a 25-pair run should decide promotion to exhibition seats.

## 2026-07-22 — score formula + game length (session F, after 0bd6734)

Two rules fixes, both verified against the Civilopedia:

1. **Score formula was not Civ 6's.** The engine scored 10/city, 3/Citizen,
   3/district, 2/civic and **1 point per unit**. Gathering Storm scores
   3/civic, 5/city, 2/district (4 unique), 1/building, 1/Citizen, 5/Great
   Person, 10 for founding a religion + 2 per foreign follower city,
   2/technology, 15/wonder, plus Era Score — and nothing for units. Ties now
   resolve through the shipped tiebreaker chain. This is not cosmetic: score
   decides every capped game and feeds `evolve` fitness, Elo placement, and
   `StrategicAi::position_value`, so the AI was being paid to hoard units
   and population rather than build wonders and Great People.
2. **Standard speed is 500 turns**, and the engine already models
   Standard-speed costs everywhere — but the default-speed CLI path kept
   each command's historical budget (simulate 250, soak 120), so a
   "Standard" game played half a game and ended on an arbitrary cutoff.

**Soak, same seeds 100-111, now at the stock 500 turns:**

| Outcome | 350t, old score | 500t, GS score |
|---|---|---|
| religious | 3 | 8 |
| score (turn cap) | 9 | **1** |
| diplomatic | 0 | 2 |
| culture | 0 | 1 |

Games are now decided by real victories instead of an arbitrary cutoff —
only one of twelve reaches the turn limit, and four different victory
types appear. 436/436 tests.

**Top open balance lead: religion still wins 8/12 at full length.** The
60%-pressure home defense fixed the *early* runaway (it was 11/12 before),
but over a full 500 turns religion still converts the world more often
than any other lane completes. Next probe: whether Missionary/Apostle
spread pressure and the passive ±9-tile pressure match the stock numbers,
and whether two defensive Missionaries is simply too small a budget.

**ai_eval advanced vs basic under the corrected score** (25 pairs, seed
4000): advanced 33/50 (66%), down from 78% under the old formula — the
old scoring was inflating advanced's edge by paying for its larger unit
count and population. 66% is the honest number.

**StrategicAi promotion gate** (25 pairs, seed 6000): strategic 27/50
(54%) over advanced. Above parity but inside the noise band at n=50, and
each decision costs six full rollouts. Verdict: keep as the builtin
`strategic` for further work; **not** promoted to the exhibition default.

## 2026-07-22 — the difficulty ladder as an external yardstick (session U)

Elo between our own bots is a closed system: it says one bot is 130 points
better than another, and nothing about whether either is any good. Now that
difficulty is a real setting (see [UNCIV_LESSONS.md](UNCIV_LESSONS.md)),
`ai_eval --difficulty <level>` gives an outside reference — the challenger
plays the *human* side of the handicap and its opponents play the AI side, so
"beats Emperor" means what a Civ player expects it to mean. Seats still swap,
which moves the challenger around the map rather than moving the handicap.

Reference run, `ai_eval advanced basic --pairs 6 --turns 90`, challenger
`advanced` against handicapped `basic`:

| Level | Challenger seat-win% | Challenger score | Opponent score |
|---|---|---|---|
| prince (no handicap) | 58.3% | 90.8 | 75.2 |
| deity | 16.7% | 80.9 | 154.7 |

Read that as calibration, not as a result: six pairs at 90 turns is a smoke
test, and Deity hands the opposition +80% Production and Gold, +32% Science,
Culture and Faith, +3 Combat Strength, four free boosts per era and seven
extra opening units. The point is that the axis exists and moves the right
way; the number worth tracking over time is the highest level the current
agent still beats at `--pairs 100`.
