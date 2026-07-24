//! Paired, seat-balanced head-to-head evaluator for built-in AIs.
use civvis::ai::{run_game, Ai};
use civvis::elo::{builtin_ai, BUILTIN_AIS};
use civvis::game::{default_difficulty, Game, GameOptions};
use civvis::rules::Rules;
use std::collections::{BTreeMap, BTreeSet};

const PROMOTION_MIN_MAPS: usize = 20;
const Z_95: f64 = 1.959_963_984_540_054;

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
    verdict: PromotionVerdict,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PairOutcomes {
    a_sweeps: usize,
    neutral: usize,
    b_sweeps: usize,
    mixed_with_draw: usize,
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

/// A conservative Wilson score interval with one observation per mirrored map.
///
/// Pair scores can be fractional because a split scores 0.5 and a game without
/// a winner is a draw. Treating each bounded map score as one Bernoulli-equivalent
/// observation uses the maximum variance for that mean, so the swapped games are
/// never falsely counted as independent evidence.
fn paired_inference(scores: &[f64]) -> PairedInference {
    let maps = scores.len();
    if maps == 0 {
        return PairedInference {
            maps,
            score: 0.5,
            low: 0.0,
            high: 1.0,
            elo: 0.0,
            elo_low: elo_edge(0.0),
            elo_high: elo_edge(1.0),
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
    let verdict = if maps < PROMOTION_MIN_MAPS {
        PromotionVerdict::Insufficient
    } else if low > 0.5 {
        PromotionVerdict::Promote
    } else if high < 0.5 {
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
    targets: BTreeMap<String, usize>,
}

impl Metrics {
    fn record(&mut self, g: &Game, pid: usize, won: bool, target: Option<&str>) {
        let cities = g.player_city_ids(pid);
        self.games += 1;
        self.wins += won as usize;
        *self
            .targets
            .entry(target.unwrap_or("adaptive").to_string())
            .or_default() += 1;
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
            BUILTIN_AIS.contains(&name),
            "unknown AI {name:?}: {BUILTIN_AIS:?}"
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

    for pair in 0..pairs {
        let game_seed = seed + pair as u64;
        let mut pair_score = 0.0;
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
            run_game(&mut g, &mut ais);
            total_turns += g.turn as u64;
            pair_score += game_score(g.winner, &seats, a);
            // Legacy per-seat win metrics count a game nobody won as zero
            // wins. The paired promotion score above records it as a draw.
            for (pid, name) in seats.iter().enumerate() {
                let target = ais[pid].plan_report().and_then(|plan| plan.victory_target);
                totals
                    .get_mut(*name)
                    .unwrap()
                    .record(&g, pid, g.winner == Some(pid), target);
            }
        }
        pair_scores.push(pair_score / 2.0);
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
    match inference.verdict {
        PromotionVerdict::Insufficient => println!(
            "promotion gate: INSUFFICIENT — {} independent maps; require at least {PROMOTION_MIN_MAPS}",
            inference.maps
        ),
        PromotionVerdict::Promote => println!(
            "promotion gate: PASS — {a}'s 95% lower bound is above parity after {} maps",
            inference.maps
        ),
        PromotionVerdict::Retain => println!(
            "promotion gate: RETAIN {b} — {a}'s 95% upper bound is below parity after {} maps",
            inference.maps
        ),
        PromotionVerdict::Inconclusive => println!(
            "promotion gate: INCONCLUSIVE — the 95% interval overlaps parity after {} maps",
            inference.maps
        ),
    }
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
    println!("\nFinal explicit targets:");
    for name in [a, b] {
        println!("  {name:<11} {:?}", totals[name].targets);
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
        assert_eq!(result.verdict, PromotionVerdict::Promote);
    }

    #[test]
    fn minimum_map_gate_overrides_an_early_clean_sweep() {
        let result = paired_inference(&vec![1.0; PROMOTION_MIN_MAPS - 1]);
        assert!(result.low > 0.5);
        assert_eq!(result.verdict, PromotionVerdict::Insufficient);
    }

    #[test]
    fn decisive_incumbent_edge_retains_it() {
        let result = paired_inference(&vec![0.0; 30]);
        assert!(result.high < 0.5);
        assert_eq!(result.verdict, PromotionVerdict::Retain);
    }

    #[test]
    fn balanced_maps_are_inconclusive() {
        let scores: Vec<f64> = (0..40)
            .map(|index| if index % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let result = paired_inference(&scores);
        assert!(result.low < 0.5 && result.high > 0.5);
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
}
