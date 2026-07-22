# Ten gaps between today's AI and world-class Civ 6 play

State as of 2026-07-22: the engine is complete enough to matter (all six
victory types, zero ❌ rows in MECHANICS.md, ~27k turns/sec), and the AI
stack has four rungs — `RandomAi`, `BasicAi` (29 GA-evolved weights,
anchor-stabilized evolution in `evolve.rs`), `AdvancedAi` (scripted grand
strategy / campaigns / threat state), and `NeuralAi` (distilled value net
judging war declarations via rollouts). Evaluation exists (`tournament`,
`ai_eval` paired runner, `soak`). That is a real foundation — and every rung
of it is still a scripted bot with garnish. Ranked, most impactful first:

1. **No learned policy.** Everything the AI does turn-to-turn comes from
   hand-written heuristics; the only learning is 29 scalar weights and one
   value net consulted for one decision type. World-class play means a policy
   network trained by self-play RL (AlphaZero/PPO-style) choosing among the
   engine's actions. The ceiling of the current approach is "best possible
   scripted bot," which is nowhere near it.

2. **No search.** `NeuralAi::consider_war` is the only lookahead in the
   codebase — everything else is greedy. The roadmap's MCTS baseline was
   never built. Civ turns are combinatorial, so raw MCTS over unit-level
   actions won't work; the gap is hierarchical search over macro-decisions
   (settle here, tech path X, war on Y now vs +10 turns) with fast scripted
   rollouts underneath — exactly the machinery the war check already
   prototypes, generalized.

3. **No spatial representation.** `evolve::features()` is 25 global scalars;
   the map never reaches any learned component. Tactics, city placement, and
   terrain play are unlearnable without a tile-grid observation (hex conv or
   transformer) plus per-unit embeddings and action masks. The roadmap's
   "observation tensors for RL" item is the prerequisite for gaps 1 and 2.

4. **Tactical combat micro is heuristic.** Focus fire, terrain/river/ZOC
   exploitation, siege escort, retreat-and-heal cycles, kill-securing — all
   threshold rules today (the barb chase-no-attack bug was this class).
   Combat micro is where every scripted Civ AI, Firaxis included, bleeds the
   most Elo; a local battle solver (small search or learned policy over the
   engaged cluster) is the single biggest per-system win available.

5. **The AIs cheat on fog.** `BasicAi`/`AdvancedAi` read full `Game` state.
   A world-class agent plays from `obs::observation(pid)`: belief state over
   unseen territory, remembered enemy units, inferred opponent science/army
   from what's visible, scouting valued as information gain. No memory or
   belief infrastructure exists yet — and honest-obs play is also what makes
   the agent portable to real Civ 6, which never grants omniscience.

6. **No long-horizon victory routing.** Rollout horizon is 12 rounds; games
   run 300-500 turns. `AdvancedAi`'s grand-strategy state picks postures but
   nothing plans a victory line ("culture win via these 3 wonder cities by
   T280") and replans as evidence arrives. Credit assignment across
   hundreds of turns is the hard RL problem here; explicit goal decomposition
   (victory route → milestones → per-city build orders) is how comparable
   systems made it tractable.

7. **Diplomacy is war/peace only.** No deals, tribute, alliances, joint
   wars, denouncements, or negotiated peace terms — the only 🟡 in
   MECHANICS.md that removes an entire strategic dimension. Dominant human
   play leans on diplomacy constantly (buy time, sell luxuries, bribe wars).
   Engine work must land first; then the AI needs a valuation model for
   deals. Also the price of entry for ever playing humans credibly.

8. **Training infrastructure doesn't reach the hardware.** `evolve` is a
   single-machine CPU GA; the value net was trained by an offline script on
   a CSV. The rig has a 96GB RTX Pro 6000 sitting idle. Needed: parallel
   self-play workers feeding a GPU trainer (the engine's speed makes this
   viable today), replay buffers, league play with past champions (the
   anchor bot is a first step against strategy cycling), and periodic gated
   promotion — the loop, not just the pieces.

9. **Evaluation has no external calibration, and the sim has exploitable
   seams.** Elo is measured only within the in-house pool; nothing says what
   "beats AdvancedAi 65%" means against Deity-grade opposition. Worse, a
   strong optimizer will farm the 🟡 simplifications (score-victory margins,
   placement-free wonders, simplified congress) — mastering CIVVIS quirks,
   not Civ 6. Needed: per-victory-type win matrices, exploit-hunting soak
   analysis, fidelity spot-checks against the wiki, and human games as the
   ground-truth benchmark.

10. **The late game doesn't exist.** Content stops at renaissance plus a
    space-race stub: no industrial→information eras, corps/armies, flight,
    late wonders, promotion effects, apostle combat, or late culture
    machinery (national parks, rock bands). Half of Civ 6's strategy space —
    and most of its victory endgames as actually played — is missing, so an
    agent trained here masters a truncated game. Lowest rank not because it
    is small but because gaps 1-3 pay off even on the truncated game, and
    content can land incrementally in parallel.

The through-line: 1-3 are one project (representation → search → learned
policy), 4-6 are what the learned agent must be good *at*, 7 and 10 are
engine surface area, and 8-9 are the loop that turns compute into strength
and proves it. Sequence the first three; the rest parallelize across
sessions the way the rules batches did.
