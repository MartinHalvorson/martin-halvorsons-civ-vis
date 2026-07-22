//! CLI: simulate / soak / benchmark (mirrors the Python CLI outputs).
use std::time::Instant;

use civvis::ai::{run_game, AdvancedAi};
use civvis::game::Game;
use civvis::setup::MapSize;

fn arg(args: &[String], key: &str, default: i64) -> i64 {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn arg_text(args: &[String], key: &str, default: &str) -> String {
    args.iter()
        .position(|arg| arg == key)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn auto_cs(args: &[String], players: i64) -> usize {
    let cs = arg(args, "--city-states", -1);
    if cs >= 0 {
        cs as usize
    } else {
        MapSize::for_players(players.max(1) as usize).default_city_states
    }
}

fn auto_dimension(args: &[String], key: &str, players: i64, width: bool) -> i32 {
    let size = MapSize::for_players(players.max(1) as usize);
    arg(
        args,
        key,
        if width { size.width } else { size.height } as i64,
    ) as i32
}

fn standings(g: &Game) {
    let w = &g.players[g.winner.unwrap()];
    println!(
        "Winner: {} (player {}) by {} on turn {}",
        w.civ,
        w.id,
        g.victory_type.clone().unwrap_or_default(),
        g.turn
    );
    let mut majors: Vec<usize> = g
        .players
        .iter()
        .filter(|p| !p.is_minor)
        .map(|p| p.id)
        .collect();
    majors.sort_by_key(|pid| -g.score(*pid));
    for pid in majors {
        let p = &g.players[pid];
        let cities = g.player_city_ids(pid);
        let pop: i32 = cities.iter().map(|c| g.cities[c].pop).sum();
        println!(
            "  {:<10} score={:<4} cities={} pop={} techs={} {}",
            p.civ,
            g.score(pid),
            cities.len(),
            pop,
            p.techs.len(),
            if p.alive { "" } else { "(eliminated)" }
        );
    }
    let minors: Vec<&str> = g
        .players
        .iter()
        .filter(|p| p.is_minor && !p.is_barbarian)
        .map(|p| p.civ.as_str())
        .collect();
    if !minors.is_empty() {
        println!("  City-states: {}", minors.join(", "));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    match cmd {
        "simulate" => {
            let players = arg(&args, "--players", 4);
            let g0 = Instant::now();
            let mut g = Game::new(
                players as usize,
                auto_dimension(&args, "--width", players, true),
                auto_dimension(&args, "--height", players, false),
                arg(&args, "--seed", 0) as u64,
                arg(&args, "--turns", 250) as u32,
                auto_cs(&args, players),
            );
            let mut ais = AdvancedAi::fleet(&g);
            run_game(&mut g, &mut ais);
            println!("[{:.3}s]", g0.elapsed().as_secs_f64());
            standings(&g);
        }
        "soak" => {
            let players = arg(&args, "--players", 4);
            let games = arg(&args, "--games", 10);
            let start = arg(&args, "--start-seed", 0);
            let mut fails = 0;
            for seed in start..start + games {
                let t0 = Instant::now();
                let result = std::panic::catch_unwind(|| {
                    let mut g = Game::new(
                        players as usize,
                        auto_dimension(&args, "--width", players, true),
                        auto_dimension(&args, "--height", players, false),
                        seed as u64,
                        arg(&args, "--turns", 120) as u32,
                        auto_cs(&args, players),
                    );
                    let mut ais = AdvancedAi::fleet(&g);
                    run_game(&mut g, &mut ais);
                    g
                });
                match result {
                    Ok(g) => {
                        let majors: Vec<_> = g.players.iter().filter(|p| !p.is_minor).collect();
                        let minors: Vec<_> = g
                            .players
                            .iter()
                            .filter(|p| p.is_minor && !p.is_barbarian)
                            .collect();
                        let w = &g.players[g.winner.unwrap()];
                        let mut flags = String::new();
                        if majors.iter().all(|p| p.techs.len() <= 2) {
                            flags.push_str(" NO-TECH-PROGRESS");
                        }
                        if w.is_minor {
                            flags.push_str(" MINOR-WINNER");
                        }
                        println!(
                            "seed {:3}  t{:<4} {:<10} {:<8} majors_alive={}/{} cities={:<2} cs_alive={}/{} [{:.2}s]{}",
                            seed,
                            g.turn,
                            g.victory_type.clone().unwrap_or_default(),
                            w.civ,
                            majors.iter().filter(|p| p.alive).count(),
                            majors.len(),
                            g.cities.len(),
                            minors.iter().filter(|p| p.alive).count(),
                            minors.len(),
                            t0.elapsed().as_secs_f64(),
                            flags
                        );
                    }
                    Err(_) => {
                        fails += 1;
                        println!("seed {seed:3}  CRASH (panic)");
                    }
                }
            }
            println!("\n{}/{} games completed", games - fails, games);
            if fails > 0 {
                std::process::exit(1);
            }
        }
        "benchmark" => {
            let games = arg(&args, "--games", 50);
            let turns = arg(&args, "--turns", 100) as u32;
            let t0 = Instant::now();
            let mut total_turns: u64 = 0;
            for seed in 0..games {
                let mut g = Game::new(2, 20, 14, seed as u64, turns, 0);
                let mut ais = AdvancedAi::fleet(&g);
                run_game(&mut g, &mut ais);
                total_turns += g.turn as u64;
            }
            let dt = t0.elapsed().as_secs_f64();
            println!(
                "{} games, {} turns in {:.2}s = {:.0} turns/sec (2 players, 20x14)",
                games,
                total_turns,
                dt,
                total_turns as f64 / dt
            );
        }
        "tournament" => {
            let names: Vec<String> = args
                .iter()
                .position(|a| a == "--ais")
                .and_then(|i| args.get(i + 1))
                .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
                .unwrap_or_else(|| vec!["advanced".to_string(), "basic".to_string()]);
            for n in &names {
                if !civvis::elo::BUILTIN_AIS.contains(&n.as_str()) {
                    eprintln!(
                        "unknown AI {n:?}; builtin: {:?} (custom bots: \
                              use civvis::elo::run_tournament from Rust)",
                        civvis::elo::BUILTIN_AIS
                    );
                    std::process::exit(1);
                }
            }
            let cfg = civvis::elo::TourneyCfg {
                games: arg(&args, "--games", 20) as u32,
                players_per_game: arg(&args, "--players", 4) as usize,
                width: auto_dimension(&args, "--width", arg(&args, "--players", 4), true),
                height: auto_dimension(&args, "--height", arg(&args, "--players", 4), false),
                max_turns: arg(&args, "--turns", 150) as u32,
                num_city_states: auto_cs(&args, arg(&args, "--players", 4)),
                seed: arg(&args, "--seed", 0) as u64,
                k: arg(&args, "--k", 24) as f64,
                verbose: !args.iter().any(|a| a == "--quiet"),
            };
            let pool = civvis::elo::run_tournament(&names, civvis::elo::builtin_ai, &cfg);
            println!();
            print!("{}", civvis::elo::leaderboard(&pool));
        }
        "evolve" => {
            let players = arg(&args, "--players", 4);
            civvis::evolve::evolve(&civvis::evolve::EvoCfg {
                generations: arg(&args, "--generations", 1_000_000) as u32,
                pop: arg(&args, "--pop", 16) as usize,
                games: arg(&args, "--games", 8) as usize,
                players: players as usize,
                width: auto_dimension(&args, "--width", players, true),
                height: auto_dimension(&args, "--height", players, false),
                max_turns: arg(&args, "--turns", 160) as u32,
                seed: arg(&args, "--seed", 1) as u64,
                threads: arg(&args, "--threads", 8) as usize,
                dir: arg_text(&args, "--dir", "evolved"),
            });
        }
        "play" => {
            let players = arg(&args, "--players", 4);
            let seed = {
                let s = arg(&args, "--seed", -1);
                if s >= 0 {
                    s as u64
                } else {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .subsec_nanos() as u64
                }
            };
            civvis::server::serve(
                arg(&args, "--port", 8765) as u16,
                !args.iter().any(|a| a == "--no-open"),
                civvis::server::Params {
                    num_players: players as usize,
                    width: auto_dimension(&args, "--width", players, true),
                    height: auto_dimension(&args, "--height", players, false),
                    seed,
                    max_turns: arg(&args, "--turns", 500) as u32,
                    num_city_states: auto_cs(&args, players),
                    spectate: args.iter().any(|a| a == "--spectate" || a == "--watch"),
                },
            );
        }
        _ => {
            println!(
                "usage: civvis <simulate|soak|benchmark|tournament|play|evolve> \
                      [--players N] [--seed N] [--turns N] [--width N] [--height N] \
                      [--city-states N] [--games N] [--ais a,b] [--port N] [--no-open] \
                      [--spectate]"
            );
        }
    }
}
