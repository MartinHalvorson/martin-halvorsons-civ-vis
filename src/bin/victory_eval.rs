//! Full-length multiplayer diagnostics for AI victory pursuit.
use std::collections::BTreeMap;

use civvis::ai::{run_game, AdvancedAi};
use civvis::game::Game;
use civvis::setup::MapSize;

fn number(args: &[String], flag: &str, default: i64) -> i64 {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let games = number(&args, "--games", 20).max(1) as usize;
    let players = number(&args, "--players", 4).max(2) as usize;
    let turns = number(&args, "--turns", 500).max(1) as u32;
    let first_seed = number(&args, "--seed", 0).max(0) as u64;
    let size = MapSize::for_players(players);
    let mut victories: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_turns = 0_u64;

    for offset in 0..games {
        let seed = first_seed + offset as u64;
        let mut game = Game::new_full(
            players,
            size.width,
            size.height,
            seed,
            turns,
            size.default_city_states,
            false,
        );
        let mut ais = AdvancedAi::fleet(&game);
        run_game(&mut game, &mut ais);
        total_turns += game.turn as u64;
        let victory = game.victory_type.clone().unwrap_or_else(|| "none".to_string());
        *victories.entry(victory.clone()).or_default() += 1;

        let majors: Vec<_> = game.players.iter()
            .filter(|player| !player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .collect();
        let top_dvp = majors.iter().map(|pid| game.players[*pid].dvp).max().unwrap_or(0);
        let top_techs = majors.iter().map(|pid| game.players[*pid].techs.len()).max().unwrap_or(0);
        let top_projects = majors.iter()
            .map(|pid| game.players[*pid].science_projects.len()).max().unwrap_or(0);
        let top_distance = majors.iter().map(|pid| game.players[*pid].exoplanet_distance)
            .fold(0.0_f64, f64::max);
        let best_culture_margin = majors.iter().map(|pid| {
            let target = majors.iter().filter(|other| *other != pid)
                .map(|other| game.domestic_tourists(*other)).max().unwrap_or(0);
            game.foreign_tourists(*pid) - target
        }).max().unwrap_or(0);
        let top_capitals = majors.iter().map(|pid| game.cities.values()
            .filter(|city| city.is_capital && city.owner == *pid)
            .count()).max().unwrap_or(0);
        let top_religious_civs = majors.iter().filter_map(|pid| {
            let religion = game.players[*pid].religion.as_deref()?;
            Some(majors.iter().filter(|other| {
                let cities: Vec<_> = game.cities.values()
                    .filter(|city| city.owner == **other).collect();
                let followers = cities.iter()
                    .filter(|city| game.city_religion(city) == Some(religion)).count();
                !cities.is_empty() && followers * 2 > cities.len()
            }).count())
        }).max().unwrap_or(0);

        println!(
            "seed {seed:<5} t{:<3} {:<10} {:<9} tech {:>2}/33 proj {top_projects} dist {:>2.0}/50 dvp {:>2}/20 culture {:+} caps {}/{} religion {}/{}",
            game.turn,
            victory,
            game.players[game.winner.unwrap()].civ,
            top_techs,
            top_distance,
            top_dvp,
            best_culture_margin,
            top_capitals,
            majors.len(),
            top_religious_civs,
            majors.len(),
        );
    }

    println!("\nVictory mix ({games} games, average {:.1} turns):", total_turns as f64 / games as f64);
    for (victory, count) in victories {
        println!("  {victory:<10} {count:>3}  ({:>5.1}%)", 100.0 * count as f64 / games as f64);
    }
}
