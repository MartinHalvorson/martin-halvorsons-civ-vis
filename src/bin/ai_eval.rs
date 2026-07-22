//! Paired, seat-balanced head-to-head evaluator for built-in AIs.
use civvis::ai::{run_game, Ai};
use civvis::elo::{builtin_ai, BUILTIN_AIS};
use civvis::game::Game;
use std::collections::BTreeMap;

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
    support_units: f64,
    missionaries: f64,
    victories: BTreeMap<String, usize>,
}

impl Metrics {
    fn record(&mut self, g: &Game, pid: usize, won: bool) {
        let cities = g.player_city_ids(pid);
        self.games += 1;
        self.wins += won as usize;
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
    let mut totals: BTreeMap<String, Metrics> = [a, b]
        .into_iter()
        .map(|name| (name.to_string(), Metrics::default()))
        .collect();
    let mut total_turns = 0_u64;

    for pair in 0..pairs {
        let game_seed = seed + pair as u64;
        for swap in 0..2 {
            let seats: Vec<&str> = (0..players)
                .map(|pid| if (pid + swap) % 2 == 0 { a } else { b })
                .collect();
            let mut g = Game::new(players, width, height, game_seed, turns, city_states);
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
            let winner = g.winner.unwrap();
            for (pid, name) in seats.iter().enumerate() {
                totals
                    .get_mut(*name)
                    .unwrap()
                    .record(&g, pid, winner == pid);
            }
        }
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
    println!("\nAI          settler builder trader support missionary");
    for name in [a, b] {
        let m = &totals[name];
        let n = m.games as f64;
        println!(
            "{name:<11} {:>7.2} {:>7.2} {:>6.2} {:>7.2} {:>10.2}",
            m.settlers / n,
            m.builders / n,
            m.traders / n,
            m.support_units / n,
            m.missionaries / n,
        );
    }
    println!("\nVictory types:");
    for name in [a, b] {
        println!("  {name:<11} {:?}", totals[name].victories);
    }
}
