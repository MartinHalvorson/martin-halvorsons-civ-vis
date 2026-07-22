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
        VictoryTarget::Science => 1_300,
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
            let research_eras: Vec<usize> =
                game.players
                    .iter()
                    .filter(|player| !player.is_minor && !player.is_barbarian)
                    .map(|player| {
                        player
                            .techs
                            .iter()
                            .filter_map(|node| game.rules.techs.get(node).map(|spec| spec.era))
                            .chain(player.civics.iter().filter_map(|node| {
                                game.rules.civics.get(node).map(|spec| spec.era)
                            }))
                            .max()
                            .unwrap_or(0)
                    })
                    .collect();
            let passed = actual == target.as_str();
            failures += usize::from(!passed);
            if passed {
                winners.entry(target.as_str()).or_default().insert(winner);
            }
            let winner_state = game.players.get(winner);
            let progress = winner_state.map(|player| match target {
                VictoryTarget::Science => format!(
                    "techs={}/{} projects={} distance={:.0} science={:.1}",
                    player.techs.len(),
                    game.rules.techs.len(),
                    player.science_projects.len(),
                    player.exoplanet_distance,
                    game.player_city_ids(winner)
                        .into_iter()
                        .map(|city| game.city_yields(city).science)
                        .sum::<f64>()
                ),
                VictoryTarget::Culture => {
                    let target = game
                        .players
                        .iter()
                        .filter(|rival| {
                            rival.id != winner
                                && rival.alive
                                && !rival.is_minor
                                && !rival.is_barbarian
                        })
                        .map(|rival| game.domestic_tourists(rival.id))
                        .max()
                        .unwrap_or(0);
                    let cities = game.player_city_ids(winner);
                    let theaters = cities
                        .iter()
                        .filter(|city| {
                            game.cities[city].districts.contains_key("theater_square")
                                || game.cities[city].districts.contains_key("acropolis")
                        })
                        .count();
                    let tourist_improvements = cities
                        .iter()
                        .flat_map(|city| game.cities[city].owned_tiles.iter())
                        .filter_map(|position| game.map.tiles[position].improvement.as_deref())
                        .filter(|improvement| {
                            game.rules.improvements[*improvement]
                                .effects
                                .get("tourism")
                                .copied()
                                .unwrap_or(0.0)
                                > 0.0
                        })
                        .count();
                    format!(
                        "visiting={} target={} domestic={} tourism={:.1}/turn cities={} theaters={} tourist_tiles={} lifetime={:.0}",
                        game.foreign_tourists(winner),
                        target,
                        game.domestic_tourists(winner),
                        game.tourism_per_turn(winner),
                        cities.len(),
                        theaters,
                        tourist_improvements,
                        player.tourism_lifetime,
                    )
                }
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
                "{:<11} seed={} target={:<10} actual={:<10} winner={} turn={} world_era={} research_eras={:?} {} [{:.2}s]",
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
                game.world_era,
                research_eras,
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
