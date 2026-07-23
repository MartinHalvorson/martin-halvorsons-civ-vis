# Strategy league (Glicko-2 ratings + selection)

`civvis league` maintains a **persistent rated pool of high-level AI
strategies** and improves it over time: strategies earn Glicko-2 ratings by
playing multiplayer games, strong ones breed refined offspring, and
confidently weak ones retire. It answers two questions the one-shot
`tournament` command cannot: *how strong is each strategy, with an
uncertainty bar, accumulated across runs* — and *what does an even stronger
strategy look like*.

```bash
civvis league                      # play 10 rounds (resumes league/ if present)
civvis league --rounds 50 --games 16 --players 4 --seed 1
civvis league --standings          # print the table without playing
```

## Entrants

A strategy is either a built-in agent (`advanced`, `basic`, ...) or a
**parameterized AdvancedAi**: a 40-gene `Weights` genome plus an optional
fixed victory lane (`science`, `culture`, `religious`, `diplomatic`,
`domination`, `score`). A fresh league seeds itself with the anchor agents
`advanced` and `basic`, `advanced_v1`, one strategy per victory lane, and
the GA champion from `evolved/best.json` when present.

## Rating: Glicko-2, rounds as rating periods

Each round schedules `--games` tables of `--players` by dealing shuffled
passes over the active roster, so everyone plays a near-equal amount. A
finished game decomposes into pairwise win/loss results by placement
(winner first, then score), and the whole round updates at once as one
Glicko-2 rating period (start 1500, RD 350, vol 0.06, tau 0.5; the
implementation reproduces the worked example in Glickman's paper — see
`league::tests`). Glicko-2 rather than Elo because the roster churns:
newcomers carry a wide RD and converge in a few rounds, idle or benched
strategies grow uncertain instead of stale-precise, and retirement can
demand a *confident* rating. Rating periods also make the result
independent of game order within a round, so `--jobs` never changes
ratings (there is a test for byte-identical leagues at different job
counts).

## Selection

Every `--evolve-every` rounds (default 4):

- **Breed** `max(1, --pop / 4)` offspring. Parents are drawn from the
  top-rated half of genome-carrying strategies; a child is a uniform
  crossover of its parents' weights plus bounded mutation (the same
  operators `civvis evolve` uses), and mostly inherits a parent's victory
  lane (with some exploration). Offspring enter at 1500 ± 350 and must
  earn their place.
- **Retire** the lowest-rated strategies until the active roster is back
  under `--pop`, but only with evidence: never anchors, never anyone with
  fewer than 20 games or RD above 110. Retired strategies keep their
  history in the roster; only scheduling stops.

The two anchors (`advanced`, `basic`) are never retired, which pins the
scale: a league leader's margin over `advanced` is comparable across
hundreds of rounds even after every founder has been replaced.

## State on disk (`--dir`, default `league/`, gitignored)

- `league.json` — the roster: every strategy's kind/genome, rating, RD,
  volatility, record, lineage (`parents`), and status. The single source
  of truth; delete it to start a fresh league.
- `ratings.csv` — per-round rating history of active strategies (for
  plotting progress over time).
- `matches.csv` — every game: round, seed, end turn, victory type,
  placements.

Everything is deterministic for a given `--seed` and build, including
across resumed invocations (round RNGs derive from `(seed, round)`).

## Reading results honestly

- **Use the natural game length** (default `--turns 250`). At a 150-turn
  cap most games end as truncated score victories, which structurally
  favors score-lane strategies; the first 20-round trial run showed
  exactly that collapse.
- A rating is only as settled as its RD: 1800 ± 90 vs 1700 ± 35 is not a
  confident gap. The known `advanced` vs `basic` separation (~90-120
  points) is a sanity check any healthy league should reproduce.
- Ratings are relative to the current pool; cross-league numbers are not
  comparable. The anchors are the bridge.
