//! Paired, seat-balanced head-to-head evaluator for built-in AIs.
use civvis::ai::Ai;
use civvis::elo::{builtin_ai, BUILTIN_AIS, EVAL_ONLY_AIS};
use civvis::game::{default_difficulty, Action, Game, GameOptions};
use civvis::rules::Rules;
use std::collections::{BTreeMap, BTreeSet};

const PROMOTION_MIN_MAPS: usize = 20;
const Z_95: f64 = 1.959_963_984_540_054;
/// Split a 5% two-sided error budget equally between promotion and retention.
const ANYTIME_TAIL_ALPHA: f64 = 0.025;
/// Fixed, pre-declared bets for a finite mixture e-process. At the parity null
/// every paired-map score is in [0, 1], so each factor
/// `1 + lambda * (score - 0.5)` is nonnegative and has expectation at most one
/// for the challenger-side test. Negating the bet tests the incumbent side.
const BET_LAMBDAS: [f64; 10] = [0.05, 0.10, 0.20, 0.35, 0.50, 0.70, 0.90, 1.15, 1.45, 1.80];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionVerdict {
    Insufficient,
    Promote,
    Retain,
    Inconclusive,
}

#[derive(Debug, Clone, Copy)]
struct PairedInference {
    maps: usize,
    score: f64,
    low: f64,
    high: f64,
    elo: f64,
    elo_low: f64,
    elo_high: f64,
    anytime: AnytimeEvidence,
    verdict: PromotionVerdict,
}

