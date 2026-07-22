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
