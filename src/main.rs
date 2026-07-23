//! CLI: simulate / soak / benchmark (mirrors the Python CLI outputs).
use std::collections::BTreeMap;
use std::time::Instant;

use civvis::ai::{run_game, AdvancedAi};
use civvis::game::{
    default_difficulty, default_speed, Game, GameOptions, TournamentPreset, VictoryConditions,
};
use civvis::rules::Rules;
use civvis::setup::{GameProfile, GameSpeed, MapScript, MapSize};

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

fn game_profile(args: &[String]) -> GameProfile {
    let id = arg_text(args, "--game-profile", GameProfile::Civ6Tournament.id());
    GameProfile::from_id(&id).unwrap_or_else(|| {
        eprintln!("unknown game profile {id:?}; choose civ6-tournament or civ65");
        std::process::exit(2);
    })
}

fn tournament_preset(args: &[String], profile: GameProfile) -> Option<TournamentPreset> {
    let id = args
        .iter()
        .position(|arg| arg == "--tournament-preset")
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| {
            if profile.is_tournament() {
                TournamentPreset::CplFfa202607.id().to_string()
            } else {
                String::new()
            }
        });
    if id.is_empty() || id == "none" {
        return None;
    }
    TournamentPreset::from_id(&id).or_else(|| {
        eprintln!(
            "unknown tournament preset {id:?}; choose cpl-ffa-2026-07 or cpl-teamers-2026-07"
        );
        std::process::exit(2);
    })
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
    let game_profile = game_profile(args);
    let tournament_preset = tournament_preset(args, game_profile);
    let difficulty = arg_text(args, "--difficulty", &default_difficulty());
    if !rules.difficulties.contains_key(&difficulty) {
        eprintln!(
            "unknown difficulty {difficulty:?}; choose one of {:?}",
            ladder(&rules)
        );
        std::process::exit(2);
    }
    let default_game_speed = if tournament_preset.is_some() {
        "online".to_string()
    } else {
        default_speed()
    };
    let speed = arg_text(args, "--speed", &default_game_speed);
    let Some(speed_spec) = rules.speeds.get(&speed) else {
        eprintln!(
            "unknown game speed {speed:?}; choose one of {:?}",
            speeds(&rules)
        );
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
    let disaster_intensity = arg(args, "--disaster-intensity", 2);
    if !(0..=4).contains(&disaster_intensity) {
        eprintln!("--disaster-intensity must be between 0 (Minimal) and 4 (Hyperreal)");
        std::process::exit(2);
    }
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
    if tournament_preset.is_some_and(TournamentPreset::is_teamers) && teams.is_empty() {
        eprintln!("the CPL teamers preset requires explicit --teams assignments");
        std::process::exit(2);
    }
    GameOptions {
        map_script: MapScript::from_id(&arg_text(args, "--map", "pangaea"))
            .unwrap_or(MapScript::Pangaea),
        difficulty,
        speed,
        game_profile,
        // A headless game has nobody at the keyboard, so the difficulty only
        // reaches the AI side of the ladder unless a seat is named human.
        human_seats: arg_text(args, "--human-seats", "")
            .split(',')
            .filter_map(|seat| seat.trim().parse().ok())
            .collect(),
        disaster_intensity: disaster_intensity as u8,
        teams,
        tournament_preset,
        barbarians: !tournament_preset.is_some_and(TournamentPreset::is_teamers),
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
    let winners = g.winning_players();
    let winner_name = if winners.len() > 1 {
        format!(
            "Team {}",
            winners
                .iter()
                .map(|winner| g.players[*winner].civ.as_str())
                .collect::<Vec<_>>()
                .join(" + ")
        )
    } else {
        w.civ.clone()
    };
    println!(
        "Winner: {} (players {}) by {} on turn {}",
        winner_name,
        winners
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(","),
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
                let stale = if g.unit_is_obsolete(pid, kind) {
                    "*"
                } else {
                    ""
                };
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
            let mut g =
                Game::new_with(game_options(&args, players, arg(&args, "--seed", 0) as u64));
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
                            .flat_map(|p| g.units.values().filter(move |unit| unit.owner == p.id))
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
            let game_profile = game_profile(&args);
            let tournament_preset = tournament_preset(&args, game_profile);
            if tournament_preset.is_some_and(TournamentPreset::is_teamers) {
                eprintln!(
                    "the Elo tournament command ranks individual entrants; use play or simulate for CPL teamers"
                );
                std::process::exit(2);
            }
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
                max_turns: arg(
                    &args,
                    "--turns",
                    if tournament_preset.is_some() {
                        250
                    } else {
                        150
                    },
                ) as u32,
                num_city_states: auto_cs(&args, arg(&args, "--players", 4)),
                seed: arg(&args, "--seed", 0) as u64,
                k: arg(&args, "--k", 24) as f64,
                verbose: !args.iter().any(|a| a == "--quiet"),
                game_profile,
                tournament_preset,
            };
            let ratings_path = arg_text(&args, "--ratings", civvis::elo::DEFAULT_RATINGS_PATH);
            match civvis::elo::run_persistent_tournament(
                &names,
                civvis::elo::builtin_ai,
                &cfg,
                &ratings_path,
            ) {
                Ok(pool) => {
                    println!();
                    print!("{}", civvis::elo::leaderboard(&pool));
                    println!("ratings checkpointed to {ratings_path}");
                }
                Err(error) => {
                    eprintln!("Elo tournament failed: {error}");
                    std::process::exit(1);
                }
            }
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
                options: Some(options),
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
                    game_profile: play_options.game_profile,
                    map_script,
                    game_speed,
                    max_turns: play_options.max_turns,
                    victory_conditions: victory_conditions(&args),
                    disaster_intensity: play_options.disaster_intensity,
                    num_city_states: auto_cs(&args, players),
                    spectate: args.iter().any(|a| a == "--spectate" || a == "--watch"),
                    difficulty: play_options.difficulty,
                    speed: play_options.speed,
                    teams: play_options.teams,
                    tournament_preset: play_options.tournament_preset,
                    supervised: args.iter().any(|a| a == "--supervised"),
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
            println!(
                "
{} entries",
                found.len()
            );
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
                "usage: civvis <simulate|soak|benchmark|tournament|play|evolve|validate|pedia> \
                      [--players N] [--seed N] [--turns N] [--width N] [--height N] \
                      [--city-states N] [--games N] [--ais a,b] [--ratings path] [--port N] [--no-open] \
                      [--map pangaea|continents|small_continents|inland_sea] \
                      [--game-profile civ6-tournament|civ65] \
                      [--difficulty settler|chieftain|warlord|prince|king|emperor|immortal|deity] \
                      [--speed online|quick|standard|epic|marathon] \
                      [--disaster-intensity 0|1|2|3|4] \
                      [--human-seats 0,1] [--teams 0,0,1,1] [--mods path/to/mod,path/to/other] \
                      [--tournament-preset cpl-ffa-2026-07|cpl-teamers-2026-07] \
                      [--victories science,culture,religious,diplomatic,domination,score] \
                      [--spectate] [--supervised] [--resume checkpoint.json] [--strict]"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{game_profile, tournament_preset};
    use civvis::game::TournamentPreset;
    use civvis::setup::GameProfile;

    #[test]
    fn cli_defaults_to_the_tournament_profile_and_dated_policy() {
        let args = vec!["play".to_string()];
        let profile = game_profile(&args);
        assert_eq!(profile, GameProfile::Civ6Tournament);
        assert_eq!(
            tournament_preset(&args, profile),
            Some(TournamentPreset::CplFfa202607)
        );
    }

    #[test]
    fn cli_civ65_profile_drops_the_tournament_policy() {
        let args = vec![
            "play".to_string(),
            "--game-profile".to_string(),
            "civ65".to_string(),
        ];
        let profile = game_profile(&args);
        assert_eq!(profile, GameProfile::Civ65);
        assert_eq!(tournament_preset(&args, profile), None);
    }
}