#[derive(Debug, Clone, Copy)]
struct AnytimeEvidence {
    challenger_peak_e: f64,
    incumbent_peak_e: f64,
    challenger_p: f64,
    incumbent_p: f64,
    challenger_crossed_at: Option<usize>,
    incumbent_crossed_at: Option<usize>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PairOutcomes {
    a_sweeps: usize,
    neutral: usize,
    b_sweeps: usize,
    mixed_with_draw: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DirectionalOutcomes {
    challenger_favored: usize,
    neutral: usize,
    incumbent_favored: usize,
}

fn elo_edge(score: f64) -> f64 {
    let bounded = score.clamp(1e-6, 1.0 - 1e-6);
    400.0 * (bounded / (1.0 - bounded)).log10()
}

fn game_score(winner: Option<usize>, seats: &[&str], challenger: &str) -> f64 {
    winner
        .and_then(|pid| seats.get(pid))
        .map_or(0.5, |name| if *name == challenger { 1.0 } else { 0.0 })
}

/// Challenger share of terminal Civilization score across the evaluated
/// seats. This is a bounded secondary development diagnostic, not a win and
/// never an input to the promotion verdict.
fn terminal_score_share(g: &Game, seats: &[&str], challenger: &str) -> f64 {
    let mut challenger_score = 0_i64;
    let mut total_score = 0_i64;
    for (pid, name) in seats.iter().enumerate() {
        let score = g.score(pid).max(0);
        total_score += score;
        if *name == challenger {
            challenger_score += score;
        }
    }
    if total_score > 0 {
        challenger_score as f64 / total_score as f64
    } else {
        0.5
    }
}

fn log_mean_exp(values: &[f64]) -> f64 {
    let largest = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    largest
        + values
            .iter()
            .map(|value| (*value - largest).exp())
            .sum::<f64>()
            .ln()
        - (values.len() as f64).ln()
}

/// Anytime-valid evidence against parity from a finite mixture of betting
/// martingales. The process starts with one unit of wealth; Ville's inequality
/// makes `1 / peak wealth` a valid upper bound on the probability of ever
/// observing at least this much evidence under the null, even if the evaluator
/// is rerun with longer prefixes and stopped when a result looks favorable.
///
/// Monitoring begins only at `PROMOTION_MIN_MAPS`, so a lucky sub-minimum prefix
/// cannot earn a permanent promotion before the representativeness floor.
fn anytime_evidence(scores: &[f64]) -> AnytimeEvidence {
    let mut challenger_log_wealth = [0.0; BET_LAMBDAS.len()];
    let mut incumbent_log_wealth = [0.0; BET_LAMBDAS.len()];
    let mut challenger_peak_log_e = 0.0_f64;
    let mut incumbent_peak_log_e = 0.0_f64;
    let mut challenger_crossed_at = None;
    let mut incumbent_crossed_at = None;
    let crossing_log_e = -(ANYTIME_TAIL_ALPHA.ln());

    for (index, raw_score) in scores.iter().enumerate() {
        debug_assert!((0.0..=1.0).contains(raw_score));
        let edge = raw_score.clamp(0.0, 1.0) - 0.5;
        for (bet, lambda) in BET_LAMBDAS.iter().enumerate() {
            challenger_log_wealth[bet] += (1.0 + lambda * edge).ln();
            incumbent_log_wealth[bet] += (1.0 - lambda * edge).ln();
        }
        let maps = index + 1;
        if maps < PROMOTION_MIN_MAPS {
            continue;
        }
        let challenger_log_e = log_mean_exp(&challenger_log_wealth);
        let incumbent_log_e = log_mean_exp(&incumbent_log_wealth);
        challenger_peak_log_e = challenger_peak_log_e.max(challenger_log_e);
        incumbent_peak_log_e = incumbent_peak_log_e.max(incumbent_log_e);
        if challenger_crossed_at.is_none() && challenger_log_e >= crossing_log_e {
            challenger_crossed_at = Some(maps);
        }
        if incumbent_crossed_at.is_none() && incumbent_log_e >= crossing_log_e {
            incumbent_crossed_at = Some(maps);
        }
    }

    AnytimeEvidence {
        challenger_peak_e: challenger_peak_log_e.min(f64::MAX.ln()).exp(),
        incumbent_peak_e: incumbent_peak_log_e.min(f64::MAX.ln()).exp(),
        challenger_p: (-challenger_peak_log_e).exp().min(1.0),
        incumbent_p: (-incumbent_peak_log_e).exp().min(1.0),
        challenger_crossed_at,
        incumbent_crossed_at,
    }
}

/// A conservative Wilson score interval with one observation per mirrored map.
///
/// Pair scores can be fractional because a split scores 0.5 and a game without
/// a winner is a draw. Treating each bounded map score as one Bernoulli-equivalent
/// observation uses the maximum variance for that mean, so the swapped games are
/// never falsely counted as independent evidence.
fn paired_inference(scores: &[f64]) -> PairedInference {
    let maps = scores.len();
    let anytime = anytime_evidence(scores);
    if maps == 0 {
        return PairedInference {
            maps,
            score: 0.5,
            low: 0.0,
            high: 1.0,
            elo: 0.0,
            elo_low: elo_edge(0.0),
            elo_high: elo_edge(1.0),
            anytime,
            verdict: PromotionVerdict::Insufficient,
        };
    }

    let score = scores.iter().sum::<f64>() / maps as f64;
    let n = maps as f64;
    let z2 = Z_95 * Z_95;
    let denominator = 1.0 + z2 / n;
    let center = (score + z2 / (2.0 * n)) / denominator;
    let radius = Z_95 * ((score * (1.0 - score) / n + z2 / (4.0 * n * n)).sqrt()) / denominator;
    let low = (center - radius).clamp(0.0, 1.0);
    let high = (center + radius).clamp(0.0, 1.0);
    let challenger_evidence = anytime.challenger_p <= ANYTIME_TAIL_ALPHA;
    let incumbent_evidence = anytime.incumbent_p <= ANYTIME_TAIL_ALPHA;
    let verdict = if maps < PROMOTION_MIN_MAPS {
        PromotionVerdict::Insufficient
    } else if challenger_evidence && incumbent_evidence {
        // Strong evidence in both directions means the run is nonstationary
        // or its map order is pathological, not that either AI is promotable.
        PromotionVerdict::Inconclusive
    } else if challenger_evidence && low > 0.5 {
        PromotionVerdict::Promote
    } else if incumbent_evidence && high < 0.5 {
        PromotionVerdict::Retain
    } else {
        PromotionVerdict::Inconclusive
    };

    PairedInference {
        maps,
        score,
        low,
        high,
        elo: elo_edge(score),
        elo_low: elo_edge(low),
        elo_high: elo_edge(high),
        anytime,
        verdict,
    }
}

fn pair_outcomes(scores: &[f64]) -> PairOutcomes {
    let mut outcomes = PairOutcomes::default();
    for score in scores {
        if (*score - 1.0).abs() < f64::EPSILON {
            outcomes.a_sweeps += 1;
        } else if score.abs() < f64::EPSILON {
            outcomes.b_sweeps += 1;
        } else if (*score - 0.5).abs() < f64::EPSILON {
            outcomes.neutral += 1;
        } else {
            outcomes.mixed_with_draw += 1;
        }
    }
    outcomes
}

/// Exact two-sided sign-test probability under equally likely directions.
/// Neutral mirrored maps are deliberately excluded: they contain effect-size
/// information but no evidence about which AI is directionally stronger.
fn exact_sign_p(a_favored: usize, b_favored: usize) -> f64 {
    let n = a_favored + b_favored;
    if n == 0 {
        return 1.0;
    }
    let tail = a_favored.min(b_favored);
    let n_f = n as f64;
    let mut log_choose = 0.0;
    let mut log_terms = Vec::with_capacity(tail + 1);
    for k in 0..=tail {
        log_terms.push(log_choose - n_f * std::f64::consts::LN_2);
        if k < tail {
            log_choose += ((n - k) as f64).ln() - ((k + 1) as f64).ln();
        }
    }
    let largest = log_terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lower_tail = largest.exp()
        * log_terms
            .iter()
            .map(|term| (*term - largest).exp())
            .sum::<f64>();
    (2.0 * lower_tail).min(1.0)
}

fn directional_outcomes(scores: &[f64]) -> DirectionalOutcomes {
    let mut outcomes = DirectionalOutcomes::default();
    for score in scores {
        if *score > 0.5 + f64::EPSILON {
            outcomes.challenger_favored += 1;
        } else if *score < 0.5 - f64::EPSILON {
            outcomes.incumbent_favored += 1;
        } else {
            outcomes.neutral += 1;
        }
    }
    outcomes
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PlanTrace {
    observations: usize,
    switches: usize,
    targets: BTreeMap<String, usize>,
    last_target: Option<String>,
}

impl PlanTrace {
    fn observe(&mut self, target: &str) {
        if self
            .last_target
            .as_deref()
            .is_some_and(|previous| previous != target)
        {
            self.switches += 1;
        }
        self.observations += 1;
        *self.targets.entry(target.to_string()).or_default() += 1;
        self.last_target = Some(target.to_string());
    }

    /// Target used on the most observed player-turns. A tie keeps the final
    /// target, matching the tournament's dominant-strategy attribution.
    fn dominant_target(&self) -> &str {
        let most = self.targets.values().copied().max().unwrap_or(0);
        if self
            .last_target
            .as_ref()
            .is_some_and(|target| self.targets.get(target) == Some(&most))
        {
            return self.last_target.as_deref().unwrap();
        }
        self.targets
            .iter()
            .find(|(_, turns)| **turns == most)
            .map_or("unreported", |(target, _)| target.as_str())
    }
}

fn plan_target(ai: &dyn Ai) -> &'static str {
    ai.plan_report().map_or("unreported", |plan| {
        plan.victory_target.unwrap_or("adaptive")
    })
}

fn run_traced_game(
    game: &mut Game,
    ais: &mut [Box<dyn Ai>],
    traced_players: usize,
) -> Vec<PlanTrace> {
    let mut traces: Vec<PlanTrace> = (0..traced_players).map(|_| PlanTrace::default()).collect();
    while game.winner.is_none() && game.turn <= game.max_turns {
        let pid = game.current;
        ais[pid].take_turn(game, pid);
        if pid < traced_players {
            traces[pid].observe(plan_target(ais[pid].as_ref()));
        }
        if game.winner.is_none() && game.current == pid {
            let _ = game.apply(pid, &Action::EndTurn);
        }
    }
    traces
}

#[derive(Default)]
struct TargetOutcome {
    games: usize,
    wins: usize,
}

#[derive(Default)]
struct Metrics {
    games: usize,
    wins: usize,
    score: f64,
    cities: f64,
    population: f64,
    techs: f64,
    civics: f64,
    districts: f64,
    buildings: f64,
    military: f64,
    gold: f64,
    faith: f64,
    tourists: f64,
    dvp: f64,
    envoys: f64,
    suzerainties: f64,
    military_units: f64,
    civilian_units: f64,
    religious_units: f64,
    food_yield: f64,
    production_yield: f64,
    science_yield: f64,
    culture_yield: f64,
    queued_cost: f64,
    settlers: f64,
    builders: f64,
    traders: f64,
    active_routes: f64,
    trade_capacity: f64,
    support_units: f64,
    missionaries: f64,
    victories: BTreeMap<String, usize>,
    final_targets: BTreeMap<String, usize>,
    dominant_targets: BTreeMap<String, usize>,
    target_outcomes: BTreeMap<String, TargetOutcome>,
    plan_turns: BTreeMap<String, usize>,
    plan_observations: usize,
    plan_switches: usize,
}

impl Metrics {
    fn record(&mut self, g: &Game, pid: usize, won: bool, final_target: &str, trace: &PlanTrace) {
        let cities = g.player_city_ids(pid);
        self.games += 1;
        self.wins += won as usize;
        *self
            .final_targets
            .entry(final_target.to_string())
            .or_default() += 1;
        let dominant_target = trace.dominant_target().to_string();
        *self
            .dominant_targets
            .entry(dominant_target.clone())
            .or_default() += 1;
        let outcome = self.target_outcomes.entry(dominant_target).or_default();
        outcome.games += 1;
        outcome.wins += won as usize;
        self.plan_observations += trace.observations;
        self.plan_switches += trace.switches;
        for (target, turns) in &trace.targets {
            *self.plan_turns.entry(target.clone()).or_default() += turns;
        }
        if won {
            *self
                .victories
                .entry(
                    g.victory_type
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                )
                .or_default() += 1;
        }
        self.score += g.score(pid) as f64;
        self.cities += cities.len() as f64;
        self.population += cities.iter().map(|cid| g.cities[cid].pop).sum::<i32>() as f64;
        self.techs += g.players[pid].techs.len() as f64;
        self.civics += g.players[pid].civics.len() as f64;
        self.districts += cities
            .iter()
            .map(|cid| g.cities[cid].districts.len())
            .sum::<usize>() as f64;
        self.buildings += cities
            .iter()
            .map(|cid| g.cities[cid].buildings.len())
            .sum::<usize>() as f64;
        self.military += g.military_power(pid);
        self.gold += g.players[pid].gold;
        self.faith += g.players[pid].faith;
        self.tourists += g.foreign_tourists(pid) as f64;
        self.dvp += g.players[pid].dvp as f64;
        self.envoys += g.players[pid]
            .envoys
            .iter()
            .map(|(_, count)| *count)
            .sum::<i64>() as f64;
        self.suzerainties += g
            .players
            .iter()
            .filter(|minor| minor.alive && minor.is_minor && g.suzerain_of(minor.id) == Some(pid))
            .count() as f64;
        self.active_routes += g.active_routes(pid) as f64;
        self.trade_capacity += g.trade_capacity(pid) as f64;
        for unit in g.units.values().filter(|u| u.owner == pid) {
            match unit.kind.as_str() {
                "settler" => self.settlers += 1.0,
                "builder" => self.builders += 1.0,
                "trader" => self.traders += 1.0,
                "missionary" => self.missionaries += 1.0,
                _ if g.rules.units[unit.kind.as_str()].class == "support" => {
                    self.support_units += 1.0
                }
                _ => {}
            }
            if g.rules.units[unit.kind.as_str()].class == "military" {
                self.military_units += 1.0;
            } else {
                self.civilian_units += 1.0;
            }
            if g.rules.units[unit.kind.as_str()].class == "religious" {
                self.religious_units += 1.0;
            }
        }
        for cid in &cities {
            let yields = g.city_yields(*cid);
            self.food_yield += yields.food;
            self.production_yield += yields.production;
            self.science_yield += yields.science;
            self.culture_yield += yields.culture;
            if let Some(item) = g.cities[cid].queue.first() {
                self.queued_cost += g.item_cost_for(pid, item);
            }
        }
    }
}

fn target_shares(metrics: &Metrics) -> String {
    metrics
        .plan_turns
        .iter()
        .map(|(target, turns)| {
            let share = 100.0 * *turns as f64 / metrics.plan_observations.max(1) as f64;
            format!("{target} {share:.1}%")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(args: &[String], flag: &str, default: &str) -> String {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn number(args: &[String], flag: &str, default: i64) -> i64 {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = args.first().map(String::as_str).unwrap_or("advanced");
    let b = args.get(1).map(String::as_str).unwrap_or("basic");
    assert_ne!(a, b, "choose two different AIs");
    for name in [a, b] {
        assert!(
            BUILTIN_AIS.contains(&name) || EVAL_ONLY_AIS.contains(&name),
            "unknown AI {name:?}: builtins {BUILTIN_AIS:?}; evaluator-only {EVAL_ONLY_AIS:?}"
        );
    }
    let pairs = number(&args, "--pairs", 50).max(1) as usize;
    let turns = number(&args, "--turns", 180).max(1) as u32;
    let players = number(&args, "--players", 2).max(2) as usize;
    let city_states = number(&args, "--city-states", 0).max(0) as usize;
    let width = number(&args, "--width", 24).max(8) as i32;
    let height = number(&args, "--height", 16).max(8) as i32;
    let seed = number(&args, "--seed", 4000).max(0) as u64;
    // The difficulty ladder as an external yardstick: the challenger plays
    // the human side of the handicap and its opponents play the AI side, so
    // "beats Emperor" means what a Civ player would expect it to mean.
    // Seats still swap, which moves the challenger around the map rather than
    // moving the handicap.
    let difficulty = text(&args, "--difficulty", &default_difficulty());
    if !Rules::embedded().difficulties.contains_key(&difficulty) {
        eprintln!("unknown difficulty {difficulty:?}");
        std::process::exit(2);
    }
    let mut totals: BTreeMap<String, Metrics> = [a, b]
        .into_iter()
        .map(|name| (name.to_string(), Metrics::default()))
        .collect();
    let mut total_turns = 0_u64;
    let mut pair_scores = Vec::with_capacity(pairs);
    let mut pair_terminal_scores = Vec::with_capacity(pairs);

    for pair in 0..pairs {
        let game_seed = seed + pair as u64;
        let mut pair_score = 0.0;
        let mut pair_terminal_score = 0.0;
        for swap in 0..2 {
            let seats: Vec<&str> = (0..players)
                .map(|pid| if (pid + swap) % 2 == 0 { a } else { b })
                .collect();
            let challenger_seats: BTreeSet<usize> = seats
                .iter()
                .enumerate()
                .filter(|(_, name)| **name == a)
                .map(|(pid, _)| pid)
                .collect();
            let mut g = Game::new_with(GameOptions {
                difficulty: difficulty.clone(),
                human_seats: challenger_seats,
                ..GameOptions::new(players, width, height, game_seed, turns, city_states)
            });
            let mut ais: Vec<Box<dyn Ai>> = g
                .players
                .iter()
                .map(|p| {
                    let name = if p.id < players { seats[p.id] } else { "basic" };
                    builtin_ai(name, game_seed + p.id as u64)
                })
                .collect();
            let traces = run_traced_game(&mut g, &mut ais, players);
            total_turns += g.turn as u64;
            pair_score += game_score(g.winner, &seats, a);
            pair_terminal_score += terminal_score_share(&g, &seats, a);
            // Legacy per-seat win metrics count a game nobody won as zero
            // wins. The paired promotion score above records it as a draw.
            for (pid, name) in seats.iter().enumerate() {
                let target = plan_target(ais[pid].as_ref());
                totals.get_mut(*name).unwrap().record(
                    &g,
                    pid,
                    g.winner == Some(pid),
                    target,
                    &traces[pid],
                );
            }
        }
        pair_scores.push(pair_score / 2.0);
        pair_terminal_scores.push(pair_terminal_score / 2.0);
    }

    println!(
        "mirrored head-to-head: {pairs} maps, {} games, {players} players, average {:.1} turns",
        2 * pairs,
        total_turns as f64 / (2 * pairs) as f64
    );
    let games = 2 * pairs;
    print!("game-win share:");
    for name in [a, b] {
        let wins = totals[name].wins;
        print!(
            " {name} {wins}/{games} ({:.1}%)",
            100.0 * wins as f64 / games as f64
        );
    }
    println!();
    let inference = paired_inference(&pair_scores);
    let outcomes = pair_outcomes(&pair_scores);
    let directions = directional_outcomes(&pair_scores);
    println!(
        "paired-map score for {a}: {:.1}% (95% Wilson CI {:.1}%..{:.1}%), Elo-equivalent {:+.0} (CI {:+.0}..{:+.0})",
        100.0 * inference.score,
        100.0 * inference.low,
        100.0 * inference.high,
        inference.elo,
        inference.elo_low,
        inference.elo_high,
    );
    println!(
        "paired outcomes: {a} sweeps {}, neutral splits/draws {}, {b} sweeps {}, draw-mixed {}",
        outcomes.a_sweeps, outcomes.neutral, outcomes.b_sweeps, outcomes.mixed_with_draw
    );
    let sign_p = exact_sign_p(directions.challenger_favored, directions.incumbent_favored);
    let directional_verdict =
        if sign_p < 0.05 && directions.challenger_favored > directions.incumbent_favored {
            format!("SIGNIFICANT {a} DIRECTION")
        } else if sign_p < 0.05 && directions.incumbent_favored > directions.challenger_favored {
            format!("SIGNIFICANT {b} DIRECTION")
        } else {
            "INCONCLUSIVE DIRECTION".to_string()
        };
    println!(
        "paired direction: {a}-favored {}, neutral {}, {b}-favored {}; exact two-sided sign p={sign_p:.4} ({directional_verdict})",
        directions.challenger_favored, directions.neutral, directions.incumbent_favored
    );
    let challenger_crossing = inference
        .anytime
        .challenger_crossed_at
        .map_or("not crossed".to_string(), |map| {
            format!("crossed at map {map}")
        });
    let incumbent_crossing = inference
        .anytime
        .incumbent_crossed_at
        .map_or("not crossed".to_string(), |map| {
            format!("crossed at map {map}")
        });
    println!(
        "anytime-valid betting evidence (2.5% per direction after {PROMOTION_MIN_MAPS} maps): {a} peak e={:.3e}, p<={:.4} ({challenger_crossing}); {b} peak e={:.3e}, p<={:.4} ({incumbent_crossing})",
        inference.anytime.challenger_peak_e,
        inference.anytime.challenger_p,
        inference.anytime.incumbent_peak_e,
        inference.anytime.incumbent_p,
    );
    match inference.verdict {
        PromotionVerdict::Insufficient => println!(
            "promotion gate: INSUFFICIENT — {} independent maps; require at least {PROMOTION_MIN_MAPS}",
            inference.maps
        ),
        PromotionVerdict::Promote => println!(
            "promotion gate: PASS — {a}'s effect interval and anytime-valid evidence both clear parity after {} maps",
            inference.maps,
        ),
        PromotionVerdict::Retain => println!(
            "promotion gate: RETAIN {b} — {b}'s effect interval and anytime-valid evidence both clear parity after {} maps",
            inference.maps,
        ),
        PromotionVerdict::Inconclusive => println!(
            "promotion gate: INCONCLUSIVE — effect size or anytime-valid evidence has not cleared parity after {} maps",
            inference.maps,
        ),
    }
    let terminal_mean = pair_terminal_scores.iter().sum::<f64>() / pairs as f64;
    let terminal_directions = directional_outcomes(&pair_terminal_scores);
    let terminal_sign_p = exact_sign_p(
        terminal_directions.challenger_favored,
        terminal_directions.incumbent_favored,
    );
    let terminal_anytime = anytime_evidence(&pair_terminal_scores);
    println!(
        "paired terminal-score diagnostic for {a}: {:.1}% (not a promotion input)",
        100.0 * terminal_mean
    );
    println!(
        "terminal-score direction: {a}-favored {}, neutral {}, {b}-favored {}; exact two-sided sign p={terminal_sign_p:.4}",
        terminal_directions.challenger_favored,
        terminal_directions.neutral,
        terminal_directions.incumbent_favored,
    );
    println!(
        "terminal-score anytime evidence (2.5% per direction after {PROMOTION_MIN_MAPS} maps): {a} peak e={:.3e}, p<={:.4}; {b} peak e={:.3e}, p<={:.4}",
        terminal_anytime.challenger_peak_e,
        terminal_anytime.challenger_p,
        terminal_anytime.incumbent_peak_e,
        terminal_anytime.incumbent_p,
    );
    println!("AI          seat-win% score cities pop tech civic dist build military gold");
    for name in [a, b] {
        let m = &totals[name];
        let n = m.games as f64;
        println!(
            "{name:<11} {:>7.1}% {:>5.1} {:>6.2} {:>3.1} {:>4.1} {:>5.1} {:>4.1} {:>5.1} {:>8.1} {:>5.1}",
            100.0 * m.wins as f64 / n,
            m.score / n,
            m.cities / n,
            m.population / n,
            m.techs / n,
            m.civics / n,
            m.districts / n,
            m.buildings / n,
            m.military / n,
            m.gold / n,
        );
    }
    println!("\nAI          faith tourists dvp envoys suzerain religious#");
    for name in [a, b] {
        let m = &totals[name];
        let n = m.games as f64;
        println!(
            "{name:<11} {:>5.1} {:>8.1} {:>3.1} {:>6.1} {:>8.2} {:>10.2}",
            m.faith / n,
            m.tourists / n,
            m.dvp / n,
            m.envoys / n,
            m.suzerainties / n,
            m.religious_units / n,
        );
    }
    println!("\nAI          mil# civ#  food prod science culture queued-cost");
    for name in [a, b] {
        let m = &totals[name];
        let n = m.games as f64;
        println!(
            "{name:<11} {:>4.1} {:>4.1} {:>5.1} {:>4.1} {:>7.1} {:>7.1} {:>11.1}",
            m.military_units / n,
            m.civilian_units / n,
            m.food_yield / n,
            m.production_yield / n,
            m.science_yield / n,
            m.culture_yield / n,
            m.queued_cost / n,
        );
    }
    println!("\nAI          settler builder trader routes/cap support missionary");
    for name in [a, b] {
        let m = &totals[name];
        let n = m.games as f64;
        println!(
            "{name:<11} {:>7.2} {:>7.2} {:>6.2} {:>5.2}/{:<4.2} {:>7.2} {:>10.2}",
            m.settlers / n,
            m.builders / n,
            m.traders / n,
            m.active_routes / n,
            m.trade_capacity / n,
            m.support_units / n,
            m.missionaries / n,
        );
    }
    println!("\nVictory types:");
    for name in [a, b] {
        println!("  {name:<11} {:?}", totals[name].victories);
    }
    println!("\nPlan commitment by observed player-turn:");
    for name in [a, b] {
        let metrics = &totals[name];
        println!(
            "  {name:<11} switches/game {:.2}; {}",
            metrics.plan_switches as f64 / metrics.games.max(1) as f64,
            target_shares(metrics)
        );
    }
    println!("\nFinal plan targets:");
    for name in [a, b] {
        println!("  {name:<11} {:?}", totals[name].final_targets);
    }
    println!("\nDominant plan targets and seat outcomes:");
    for name in [a, b] {
        println!("  {name:<11} {:?}", totals[name].dominant_targets);
        for (target, outcome) in &totals[name].target_outcomes {
            println!(
                "    {target:<11} {}/{} wins ({:.1}%)",
                outcome.wins,
                outcome.games,
                100.0 * outcome.wins as f64 / outcome.games.max(1) as f64
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_uses_mirrored_maps_as_independent_observations() {
        let one_map = paired_inference(&[1.0]);
        let two_maps = paired_inference(&[1.0, 1.0]);
        assert_eq!(one_map.maps, 1);
        assert!(one_map.low < two_maps.low);
        assert!(one_map.high <= 1.0);
        assert_eq!(one_map.verdict, PromotionVerdict::Insufficient);
    }

    #[test]
    fn strong_replicated_edge_passes_promotion_gate() {
        let scores = vec![1.0; 30];
        let result = paired_inference(&scores);
        assert!(result.low > 0.5);
        assert!(result.anytime.challenger_p <= ANYTIME_TAIL_ALPHA);
        assert_eq!(
            result.anytime.challenger_crossed_at,
            Some(PROMOTION_MIN_MAPS)
        );
        assert_eq!(result.verdict, PromotionVerdict::Promote);
    }

    #[test]
    fn minimum_map_gate_overrides_an_early_clean_sweep() {
        let result = paired_inference(&vec![1.0; PROMOTION_MIN_MAPS - 1]);
        assert!(result.low > 0.5);
        assert_eq!(result.anytime.challenger_peak_e, 1.0);
        assert_eq!(result.anytime.challenger_p, 1.0);
        assert_eq!(result.verdict, PromotionVerdict::Insufficient);
    }

    #[test]
    fn decisive_incumbent_edge_retains_it() {
        let result = paired_inference(&vec![0.0; 30]);
        assert!(result.high < 0.5);
        assert!(result.anytime.incumbent_p <= ANYTIME_TAIL_ALPHA);
        assert_eq!(result.verdict, PromotionVerdict::Retain);
    }

    #[test]
    fn balanced_maps_are_inconclusive() {
        let scores: Vec<f64> = (0..40)
            .map(|index| if index % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let result = paired_inference(&scores);
        assert!(result.low < 0.5 && result.high > 0.5);
        assert_eq!(result.anytime.challenger_p, 1.0);
        assert_eq!(result.anytime.incumbent_p, 1.0);
        assert_eq!(result.verdict, PromotionVerdict::Inconclusive);
    }

    #[test]
    fn neutral_maps_neither_spend_nor_create_betting_evidence() {
        let result = paired_inference(&vec![0.5; 100]);
        assert_eq!(result.anytime.challenger_peak_e, 1.0);
        assert_eq!(result.anytime.incumbent_peak_e, 1.0);
        assert_eq!(result.anytime.challenger_p, 1.0);
        assert_eq!(result.anytime.incumbent_p, 1.0);
        assert_eq!(result.verdict, PromotionVerdict::Inconclusive);
    }

    #[test]
    fn repeated_draw_mixed_edges_accumulate_bounded_score_evidence() {
        let result = paired_inference(&vec![0.75; 80]);
        assert!(result.anytime.challenger_p <= ANYTIME_TAIL_ALPHA);
        assert_eq!(result.anytime.incumbent_p, 1.0);
        assert_eq!(result.verdict, PromotionVerdict::Promote);
    }

    #[test]
    fn subminimum_lucky_prefix_cannot_bank_a_later_promotion() {
        let mut scores = vec![1.0; PROMOTION_MIN_MAPS / 2];
        scores.extend(vec![0.0; PROMOTION_MIN_MAPS / 2]);
        let result = paired_inference(&scores);
        assert_eq!(result.anytime.challenger_crossed_at, None);
        assert_eq!(result.anytime.challenger_p, 1.0);
        assert_eq!(result.verdict, PromotionVerdict::Inconclusive);
    }

    #[test]
    fn contradictory_anytime_crossings_flag_nonstationarity() {
        let mut scores = vec![1.0; 30];
        scores.extend(vec![0.0; 100]);
        let result = paired_inference(&scores);
        assert!(result.anytime.challenger_p <= ANYTIME_TAIL_ALPHA);
        assert!(result.anytime.incumbent_p <= ANYTIME_TAIL_ALPHA);
        assert_eq!(result.verdict, PromotionVerdict::Inconclusive);
    }

    #[test]
    fn elo_equivalent_is_symmetric_around_parity() {
        assert!((elo_edge(0.64) + elo_edge(0.36)).abs() < 1e-9);
        assert_eq!(elo_edge(0.5), 0.0);
    }

    #[test]
    fn pair_outcome_counts_keep_draw_mixed_maps_visible() {
        assert_eq!(
            pair_outcomes(&[1.0, 0.5, 0.0, 0.25, 0.75]),
            PairOutcomes {
                a_sweeps: 1,
                neutral: 1,
                b_sweeps: 1,
                mixed_with_draw: 2,
            }
        );
    }

    #[test]
    fn games_without_a_head_to_head_winner_are_draws() {
        let seats = ["challenger", "incumbent"];
        assert_eq!(game_score(Some(0), &seats, "challenger"), 1.0);
        assert_eq!(game_score(Some(1), &seats, "challenger"), 0.0);
        assert_eq!(game_score(None, &seats, "challenger"), 0.5);
        assert_eq!(game_score(Some(2), &seats, "challenger"), 0.5);
    }

    #[test]
    fn terminal_score_share_is_bounded_symmetric_and_independent_of_winner() {
        let mut game = Game::new(2, 20, 14, 71, 40, 0);
        let seats = ["challenger", "incumbent"];
        let baseline = terminal_score_share(&game, &seats, "challenger");
        assert!((baseline - 0.5).abs() < 1e-12);

        game.players[0].techs.insert("writing".to_string());
        game.winner = Some(1);
        let challenger = terminal_score_share(&game, &seats, "challenger");
        let incumbent = terminal_score_share(&game, &seats, "incumbent");
        assert!(challenger > baseline);
        assert!((challenger + incumbent - 1.0).abs() < 1e-12);
    }

    #[test]
    fn plan_trace_counts_exposure_and_switches() {
        let mut trace = PlanTrace::default();
        for target in ["adaptive", "religion", "religion", "adaptive"] {
            trace.observe(target);
        }
        assert_eq!(trace.observations, 4);
        assert_eq!(trace.switches, 2);
        assert_eq!(trace.targets["adaptive"], 2);
        assert_eq!(trace.targets["religion"], 2);
        assert_eq!(trace.dominant_target(), "adaptive");
    }

    #[test]
    fn empty_plan_trace_is_explicitly_unreported() {
        assert_eq!(PlanTrace::default().dominant_target(), "unreported");
    }

    #[test]
    fn traced_loop_preserves_headless_game_result() {
        let make_game = || Game::new(2, 16, 12, 9123, 30, 0);
        let mut plain = make_game();
        let mut traced = make_game();
        let mut plain_ais: Vec<Box<dyn Ai>> = (0..plain.players.len())
            .map(|pid| builtin_ai("basic", pid as u64 + 1))
            .collect();
        let mut traced_ais: Vec<Box<dyn Ai>> = (0..traced.players.len())
            .map(|pid| builtin_ai("basic", pid as u64 + 1))
            .collect();

        civvis::ai::run_game(&mut plain, &mut plain_ais);
        let traces = run_traced_game(&mut traced, &mut traced_ais, 2);

        assert_eq!(traced.winner, plain.winner);
        assert_eq!(traced.victory_type, plain.victory_type);
        assert_eq!(traced.turn, plain.turn);
        assert_eq!(traced.score(0), plain.score(0));
        assert_eq!(traced.score(1), plain.score(1));
        assert!(traces.iter().all(|trace| trace.observations > 0));
    }

    #[test]
    fn exact_sign_test_detects_replicated_map_direction() {
        let mut scores = vec![1.0; 8];
        scores.extend(vec![0.5; 16]);
        scores.push(0.0);
        let outcomes = directional_outcomes(&scores);
        assert_eq!(
            outcomes,
            DirectionalOutcomes {
                challenger_favored: 8,
                neutral: 16,
                incumbent_favored: 1,
            }
        );
        assert!((exact_sign_p(8, 1) - 0.039_062_5).abs() < 1e-12);
        assert_eq!(exact_sign_p(1, 8), exact_sign_p(8, 1));
    }

    #[test]
    fn sign_test_keeps_neutral_and_balanced_maps_inconclusive() {
        assert_eq!(directional_outcomes(&[0.5; 20]).neutral, 20);
        assert_eq!(exact_sign_p(0, 0), 1.0);
        assert_eq!(exact_sign_p(4, 4), 1.0);
    }

    #[test]
    fn draw_mixed_maps_still_have_a_direction() {
        assert_eq!(
            directional_outcomes(&[0.75, 0.25, 0.5]),
            DirectionalOutcomes {
                challenger_favored: 1,
                neutral: 1,
                incumbent_favored: 1,
            }
        );
    }
}
