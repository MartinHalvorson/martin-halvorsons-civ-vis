# AI Development Guide

The engine exists so you can develop advanced AI strategies against a
Civ-6-like game without a UI in the loop.

## In-process Rust agents

Implement the `Ai` trait; you get the full `Game` API (`legal_actions`,
`apply`, all queries):

```rust
use civvis::{ai::{Ai, run_game, AdvancedAi}, game::{Action, Game}};

struct MyBot;
impl Ai for MyBot {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        // inspect g, apply actions...
        let _ = g.apply(pid, &Action::EndTurn);
    }
}

let mut g = Game::new(4, 28, 18, 1, 250, 2);
let mut ais = AdvancedAi::fleet(&g);
run_game(&mut g, &mut ais);
```

`AdvancedAi` is the default major-civilization agent. It maintains persistent
grand strategy, campaign and threat state; coordinates research, diplomacy,
recovery production, settlement, improvements, trade, and military focus; and
falls back to the stable city governor for routine production. `BasicAi`
remains the frozen deterministic control and the lightweight agent for
city-states/barbarians. Both read full state (cheat on fog); fair-play agents
should restrict themselves to `civvis::obs::observation(&game, pid)`.

## Coordinated forces

During a war, `AdvancedAi` partitions military and support units by movement
domain and command distance. Each resulting `ForceGroup` is an inspectable army
or fleet order with a common anchor, campaign objective, focus-fire target,
readiness, local strength ratio, and one of five postures: muster, advance,
engage, hold, or recover. Movement then scores the order as a whole: melee
screens ranged and siege units, roles keep useful engagement depth, weak local
odds discourage unsupported advances, and stragglers rejoin their group.
Orders are recomputed before every combat-unit step, so a kill, retreat, newly
opened line, or local-power swing immediately changes the remaining force's
focus and movement instead of waiting for the next turn.
Positional ties favor taking at least one useful step each turn; remaining in
place is reserved for recovery, attacks, explicit defensive/muster positions,
or cases where every legal move is materially worse. At peace, troops that
have finished exploring rotate among persistent frontier patrol posts instead
of accumulating indefinitely at the capital.

```rust
for force in ai.force_groups() {
    println!("{:?} {:?}: {:?}", force.domain, force.posture, force.units);
}
```

The planner is domain-generic: fleets intercept hostile naval units and choose
reachable coastal approaches to land objectives. New domains can use the same
group/order pipeline instead of adding another independent-unit AI.

## Genetic strategy evolution

`Weights` is a complete genome for the advanced agent. Alongside economy,
diplomacy, production, and exchange evaluation, it includes command radius,
muster radius/readiness, cohesion, focus fire, screening, role spacing,
objective pressure, local-superiority caution, and withdraw/rejoin thresholds.

```bash
cargo run --release -- evolve --generations 100 --pop 24 --games 12 \
  --players 4 --threads 8 --dir evolved
civvis tournament --ais evolved,advanced,advanced_v1,basic --games 80
```

Every genome plays the real `AdvancedAi` against the reigning champion on
shared map seeds and rotating seats. Multiplayer training tables also draw from
`archive.json`, a hall of fame of prior champions, to reduce cyclic strategies
and catastrophic forgetting. Fitness combines final score share with a smaller
kill/capture signal so battlefield doctrine can learn before it decides a whole
game. Elites survive; fitter genomes are crossed and mutated within per-gene
bounds. A candidate only replaces `best.json` after a sequential match confirms
a higher win rate and it does not regress on a generation-independent,
fixed-seed holdout benchmark. `population.json` resumes the run, `history.csv`
records training and holdout progress, and `dataset.csv` feeds value training.
Old checkpoints load with defaults for newly introduced genes and validation
metadata.

## Elo tournaments

```bash
civvis tournament --ais advanced,basic --games 40 --players 4
```

For lower-variance two-player measurement, the paired evaluator runs every map
twice with seats swapped and reports outcome plus economy/army diagnostics:

```bash
cargo run --release --bin ai_eval -- advanced basic --pairs 100 --seed 4000
```

```rust
use civvis::elo::{run_tournament, leaderboard, TourneyCfg, builtin_ai};
let names = vec!["basic".to_string(), "mybot".to_string()];
let pool = run_tournament(&names, |name, seed| match name {
    "mybot" => Box::new(MyBot),
    other => builtin_ai(other, seed),
}, &TourneyCfg::default());
println!("{}", leaderboard(&pool));
```

Multiplayer games score as pairwise Elo results by final placement
(K/(n-1) scaling). Deterministic given `cfg.seed`.

## External agents over HTTP (any language)

`civvis play --no-open --port 8765` exposes the JSON protocol:

- `GET /state` — observation for player 0 (fog applied) + `legal_actions`
- `POST /action` body `{"action": {"type": "end_turn"}}` — applies, runs the
  AI opponents, returns the new state (+`error` string on illegal actions)
- `GET /rules` — the full ruleset (techs, units, costs, ...)
- `POST /new` body `{"seed": 7, "num_players": 4}` — fresh game; selecting a
  player count also applies its full stock Civ VI map profile (4 = Tiny
  60×38/6 city-states, 6 = Small 74×46/9 city-states). Explicit `width`,
  `height`, or `num_city_states` fields override individual profile values.

Actions are plain JSON dicts identical to what `legal_actions` returns —
feed them straight into LLM tool-calling or an RL policy. One process per
concurrent game; the engine itself runs ~27k turns/sec single-core, so
in-process Rust agents are the fast path for self-play at scale.

## Evaluation tips

- Fix multiple seed sets; report paired win rate vs `basic` plus multiplayer Elo.
- Use `ai_eval` to catch regressions hidden by wins (stalled settlers, obsolete
  armies, unfinished queues, or weak science/culture output).
- Keep `random` in the pool as a sanity floor.
- `soak` flags anomalies (no tech progress, minor winners) across seeds.
