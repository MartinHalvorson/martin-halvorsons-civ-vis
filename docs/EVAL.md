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
## 2026-07-22 — learned-policy rung and the real training loop (session F)

PyTorch installed (CPU build), so the loop below was **run**, not just
written. `civvis selfplay` now also writes `dataset.csv` (the 25 scalar
`evolve::features` + win label), closing the chain:
`selfplay -> dataset -> train_valuenet.py -> valuenet.json -> agents`.

**Value net, 60 self-play games / 5,998 samples:** val BCE **0.336** against
a constant-predictor baseline of 0.562 — the scalar net genuinely learns.
(Caveat: `train_valuenet.py` still splits by sample. `train_spatial.py`
splits by game, which is correct; the scalar trainer should follow.)

**Spatial net, same pipeline:** the by-game split is the honest one and it
changed the story completely. A per-sample split reported **98.8%**
accuracy; splitting by game gives **75.0%**, which is exactly the
majority-class baseline on a 4-player export (one seat per game wins), with
a *worse* BCE than the constant predictor. The trainer now always prints
the baseline and a `beats_baseline` verdict so this cannot be misread
again. Conclusion: the pipeline is correct, 24-60 games is nowhere near
enough data. This is the concrete argument for AI_GAPS item 8's scale.

**PolicyAi (`policy`) vs AdvancedAi** — 6 pairs, seed 7000, mirrored 2p:

| AI | wins | score | gold | military |
|---|---|---|---|---|
| advanced | 7/12 (58.3%) | — | — | — |
| policy | **5/12 (41.7%)** | 169.0 | 137.8 | 158.6 |

The learned policy **does not beat the scripted agent**. It plays full
legal games with the net choosing actions (one-ply value search over the
real action space), but it is weaker — and its Gold (137.8 vs advanced's
typical ~325) shows why: a one-ply evaluator happily spends the treasury on
whatever looks locally good. This is the expected first rung, and it maps
the remaining work precisely: a policy head trained on far more self-play,
multi-ply search, and credit assignment past one action.

## 2026-07-23 — the GPU loop actually runs (session F)

**The Blackwell card needs `cu128`.** `pip install torch` (default) gives a
CPU build; the `cu126` wheel reports `cuda True` but dies at the first
kernel with `no kernel image is available for execution on the device`,
because its arch list stops at `sm_90` and an RTX PRO 6000 Blackwell is
`sm_120`. The working recipe is:

```bash
pip install --force-reinstall --no-deps torch     --index-url https://download.pytorch.org/whl/cu128
python -c "import torch; print(torch.cuda.get_arch_list())"  # must list sm_120
```

`--force-reinstall` matters: pip sees the same version number across index
URLs and otherwise does nothing.

**First real GPU training run** — 144 self-play games, 18,405 samples, 28
games held out *by game*:

| | val BCE | verdict |
|---|---|---|
| constant-predictor baseline | 0.5623 | — |
| trained net (`torch/cuda`) | **0.3685** | **BEATS baseline** |

Both trainers now hold out whole games and print this baseline comparison,
so a leaked or useless model cannot be mistaken for a good one.

## 2026-07-23 — condemnation: correct rule, no balance change (session F)

Civ 6's standing military answer to a religious offensive is condemning
foreign Missionaries with military units (only while at war, or when the
World Congress allows it). The engine had the rule implemented correctly,
but `AdvancedAi` only invoked it when an enemy religious unit *already
shared its tile* — which essentially never happens — so the counter was
dead code. Military units near home now step onto an adjacent intruder and
condemn it.

**It did not change the balance.** On the same seeds (100-107) the victory
mix is identical before and after: 5 religious, 2 diplomatic, 1 culture.
That is worth stating plainly rather than claiming a win — and it points at
the actual blocker: condemnation requires *being at war* with the religious
leader, and these AIs largely are not. The remaining lever for the religion
runaway is therefore in `victory_denial`'s willingness to open a war (or
push the Congress vote) against a runaway faith, not in the condemn
mechanic itself.

## 2026-07-23 — the learned policy overtakes the scripted agent (session F)

