//! End-to-end victory-condition evaluator.
//!
//! Every major in each game is given the same explicit victory target. A run
//! only passes when the real game loop ends with that victory type; no state
//! is injected and no victory check is called directly.
use civvis::ai::{run_game, AdvancedAi, VictoryTarget};
use civvis::game::Game;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

fn number(args: &[String], flag: &str, default: usize) -> usize {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn selected_targets(args: &[String]) -> Result<Vec<VictoryTarget>, String> {
    let Some(index) = args.iter().position(|arg| arg == "--target") else {
        return Ok(VictoryTarget::ALL.to_vec());
    };
    let raw = args
        .get(index + 1)
        .ok_or_else(|| "--target requires a value".to_string())?;
    if raw == "all" {
        return Ok(VictoryTarget::ALL.to_vec());
    }
    raw.split(',').map(str::parse).collect()
}

fn default_turn_limit(target: VictoryTarget) -> u32 {
    match target {
        VictoryTarget::Religion => 450,
        VictoryTarget::Domination => 650,
        VictoryTarget::Diplomacy => 750,
        VictoryTarget::Culture => 900,
        VictoryTarget::Science => 1_200,
        VictoryTarget::Score => 300,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let targets = selected_targets(&args).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let games = number(&args, "--games", 3);
    let start_seed = number(&args, "--start-seed", 9_000) as u64;
    let players = number(&args, "--players", 2).clamp(2, 8);
    let width = number(&args, "--width", 24).max(16) as i32;
    let height = number(&args, "--height", 16).max(12) as i32;
    let override_turns = args
        .iter()
        .position(|arg| arg == "--turns")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse::<u32>().ok());
    let mut failures = 0;
    let mut winners: BTreeMap<&'static str, BTreeSet<usize>> = BTreeMap::new();
    let started = Instant::now();

    for target in targets.iter().copied() {
        for game_index in 0..games {
            let seed = start_seed + game_index as u64;
            let city_states = if target == VictoryTarget::Diplomacy {
                (players + 1).max(3)
            } else {
                0
            };
            let turns = override_turns.unwrap_or_else(|| default_turn_limit(target));
            let game_started = Instant::now();
            let mut game = Game::new_full(players, width, height, seed, turns, city_states, false);
            let mut ais = AdvancedAi::fleet_targeting(&game, target);
            run_game(&mut game, &mut ais);

            let actual = game.victory_type.as_deref().unwrap_or("none");
            let winner = game.winner.unwrap_or(usize::MAX);
            let passed = actual == target.as_str();
            failures += usize::from(!passed);
            if passed {
                winners.entry(target.as_str()).or_default().insert(winner);
            }
            let winner_state = game.players.get(winner);
            let progress = winner_state.map(|player| match target {
                VictoryTarget::Science => format!(
                    "projects={} distance={:.0}",
                    player.science_projects.len(),
                    player.exoplanet_distance
                ),
                VictoryTarget::Culture => format!(
                    "visiting={} domestic={}",
                    game.foreign_tourists(winner),
                    game.domestic_tourists(winner)
                ),
                VictoryTarget::Religion => {
                    format!("religion={}", player.religion.as_deref().unwrap_or("none"))
                }
                VictoryTarget::Diplomacy => format!("dvp={}", player.dvp),
                VictoryTarget::Domination => {
                    format!("cities={}", game.player_city_ids(winner).len())
                }
                VictoryTarget::Score => format!("score={}", game.score(winner)),
            });
            println!(
                "{:<11} seed={} target={:<10} actual={:<10} winner={} turn={} {} [{:.2}s]",
                if passed { "PASS" } else { "FAIL" },
                seed,
                target.as_str(),
                actual,
                if winner == usize::MAX {
                    "none".to_string()
                } else {
                    winner.to_string()
                },
                game.turn,
                progress.unwrap_or_default(),
                game_started.elapsed().as_secs_f64(),
            );
        }
    }

    println!("\nseat winners by target:");
    for target in &targets {
        println!(
            "  {:<10} {:?}",
            target.as_str(),
            winners.get(target.as_str())
        );
    }
    println!(
        "{} games, {} failures in {:.2}s",
        targets.len() * games,
        failures,
        started.elapsed().as_secs_f64()
    );
    if failures > 0 {
        std::process::exit(1);
    }
}
