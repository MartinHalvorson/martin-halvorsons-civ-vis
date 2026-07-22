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
