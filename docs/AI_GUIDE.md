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
grand strategy, victory, campaign, force-group, settlement, builder, and threat
state; coordinates research, civics, policies, governments, Secret Societies,
diplomacy, production, spending, religion, trade, and unit orders; and falls
back to the stable city governor for routine production. `advanced_v1`
preserves the pre-upgrade agent as a frozen regression control. `BasicAi` is
the deterministic lightweight agent used by city-states and barbarians. All
three read full state (cheat on fog); fair-play agents should restrict
themselves to `civvis::obs::observation(&game, pid)`.

Default strategic planning also reads the public victory-race information for
every rival. An imminent science or score win becomes a military-denial target,
a culture lead triggers defensive Culture and Tourism investment, a religious
lead is met with theological pressure (or military denial when no religion is
available), a Diplomatic Victory lead redirects Favor and envoys, and captured
foreign Capitals force a recovery posture. This urgency can override a nearer,
weaker distraction, while an explicit benchmark victory target remains fixed.
Economic plans normally persist for five turns to avoid strategic thrashing,
but a surprise major war, a newly threatened city, or an imminent rival
victory interrupts that window and triggers an immediate reassessment.
Incoming diplomatic proposals are priced against their Gold transfer,
grievances, current strategy, alliance type, war position, and campaign
fatigue; the agent no longer accepts an exploitative friendship payment or a
non-peace pact with the rival it is trying to deny.
Congress ballots follow the same plan: Diplomatic agents back themselves for
World Leader, other strategies steer target rewards toward the civilization
furthest from victory, and competition votes predict the strongest public
candidate instead of mechanically voting for the current player.

## Victory targeting and full-game validation

Every major can be assigned an explicit victory objective. Targeted agents
coordinate research, civics, policy cards, production, diplomacy, spending,
and unit orders around that objective; city-states and barbarians continue to
use the lightweight agent.

The six pipelines are concrete rather than score labels. Science reserves a
Spaceport and completes the launch chain; Culture builds a Theater Square
network, recruits cultural Great People, trains and routes capacity-aware
Archaeologists, reaches the Conservation/Professional Sports tourism unlocks,
improves tourism tiles, and sends promoted Rock Bands to the best risk-adjusted
foreign venues; Religion founds, enhances, defends,
and spreads its faith while reconverting its own core first;
Diplomacy prioritizes Favor, envoys, alliances, city-state liberation, and
World Congress; Domination coordinates production and force objectives; Score
balances expansion and near-term empire value. Society choice supports the
same goal: Hermetic Order for Science, Voidsingers for Culture/Religion, and
Owls of Minerva for economic, diplomatic, and conquest plans.

```rust
use civvis::ai::{run_game, AdvancedAi, VictoryTarget};
use civvis::game::Game;

let mut game = Game::new(4, 28, 18, 7, 1_200, 0);
let mut ais = AdvancedAi::fleet_targeting(&game, VictoryTarget::Science);
run_game(&mut game, &mut ais);
assert_eq!(game.victory_type.as_deref(), Some("science"));
```

`victory_eval` runs the real game loop without injecting progress or invoking
victory checks directly. It exits nonzero if the resulting victory type does
not exactly match the requested target:

```bash
cargo run --release --bin victory_eval -- --target all --games 3 \
  --start-seed 9000 --players 2
```

`--target` accepts `science`, `culture`, `religion`, `diplomacy`,
`domination`, `score`, a comma-separated subset, or `all`. Per-condition turn
limits reflect the length of each race; `--turns` overrides them for bounded
diagnostics. Map dimensions can be overridden with `--width` and `--height`.

### Validated regression baseline (2026-07-22)

The current engine passes exact, unassisted full-game victories for every
target on two independent seeds. On seeds 20000 and 20001 respectively, the
winning turns were Science 1021/940, Culture 559/586, Religion 79/177,
Diplomacy 305/335, Domination 82/136, and Score 301/301.

Against the frozen `advanced_v1` control on mirrored current-engine maps,
Advanced v2 won 61–39 across 100 four-player games and 26–24 across 50
eight-player games: 87–63 combined (58.0%). Use these as regression baselines,
not universal strength claims; repeat them when rules or evaluation settings
change.

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

Military units also follow class-specific doctrine rather than sharing one
generic policy. Recon units keep exploring during wars unless a clearly good
attack is available; assault and high-strength units accept thinner combat
advantages; mobile and naval-raider units exploit pillage opportunities;
ranged units preserve firing depth; siege prioritizes cities and districts;
support stays close to the line; fighters prefer interception patrols while
bombers prefer strikes and useful rebasing. If no recon unit exists, one
ordinary combat unit explores at peace instead of sending the whole army.

```rust
for force in ai.force_groups() {
    println!("{:?} {:?}: {:?}", force.domain, force.posture, force.units);
}
```

The planner is domain-generic: fleets intercept ships and embarked enemies,
screen embarked settlers, and choose adjacent coastal approaches so ranged
ships can reduce defenses before naval melee captures. Coastal empires treat
Sailing, Shipbuilding, Celestial Navigation, and Cartography as a capability
chain, keep a role-balanced exploration/escort fleet, and pursue current naval
upgrades during maritime wars. Settlers retain globally scored, route-checked
colony targets across multiple turns and linked ships lead them over water.
New domains can use the same group/order pipeline instead of adding another
independent-unit AI.

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
  player count also applies its full stock Civ VI map profile: 2 = Duel
  (44×26/3 city-states), 4 = Tiny (60×38/6), 6 = Small (74×46/9), 8 = Standard
  (84×54/12), 10 = Large (96×60/15), and 12 = Huge (106×66/18). Explicit
  `width`, `height`, or `num_city_states` fields override individual profile
  values.

Actions are plain JSON dicts identical to what `legal_actions` returns —
feed them straight into LLM tool-calling or an RL policy. One process per
concurrent game; in-process Rust agents remain the fast path for self-play at
scale. On an Apple M5 Max, the current release Advanced-v2 workload measured
1,173 turns/sec for `benchmark --games 100 --turns 100` (two players, 20×14).
Throughput varies materially with map size, era, player count, and agent; older
tens-of-thousands figures describe a much smaller historical rules workload.

## Evaluation tips

- Fix multiple seed sets; report paired win rate vs `basic` plus multiplayer Elo.
- Use `ai_eval` to catch regressions hidden by wins (stalled settlers, obsolete
  armies, unfinished queues, or weak science/culture output).
- Keep `random` in the pool as a sanity floor.
- `soak` flags anomalies (no tech progress, minor winners) across seeds.