The first `PolicyAi` rung lost 5/12, and a better net did not fix it: with
the leak-free GPU-trained value net it still scored **8/20 (40%)** against
`advanced`. So the bottleneck was never the net's calibration — it was the
one-ply architecture. A one-ply evaluator cannot see the cost of a
multi-turn commitment, so production, research and purchases all look
nearly free, which is exactly why the agent's treasury kept collapsing
(Gold 138 against advanced's ~325).

Restricting the net to action kinds whose whole effect lands **this turn**
(`TACTICAL_KINDS` — moves, attacks, strikes, fortify, pillage, condemn) and
leaving multi-turn economy to the scripted layer flips the result:

| Run | policy | advanced | policy Gold |
|---|---|---|---|
| unrestricted, 10 pairs | 8/20 (40%) | 12/20 (60%) | 138 |
| tactical-only, same 10 pairs | 11/20 (55%) | 9/20 (45%) | 685 |
| tactical-only, 25 fresh pairs (seed 8200) | **28/50 (56%)** | 22/50 (44%) | — |

Two independent seed sets agree, and the Gold column confirms the
diagnosis rather than just the outcome. This is the first configuration in
which a learned component beats the scripted agent head-to-head, and it
states the design rule plainly: give the net the decisions whose
consequences it can actually observe, and let search or scripting own the
ones it cannot.

## 2026-07-24 — threat-aware macro routing

`StrategicAi` had silently regressed behind its scripted parent. On 25 mirrored
duel maps (`ai_eval strategic advanced --pairs 25 --seed 10000 --turns 180`),
the original search layer won only **14/50 (28%)**. It still produced more
Science and Production, but `advanced` won 32 religious games while the search
agent finished 22 seats committed to Science. The cause was structural:
30-turn rollouts modeled every rival as `BasicAi`, fallback evaluation used
score share, and the next macro review could be 40 turns away from a sudden
victory threat.

The corrected router now:

- rolls candidate lanes against `AdvancedAi`, so counterfactual opponents
  exert the same victory pressure as the real benchmark;
- interrupts periodic search on public 0–100 victory-race progress, with the
  adaptive planner's 78% / 15-point margin and earlier whole-civilization
  religious warning;
- preserves invested Astrology/Holy Site/Prophet paths while a slot remains;
- treats an enabled duel religious race as mandatory victory geometry: only
  one foreign conversion is needed, so it commits while a Prophet is available
  and stays committed after founding;
- reports final explicit-target counts in `ai_eval`, making routing failures
  visible beside wins and economic diagnostics.

Same-seed result: **32/50 (64%)**, up 36 percentage points, with 30 religious
wins. A disjoint 25-map holdout (`--seed 12000`) reproduced it at **31/50
(62%)**. The duel prior is disabled in multiplayer: on 12 mirrored four-player
maps (`--seed 11000`), the generalized changes raised game wins from **8/24
(33.3%)** on unchanged mainline to **10/24 (41.7%)**, while StrategicAi kept
its Production and Science advantages. These are promotion signals, not a
claim of universal strength; the four-player sample should grow with the
league archive.

## 2026-07-24 — paired confidence and Elo-equivalent promotion gates

Raw wins from the two seat-swapped games on one generated map are correlated:
they share terrain, resources, civilizations, and much of the resulting game
geometry. `ai_eval` therefore treats each mirrored map as one independent
cluster. The challenger receives 1 for a sweep, 0.5 for a split, 0 for a
reverse sweep, and half credit for an individual game that ends without a
winner. It reports a conservative 95% Wilson interval using the number of maps,
not the larger and misleading number of games.

The same score and interval are transformed through the standard logistic Elo
expectation curve, so every comparison now has an Elo-equivalent point estimate
and confidence range. Promotion requires at least 20 independent maps and a 95%
lower score bound above 50%; an upper bound below 50% retains the incumbent.
Everything else is explicitly `INSUFFICIENT` or `INCONCLUSIVE` rather than being
promoted from a noisy headline win rate.

Re-evaluating the threat-aware Strategic benchmark illustrates the difference.
Its **32/50 (64%)** result came from 8 Strategic sweeps, 16 split maps, and one
Advanced sweep. The paired estimate is **64%, 95% CI 44.5–79.8%**, equivalent
to **+100 Elo, CI -38 to +238**. That is strong directional evidence and a large
point improvement over the old router, but it correctly remains
`INCONCLUSIVE` at 25 maps because the confidence interval overlaps parity.

## 2026-07-24 — adaptive control inside Strategic rollouts

The multiplayer router compared six forced victory targets but omitted its own
`AdvancedAi` parent as a candidate. It therefore had to commit at every review,
even when all explicit lanes were worse than remaining adaptive. On 20 mirrored
four-player maps (`--seed 13000`), the old router finished 35 of 80 seats on
Domination but produced one domination win; it scored **18/40 (45%)** overall.

`StrategicAi` now rolls out the adaptive parent beside every enabled explicit
lane. A target must beat adaptive by more than one score-share point before it
can take control, and a later review can return to adaptive without discarding
campaign or unit-role memory. Prophet commitments, duel religious geometry, and
urgent counter-routing still override the economic comparison.

On the same 20 maps the new router scored **21/40 (52.5%)**, converting three of
four Advanced sweeps into splits. On a disjoint holdout (`--seed 15000`) it
scored **17/40 (42.5%)** versus the old router's **16/40 (40%)**. Combined, the
change raises multiplayer results from **34/80 (42.5%, -53 Elo)** to **38/80
(47.5%, -17 Elo; paired-map 95% CI 32.9–62.5%)** and reduces forced targets from
75/80 final seats to 53/80. This is a replicated five-point improvement, not a
promotion over Advanced: the combined interval still overlaps parity.

The duel specialization is unchanged on its exact 25-map regression set:
**32/50 (64%, +100 Elo)** with 30 religious wins. Two broader value-shaping
experiments were rejected before this design: a victory-progress blend fell to
11/40, and generic commitment hysteresis fell to 15/40 on the first seed block.

## 2026-07-24 — full-game plan tracing

`ai_eval` now observes the reported victory target after every major-player AI
turn. It reports target exposure, switches per seat-game, the dominant target
over the whole game (final target breaks ties), and seat outcomes conditioned
on that dominant target. Bots without a `PlanReport` are explicitly
`unreported`; an Advanced/Strategic agent with no explicit target is
`adaptive`. This fixes a measurement problem in the earlier experiments: a
final target says nothing about how most of the game was played.

Fresh four-player holdout (`strategic advanced --pairs 20 --players 4
--turns 180 --width 24 --height 16 --seed 17000`):

| Result | Strategic | Advanced |
|---|---:|---:|
| Game wins | 19/40 (47.5%) | 21/40 (52.5%) |
| Paired-map Elo | -17 (95% CI -165..+131) | reference |
| Seat win rate | 23.8% | 26.2% |
| Target switches / seat-game | 2.27 | 0.00 |
| Adaptive exposure | 43.9% | 100.0% |

Strategic's final labels counted 31 domination seats, but only 10 seats were
domination-dominant over the full game. Those seats won 1/10; the 18
religion-dominant seats won 9/18. This is diagnostic association, not a causal
estimate—the router selects targets from the position, so hard positions can
select a particular lane. It is nevertheless a concrete ablation lead: test a
stricter proactive domination commitment while retaining urgent victory
denial, then accept it only on paired holdout maps.
