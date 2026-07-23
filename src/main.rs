//! CLI: simulate / soak / benchmark (mirrors the Python CLI outputs).
use std::collections::BTreeMap;
use std::time::Instant;

use civvis::ai::{run_game, AdvancedAi, Ai};
use civvis::game::{
    default_difficulty, default_speed, Game, GameOptions, VictoryConditions,
    DEFAULT_DISASTER_INTENSITY,
};
use civvis::rules::Rules;
use civvis::setup::{GameSpeed, MapScript, MapSize};

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

fn victory_conditions(args: &[String]) -> VictoryConditions {
    let Some(enabled) = args
        .iter()
        .position(|value| value == "--victories")
        .and_then(|index| args.get(index + 1))
    else {
        return VictoryConditions::default();
    };
    let has = |name: &str| enabled.split(',').any(|candidate| candidate == name);
    VictoryConditions {
        science: has("science"),
        culture: has("culture"),
        religious: has("religious"),
        diplomatic: has("diplomatic"),
        domination: has("domination"),
        score: has("score"),
    }
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

/// Difficulty and speed are chosen the same way everywhere: by name, against
/// the shipped ruleset, with the stock levels as defaults.
fn game_options(args: &[String], players: i64, seed: u64) -> GameOptions {
    let rules = Rules::embedded();
    let difficulty = arg_text(args, "--difficulty", &default_difficulty());
    if !rules.difficulties.contains_key(&difficulty) {
        eprintln!(
            "unknown difficulty {difficulty:?}; choose one of {:?}",
            ladder(&rules)
        );
        std::process::exit(2);
    }
    let speed = arg_text(args, "--speed", &default_speed());
    let Some(speed_spec) = rules.speeds.get(&speed) else {
        eprintln!("unknown game speed {speed:?}; choose one of {:?}", speeds(&rules));
        std::process::exit(2);
    };
    // An explicit --turns wins; otherwise every speed brings its own stock
    // budget (Standard is 500 turns / 2050 AD). Short historical defaults
    // ended games at the turn limit before the science, culture, and
    // diplomatic lanes could finish, which handed the win to whoever was
    // ahead on score at an arbitrary cutoff.
    let turns = if args.iter().any(|a| a == "--turns") {
        arg(args, "--turns", speed_spec.turns as i64)
    } else {
        speed_spec.turns as i64
    };
    let player_count = players.max(1) as usize;
    let teams_arg = arg_text(args, "--teams", "");
    let teams = if teams_arg.trim().is_empty() {
        Vec::new()
    } else {
        let parsed: Result<Vec<Option<usize>>, _> = teams_arg
            .split(',')
            .map(|team| {
                let team = team.trim();
                if team.is_empty() || team == "-" {
                    Ok(None)
                } else {
                    team.parse::<usize>().map(Some)
                }
            })
            .collect();
        let teams = parsed.unwrap_or_else(|_| {
            eprintln!("invalid --teams value {teams_arg:?}; use comma-separated team numbers or -");
            std::process::exit(2);
        });
        if teams.len() != player_count {
            eprintln!(
                "--teams needs exactly {player_count} entries (one per major player), got {}",
                teams.len()
            );
            std::process::exit(2);
        }
        teams
    };
    GameOptions {
        map_script: MapScript::from_id(&arg_text(args, "--map", "pangaea"))
            .unwrap_or(MapScript::Pangaea),
        difficulty,
        speed,
        // A headless game has nobody at the keyboard, so the difficulty only
        // reaches the AI side of the ladder unless a seat is named human.
        human_seats: arg_text(args, "--human-seats", "")
            .split(',')
            .filter_map(|seat| seat.trim().parse().ok())
            .collect(),
        teams,
        // Gathering Storm's lobby slider: 0 turns random disasters off,
        // 4 is Hyperreal. Sea-level rise follows CO2 either way.
        disaster_intensity: {
            let intensity = arg(args, "--disasters", i64::from(DEFAULT_DISASTER_INTENSITY));
            if !(0..=4).contains(&intensity) {
                eprintln!("--disasters takes 0 (none) to 4 (hyperreal), got {intensity}");
                std::process::exit(2);
            }
            intensity as u8
        },
        ..GameOptions::new(
            player_count,
            auto_dimension(args, "--width", players, true),
            auto_dimension(args, "--height", players, false),
            seed,
            turns as u32,
            auto_cs(args, players),
        )
    }
}

fn ladder(rules: &Rules) -> Vec<&str> {
    let mut names: Vec<&str> = rules.difficulties.keys().map(|k| k.as_str()).collect();
    names.sort_by_key(|name| rules.difficulties[*name].order);
    names
}

fn speeds(rules: &Rules) -> Vec<&str> {
    let mut names: Vec<&str> = rules.speeds.keys().map(|k| k.as_str()).collect();
    names.sort_by_key(|name| rules.speeds[*name].order);
    names
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
        // The army roster is the one part of an empire the score never shows,
        // and the place where a missing rule hides longest.
        let mut roster: BTreeMap<&str, usize> = BTreeMap::new();
        for unit in g.units.values() {
            if unit.owner == pid && g.rules.units[unit.kind.as_str()].class == "military" {
                *roster.entry(unit.kind.as_str()).or_default() += 1;
            }
        }
        let mut army: Vec<(&str, usize)> = roster.into_iter().collect();
        army.sort_by_key(|(kind, count)| (std::cmp::Reverse(*count), *kind));
        let army: Vec<String> = army
            .iter()
            .map(|(kind, count)| {
                let stale = if g.unit_is_obsolete(pid, kind) { "*" } else { "" };
                format!("{count}x{kind}{stale}")
            })
            .collect();
        println!(
            "  {:<10} score={:<4} cities={} pop={} techs={} {}",
            p.civ,
            g.score(pid),
            cities.len(),
            pop,
            p.techs.len(),
            if p.alive { "" } else { "(eliminated)" }
        );
        if !army.is_empty() {
            println!("             army: {}", army.join(" "));
        }
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

/// How many games to play at once. Defaults to one per core; `--jobs 1`
/// restores the strictly serial run, which is what timing one game wants.
fn jobs_arg(args: &[String]) -> usize {
    let requested = arg(args, "--jobs", 0);
    if requested > 0 {
        requested as usize
    } else {
        civvis::parallel::default_jobs()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    // Mods replace the ruleset for the whole process, so they have to be
    // installed before anything reads it.
    let mod_paths = civvis::mods::parse_arg(&arg_text(&args, "--mods", ""));
    if !mod_paths.is_empty() {
        match civvis::mods::activate(&mod_paths) {
            Ok(loaded) => {
                for info in loaded {
                    let about = if info.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", info.description)
                    };
                    println!("mod: {} ({}){about}", info.name, info.files.join(", "));
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
    match cmd {
        "simulate" => {
            let players = arg(&args, "--players", 4);
            let g0 = Instant::now();
            let mut g = Game::new_with(game_options(
                &args,
                players,
                arg(&args, "--seed", 0) as u64,
            ));
            let mut ais = AdvancedAi::fleet(&g);
            run_game(&mut g, &mut ais);
            println!("[{:.3}s]", g0.elapsed().as_secs_f64());
            standings(&g);
        }
        "soak" => {
            let players = arg(&args, "--players", 4);
            let games = arg(&args, "--games", 10);
            let start = arg(&args, "--start-seed", 0);
            let jobs = jobs_arg(&args);
            // Each game is played on its own thread, then described on the
            // main one, so a soak reads exactly as it did when it was serial.
            let lines = civvis::parallel::map(games as usize, jobs, |index| {
                let seed = start + index as i64;
                let t0 = Instant::now();
                let result = std::panic::catch_unwind(|| {
                    let mut g = Game::new_with(game_options(&args, players, seed as u64));
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
                        // An army nobody ever modernizes is invisible in the
                        // standings and obvious on the map. Count the units
                        // still fielded after their owner retired them, and
                        // the ones three eras behind the world besides.
                        let unit_era = |kind: &str| -> usize {
                            let spec = &g.rules.units[kind];
                            let tech = spec
                                .tech
                                .as_deref()
                                .and_then(|node| g.rules.techs.get(node))
                                .map(|node| node.era);
                            let civic = spec
                                .civic
                                .as_deref()
                                .and_then(|node| g.rules.civics.get(node))
                                .map(|node| node.era);
                            tech.or(civic).unwrap_or(0)
                        };
                        let (obsolete, ancient, army) = majors
                            .iter()
                            .filter(|p| p.alive)
                            .flat_map(|p| {
                                g.units.values().filter(move |unit| unit.owner == p.id)
                            })
                            .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
                            .fold((0, 0, 0), |(obsolete, ancient, army), unit| {
                                (
                                    obsolete + g.unit_is_obsolete(unit.owner, &unit.kind) as i32,
                                    ancient
                                        + (g.world_era.saturating_sub(unit_era(&unit.kind)) >= 3)
                                            as i32,
                                    army + 1,
                                )
                            });
                        flags.push_str(&format!(
                            " ARMY {army} obsolete={obsolete} ancient={ancient} era={}",
                            g.world_era
                        ));
                        Some(format!(
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
                        ))
                    }
                    Err(_) => None,
                }
            });
            let mut fails = 0;
            for (index, line) in lines.into_iter().enumerate() {
                match line {
                    Some(line) => println!("{line}"),
                    None => {
                        fails += 1;
                        println!("seed {:3}  CRASH (panic)", start + index as i64);
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
            let jobs = jobs_arg(&args);
            let t0 = Instant::now();
            let played = civvis::parallel::map(games as usize, jobs, |seed| {
                let mut g = Game::new(2, 20, 14, seed as u64, turns, 0);
                let mut ais = AdvancedAi::fleet(&g);
                run_game(&mut g, &mut ais);
                g.turn as u64
            });
            let total_turns: u64 = played.iter().sum();
            let dt = t0.elapsed().as_secs_f64();
            println!(
                "{} games, {} turns in {:.2}s = {:.0} turns/sec \
                 (2 players, 20x14, {jobs} at a time)",
                games,
                total_turns,
                dt,
                total_turns as f64 / dt
            );
        }
        // What an agent that searches actually does: take a position and roll
        // it forward, over and over. Cloning a position dominates that, and
        // nothing else here measured it.
        "rollouts" => {
            let players = arg(&args, "--players", 6);
            let warmup = arg(&args, "--turns", 150) as u32;
            let samples = arg(&args, "--samples", 5000) as usize;
            let mut g = Game::new_with(game_options(&args, players, arg(&args, "--seed", 0) as u64));
            let mut ais = AdvancedAi::fleet(&g);
            // Play in to the requested turn first: an empty map clones far
            // faster than a settled one, and a settled one is what an agent
            // searches from.
            while g.turn < warmup && g.winner.is_none() {
                let pid = g.current;
                ais[pid].take_turn(&mut g, pid);
                if g.winner.is_none() && g.current == pid {
                    let _ = g.apply(pid, &civvis::game::Action::EndTurn);
                }
            }
            let clone_start = Instant::now();
            let mut sink = 0usize;
            for _ in 0..samples {
                sink += g.clone().units.len();
            }
            let clone_us = clone_start.elapsed().as_secs_f64() / samples as f64 * 1e6;
            // A searching agent mostly applies ordinary moves and only
            // occasionally ends a turn, and the two cost wildly different
            // amounts, so both are reported.
            let seat = g.current;
            let mut mover = None;
            for action in g.legal_actions(seat) {
                if let civvis::game::Action::Move { .. } = action {
                    mover = Some(action);
                    break;
                }
            }
            let move_us = mover.as_ref().map(|action| {
                let start = Instant::now();
                for _ in 0..samples {
                    let mut branch = g.clone();
                    let _ = branch.apply(seat, action);
                    sink += branch.units.len();
                }
                start.elapsed().as_secs_f64() / samples as f64 * 1e6
            });
            let mut fast = g.clone();
            fast.set_fog_memory(false);
            let end_start = Instant::now();
            for _ in 0..samples {
                let mut branch = g.clone();
                let _ = branch.apply(seat, &civvis::game::Action::EndTurn);
                sink += branch.units.len();
            }
            let end_us = end_start.elapsed().as_secs_f64() / samples as f64 * 1e6;
            let fast_end_start = Instant::now();
            for _ in 0..samples {
                let mut branch = fast.clone();
                let _ = branch.apply(seat, &civvis::game::Action::EndTurn);
                sink += branch.units.len();
            }
            let fast_end_us = fast_end_start.elapsed().as_secs_f64() / samples as f64 * 1e6;
            // The same move on a position that is not maintaining fogged
            // memory — what a search that never observes mid-rollout pays.
            let fast_us = mover.as_ref().map(|action| {
                let start = Instant::now();
                for _ in 0..samples {
                    let mut branch = fast.clone();
                    let _ = branch.apply(seat, action);
                    sink += branch.units.len();
                }
                start.elapsed().as_secs_f64() / samples as f64 * 1e6
            });
            println!(
                "turn {} · {} seats · {} cities · {} units",
                g.turn,
                g.players.len(),
                g.cities.len(),
                g.units.len(),
            );
            println!("clone            {clone_us:8.1} us  = {:.0}/sec", 1e6 / clone_us);
            match move_us {
                Some(us) => println!("clone + move     {us:8.1} us  = {:.0} rollouts/sec", 1e6 / us),
                None => println!("clone + move          n/a  (no legal move for this seat)"),
            }
            println!("clone + end turn {end_us:8.1} us  = {:.0}/sec", 1e6 / end_us);
            if let Some(us) = fast_us {
                println!(
                    "clone + move (no fog){us:6.1} us  = {:.0} rollouts/sec",
                    1e6 / us
                );
            }
            println!("clone + end (no fog) {fast_end_us:6.1} us  = {:.0}/sec", 1e6 / fast_end_us);
            let _ = sink;
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
                jobs: jobs_arg(&args),
            };
            let pool = civvis::elo::run_tournament(&names, civvis::elo::builtin_ai, &cfg);
            println!();
            print!("{}", civvis::elo::leaderboard(&pool));
        }
        "selfplay" => {
            let players = arg(&args, "--players", 4).max(2);
            let options = game_options(&args, players, arg(&args, "--seed", 0) as u64);
            let cfg = civvis::selfplay::SelfPlayCfg {
                games: arg(&args, "--games", 20) as usize,
                players: players as usize,
                width: options.width,
                height: options.height,
                city_states: options.city_states,
                max_turns: options.max_turns,
                seed: arg(&args, "--seed", 0) as u64,
                every: arg(&args, "--every", 10).max(1) as u32,
                ai: arg_text(&args, "--ai", "advanced"),
                out: arg_text(&args, "--out", "selfplay"),
                jobs: jobs_arg(&args),
            };
            match civvis::selfplay::export(&cfg) {
                Ok(stats) => println!(
                    "
{} samples from {} games ({} decisive) -> {}",
                    stats.samples, stats.games, stats.decisive, cfg.out
                ),
                Err(error) => {
                    eprintln!("selfplay export failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "league" => {
            let players = arg(&args, "--players", 4);
            let cfg = civvis::league::LeagueCfg {
                rounds: arg(&args, "--rounds", 10) as u32,
                games_per_round: arg(&args, "--games", 16) as u32,
                players_per_game: players as usize,
                width: auto_dimension(&args, "--width", players, true),
                height: auto_dimension(&args, "--height", players, false),
                max_turns: arg(&args, "--turns", 250) as u32,
                num_city_states: auto_cs(&args, players),
                seed: arg(&args, "--seed", 1) as u64,
                jobs: jobs_arg(&args),
                dir: arg_text(&args, "--dir", "league"),
                evolve_every: arg(&args, "--evolve-every", 4) as u32,
                max_pop: arg(&args, "--pop", 12) as usize,
                verbose: !args.iter().any(|a| a == "--quiet"),
            };
            let civ = arg_text(&args, "--civ", "");
            if args.iter().any(|a| a == "--standings") || !civ.is_empty() {
                match civvis::league::load_league(&cfg.dir) {
                    Some(league) => {
                        if !civ.is_empty() {
                            print!("{}", civvis::league::civ_standings(&league, &civ));
                        } else if args.iter().any(|a| a == "--civs") {
                            print!("{}", civvis::league::civ_summary(&league));
                        } else {
                            print!("{}", civvis::league::standings(&league));
                        }
                    }
                    None => {
                        eprintln!("no league at {}/league.json", cfg.dir);
                        std::process::exit(1);
                    }
                }
            } else {
                civvis::league::run_league(&cfg);
            }
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
            let resumed: Option<Game> = args
                .iter()
                .position(|value| value == "--resume")
                .and_then(|index| args.get(index + 1))
                .map(|path| {
                    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| {
                        eprintln!("cannot read checkpoint {path}: {error}");
                        std::process::exit(2);
                    });
                    let game: Game = serde_json::from_str(&raw).unwrap_or_else(|error| {
                        eprintln!("cannot load checkpoint {path}: {error}");
                        std::process::exit(2);
                    });
                    // A save records the mods it was played under. Resuming
                    // under a different set silently changes the rules
                    // mid-game, so say so rather than pretend otherwise.
                    let active = civvis::mods::active_names();
                    if game.mods != active {
                        eprintln!(
                            "warning: {path} was played with mods {:?} but this process has {:?}",
                            game.mods, active
                        );
                    }
                    game
                });
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
            let play_options = game_options(&args, players, seed);
            let map_script = play_options.map_script;
            let game_speed = GameSpeed::from_id(&play_options.speed).unwrap_or(GameSpeed::Standard);
            civvis::server::serve_with_game(
                arg(&args, "--port", 8765) as u16,
                !args.iter().any(|a| a == "--no-open"),
                civvis::server::Params {
                    num_players: players as usize,
                    width: auto_dimension(&args, "--width", players, true),
                    height: auto_dimension(&args, "--height", players, false),
                    seed,
                    map_script,
                    game_speed,
                    max_turns: play_options.max_turns,
                    victory_conditions: victory_conditions(&args),
                    num_city_states: auto_cs(&args, players),
                    spectate: args.iter().any(|a| a == "--spectate" || a == "--watch"),
                    difficulty: play_options.difficulty,
                    speed: play_options.speed,
                    teams: play_options.teams,
                    supervised: args.iter().any(|a| a == "--supervised"),
                    league_dir: {
                        let dir = arg_text(&args, "--league", "");
                        (!dir.is_empty()).then_some(dir)
                    },
                    league_record: args.iter().any(|a| a == "--league-record"),
                },
                resumed,
            );
        }
        "pedia" => {
            // Everything after the command that is not a flag is the query.
            let query = args
                .iter()
                .skip(1)
                .take_while(|arg| !arg.starts_with("--"))
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            let rules = Rules::embedded();
            let found = civvis::pedia::search(&rules, &query);
            if found.is_empty() {
                println!("nothing in the ruleset matches {query:?}");
                std::process::exit(1);
            }
            print!("{}", civvis::pedia::render(&found));
            println!("
{} entries", found.len());
        }
        "validate" => {
            let findings = civvis::validate::validate(&Rules::embedded());
            let (text, clean) = civvis::validate::report(&findings);
            print!("{text}");
            let strict = args.iter().any(|a| a == "--strict");
            if !clean || (strict && !findings.is_empty()) {
                std::process::exit(1);
            }
        }
        _ => {
            println!(
                "usage: civvis <simulate|soak|benchmark|tournament|league|play|evolve|validate|pedia> \
                      [--players N] [--seed N] [--turns N] [--width N] [--height N] \
                      [--city-states N] [--games N] [--ais a,b] [--port N] [--no-open] \
                      [--map pangaea|continents|small_continents|inland_sea] \
                      [--difficulty settler|chieftain|warlord|prince|king|emperor|immortal|deity] \
                      [--speed online|quick|standard|epic|marathon] \
                      [--disasters 0|1|2|3|4] \
                      [--human-seats 0,1] [--teams 0,0,1,1] [--mods path/to/mod,path/to/other] \
                      [--victories science,culture,religious,diplomatic,domination,score] \
                      [--spectate] [--supervised] [--resume checkpoint.json] [--strict] \
                      [--league dir] [--league-record] [--standings [--civ Rome | --civs]] [--rounds N] \
                      [--evolve-every N] [--pop N]"
            );
        }
    }
}
