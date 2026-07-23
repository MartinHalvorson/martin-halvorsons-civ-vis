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

## Players

Every strategy plays under a **username themed to what it plays**, listed
with its Elo on the leaderboard: founders keep fixed handles
(`JackOfAllTrades` = advanced, `TrainingWheels` = basic, `TechPriest` =
science lane, `Warmonger` = domination, ...) and bred offspring draw a
fresh handle from their victory lane's pool (`LabRat`, `SiegeLord`,
`PointHoarder2`, ...), so a name tells you the strategy at a glance.
Handles are unique per league and deterministic; rosters saved before
usernames existed are backfilled on load. `civvis league --standings`
prints the ranked player table — username, current Elo ± RD, strategy,
record, birth round, status.

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
counts). A live server rating one finished game uses the same code with
idle ageing switched off — see "Watching players in the game HUD".

## Civ-specific ratings

Not every civ wants to play the same way, so besides its overall rating
every strategy keeps a **per-civ Glicko table** (`civ_elo`): its skill
specifically when drawing Rome, Egypt, ... (civs are fixed per seat in
`Game::new`, so every league game feeds both tables). Opponents are
measured by their global rating, which keeps civ numbers on the overall
scale. Civ tables are sparse — they update only in periods actually
played — and a table needs 5 games before it outranks the global rating
for display and seating.

- `civvis league --civ Rome` — who plays Rome best, ranked by Rome elo.
- `civvis league --civs` — each civ's current champion strategy.

## Watching players in the game HUD

`civvis play --spectate --league league/` seats every major civ with its
best-rated available strategy (distinct specialists per civ) and the
spectator HUD lists, per player: **civ, league username + strategy, its
elo** (civ-specific when settled, ±RD on hover) **and the elo-implied
expected win chance** against the table — compare against who actually
wins to audit the ratings over time. That last number is a share of the
one win a table has to give (`elo::win_shares`), so the seats sum to
100%; averaging the pairwise expectations instead would put every seat
near 50% and could never be checked against a winner. Without
`--league`, a `league/` dir in the working directory still labels the
default fleet with the "advanced" entrant's elo; the AIs themselves are
unchanged.

Add `--league-record` and each finished game is rated into that roster
as its own one-game rating period: the table moves as the exhibition
plays, and the next game seats from the ratings the last one produced.
Only the six seats that played are touched — a league round schedules
the whole roster, so a missing strategy really idled and its RD should
grow, but a six-seat game is not an idle period for everyone who could
not have entered it, and ageing them per game would pin the roster at
maximum uncertainty within an afternoon. The roster is re-read from disk
at the moment of recording and seats are matched by strategy *name*, so
a game long enough to outlive a concurrent update writes its result on
top rather than reverting it. Results also append to `matches.csv` and
`ratings.csv` beside `league.json`.

A snapshot of a finished league lives in the repo at `data/league/`
(see its README for provenance), so any checkout — including other
machines — can show rated, named players out of the box. The spectator
supervisor (`tools/spectator_supervisor.py`) defaults to `--league auto`,
which seeds a runtime copy of that snapshot at the repo-root `league/`
path (gitignored) and records into it — the committed snapshot is the
starting position, not a file the exhibition rewrites. Delete that
directory to start again from the snapshot. Pass `--league off` to run
the exhibition unrated, `--no-league-record` to seat rated players
without moving their ratings, or `--league <dir>` to use a live local
league instead.

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
