//! Self-play sample export for GPU training.
//!
//! Plays full games with the built-in agents and writes fog-honest
//! `obs_tensor` snapshots labeled with each game's final outcome. This is
//! the bridge between the engine and an off-box trainer: the value net and
//! any future policy net learn from these files rather than from a
//! hand-rolled scalar CSV.
//!
//! Output (`--out <dir>`):
//! - `meta.json` — shapes, plane/global names, sample count, config
//! - `planes.f32` — `samples × planes × height × width` little-endian f32
//! - `globals.f32` — `samples × globals` little-endian f32
//! - `dataset.csv` — the 25 scalar `evolve::features`, the win label, and
//!   the source game index, the format `tools/train_valuenet.py` consumes
//!   (the trailing game column lets it hold out whole games)
//! - `labels.f32` — `samples × 3`: win label (1/0), turn fraction, and
//!   the source game index (split train/val BY GAME, never by sample:
//!   snapshots from one game are highly correlated)
//! `--scalar-only` leaves the large plane/global files empty and exports only
//! grouped scalar rows plus labels, making large value-model runs inexpensive.
//! `--counterfactual` instead exports unresolved endpoints from Strategic's
//! adaptive and victory-lane rollouts, labels each by continuing that exact
//! branch to the winner, and keeps all branches grouped by their source game.
//! It requires `--scalar-only --ai strategic_score` so data generation cannot
//! accidentally feed a learned evaluator back into its own labels.
//!
//! Read in Python with
//! `np.fromfile(...).reshape(meta["planes_shape"])`.
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::ai::{Ai, VictoryTarget};
use crate::elo::builtin_ai;
use crate::game::{Action, Game, GameOptions};
use crate::evolve::features as scalar_features;
use crate::obs_tensor::{obs_tensor, PLANES};
use crate::strategic::{StrategicAi, FIRST_REVIEW_TURN};

pub struct SelfPlayCfg {
    pub games: usize,
    pub players: usize,
    pub width: i32,
    pub height: i32,
    pub city_states: usize,
    pub max_turns: u32,
    pub seed: u64,
    /// Sample every N game turns.
    pub every: u32,
    pub ai: String,
    pub out: String,
    /// Skip expensive spatial observations when only `dataset.csv` is needed.
    pub scalar_only: bool,
    /// Export Strategic rollout endpoints rather than ordinary trajectory
    /// snapshots. Every endpoint retains its source-game group.
    pub counterfactual: bool,
    /// How many games to play at once. Samples are still written in game
    /// order; a chunk of this many games is held in memory while it plays.
    pub jobs: usize,
}

pub struct SelfPlayStats {
    pub games: usize,
    pub samples: usize,
    pub decisive: usize,
}

/// Play one game, holding every sample it produced until its winner is known.
struct PendingSample {
    planes: Vec<f32>,
    globals: Vec<f32>,
    scalars: Vec<f32>,
    pid: usize,
    fraction: f32,
    /// Counterfactual branches already have their own terminal label. Normal
    /// trajectory samples inherit the source game's winner after it finishes.
    won: Option<bool>,
    lane: Option<&'static str>,
}

type PlayedGame = (Game, Vec<PendingSample>, Vec<String>, usize, usize);

fn play_one(cfg: &SelfPlayCfg, game_index: usize) -> PlayedGame {
    let seed = cfg.seed.wrapping_add(game_index as u64);
    let mut g = Game::new_with(GameOptions::new(
        cfg.players,
        cfg.width,
        cfg.height,
        seed,
        cfg.max_turns,
        cfg.city_states,
    ));
    let mut ais: Vec<Box<dyn Ai>> = g
        .players
        .iter()
        .map(|p| builtin_ai(&cfg.ai, seed.wrapping_add(p.id as u64)))
        .collect();
    let counterfactual_sampler = cfg.counterfactual.then(|| {
        StrategicAi::score_only_with_weights(
            crate::evolve::load_champion("evolved").unwrap_or_default(),
        )
    });

    let mut pending: Vec<PendingSample> = Vec::new();
    let mut global_names: Vec<String> = Vec::new();
    let mut globals_len = 0usize;
    let mut counterfactual_roots = 0usize;
    let mut last_sampled: Option<u32> = None;
    while g.winner.is_none() && g.turn <= cfg.max_turns {
        let pid = g.current;
        let sample_due = if cfg.counterfactual {
            g.turn >= FIRST_REVIEW_TURN && (g.turn - FIRST_REVIEW_TURN).is_multiple_of(cfg.every)
        } else {
            g.turn.is_multiple_of(cfg.every)
        };
        if sample_due && last_sampled != Some(g.turn) {
            last_sampled = Some(g.turn);
            if let Some(sampler) = &counterfactual_sampler {
                let majors: Vec<usize> = g
                    .players
                    .iter()
                    .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
                    .map(|player| player.id)
                    .collect();
                if !majors.is_empty() {
                    // One root per checkpoint bounds an otherwise multiplicative
                    // number of full-game continuations. Rotate seats across
                    // checkpoints and source games without adding randomness.
                    let checkpoint = (g.turn - FIRST_REVIEW_TURN) / cfg.every;
                    let player = majors[(game_index + checkpoint as usize) % majors.len()];
                    counterfactual_roots += 1;
                    pending.extend(
                        sampler
                            .counterfactual_value_samples(&g, player)
                            .into_iter()
                            .map(|sample| PendingSample {
                                planes: Vec::new(),
                                globals: Vec::new(),
                                scalars: sample.features,
                                pid: player,
                                fraction: sample.turn_fraction,
                                won: Some(sample.won),
                                lane: Some(sample.target.map_or("adaptive", VictoryTarget::as_str)),
                            }),
                    );
                }
            } else {
                let fraction = g.turn as f32 / cfg.max_turns.max(1) as f32;
                for player in 0..g.players.len() {
                    if g.players[player].is_minor
                        || g.players[player].is_barbarian
                        || !g.players[player].alive
                    {
                        continue;
                    }
                    let (planes, globals) = if cfg.scalar_only {
                        (Vec::new(), Vec::new())
                    } else {
                        let tensor = obs_tensor(&g, player);
                        if global_names.is_empty() {
                            global_names = tensor.global_names.clone();
                            globals_len = tensor.global.len();
                        }
                        (tensor.data, tensor.global)
                    };
                    pending.push(PendingSample {
                        planes,
                        globals,
                        scalars: scalar_features(&g, player),
                        pid: player,
                        fraction,
                        won: None,
                        lane: None,
                    });
                }
            }
        }
        ais[pid].take_turn(&mut g, pid);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
    (g, pending, global_names, globals_len, counterfactual_roots)
}

/// Play the configured games, exporting ordinary living-major snapshots or
/// one rotated Strategic counterfactual root at each sampling checkpoint.
/// Returns what was written.
pub fn export(cfg: &SelfPlayCfg) -> std::io::Result<SelfPlayStats> {
    if cfg.every == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--every must be at least 1",
        ));
    }
    if cfg.counterfactual && (!cfg.scalar_only || cfg.ai != "strategic_score") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--counterfactual requires --scalar-only --ai strategic_score",
        ));
    }
    let dir = Path::new(&cfg.out);
    fs::create_dir_all(dir)?;
    let mut planes_out = BufWriter::new(fs::File::create(dir.join("planes.f32"))?);
    let mut globals_out = BufWriter::new(fs::File::create(dir.join("globals.f32"))?);
    let mut labels_out = BufWriter::new(fs::File::create(dir.join("labels.f32"))?);
    let mut csv_out = BufWriter::new(fs::File::create(dir.join("dataset.csv"))?);

    let mut samples = 0usize;
    let mut decisive = 0usize;
    let mut globals_len = 0usize;
    let mut global_names: Vec<String> = Vec::new();
    let mut counterfactual_roots = 0usize;
    let mut counterfactual_lanes = BTreeMap::<&'static str, usize>::new();

    // Games are played a chunk at a time and written out in game order, so
    // the export is byte for byte what a single-threaded run produced while
    // only one chunk's samples are ever held in memory.
    let jobs = cfg.jobs.max(1);
    let mut game_index = 0usize;
    while game_index < cfg.games {
        let chunk = jobs.min(cfg.games - game_index);
        let chunk_start = game_index;
        let played =
            crate::parallel::map(chunk, jobs, |offset| play_one(cfg, chunk_start + offset));
        for (offset, (g, pending, names, len, roots)) in played.into_iter().enumerate() {
            let game_index = chunk_start + offset;
            let seed = cfg.seed.wrapping_add(game_index as u64);
            if global_names.is_empty() {
                global_names = names;
                globals_len = len;
            }
            if g.winner.is_some() {
                decisive += 1;
            }
            counterfactual_roots += roots;
            for sample in pending {
                let won = if sample.won.unwrap_or(g.winner == Some(sample.pid)) {
                    1.0f32
                } else {
                    0.0f32
                };
                if let Some(lane) = sample.lane {
                    *counterfactual_lanes.entry(lane).or_default() += 1;
                }
                for value in &sample.planes {
                    planes_out.write_all(&value.to_le_bytes())?;
                }
                for value in &sample.globals {
                    globals_out.write_all(&value.to_le_bytes())?;
                }
                labels_out.write_all(&won.to_le_bytes())?;
                labels_out.write_all(&sample.fraction.to_le_bytes())?;
                labels_out.write_all(&(game_index as f32).to_le_bytes())?;
                let row: Vec<String> = sample.scalars.iter().map(|v| format!("{v:.4}")).collect();
                writeln!(csv_out, "{},{},{}", row.join(","), won as u8, game_index)?;
                samples += 1;
            }
            println!(
                "game {:3} seed {:<6} t{:<4} {:<10} samples={}",
                game_index,
                seed,
                g.turn,
                g.victory_type.clone().unwrap_or_else(|| "none".into()),
                samples
            );
        }
        game_index += chunk;
    }
    planes_out.flush()?;
    globals_out.flush()?;
    labels_out.flush()?;
    csv_out.flush()?;

    let plane_names: Vec<&str> = if cfg.scalar_only {
        Vec::new()
    } else {
        PLANES.to_vec()
    };
    let meta = serde_json::json!({
        "samples": samples,
        "games": cfg.games,
        "decisive_games": decisive,
        "planes_shape": [samples, plane_names.len(), cfg.height, cfg.width],
        "globals_shape": [samples, globals_len],
        "labels_shape": [samples, 3],
        "labels": ["won", "turn_fraction", "game"],
        "plane_names": plane_names,
        "global_names": global_names,
        "dtype": "<f4",
        "config": {
            "players": cfg.players,
            "width": cfg.width,
            "height": cfg.height,
            "city_states": cfg.city_states,
            "max_turns": cfg.max_turns,
            "seed": cfg.seed,
            "every": cfg.every,
            "ai": cfg.ai,
            "scalar_only": cfg.scalar_only,
            "counterfactual": cfg.counterfactual,
        },
        "counterfactual_roots": counterfactual_roots,
        "counterfactual_samples_by_lane": counterfactual_lanes,
        "victory_targets": VictoryTarget::ALL.map(|t| t.as_str()),
    });
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    Ok(SelfPlayStats {
        games: cfg.games,
        samples,
        decisive,
    })
}

#[cfg(test)]
mod tests {
    use super::{export, SelfPlayCfg};
    use crate::obs_tensor::PLANES;

    #[test]
    fn counterfactual_export_requires_score_only_scalar_controls() {
        let cfg = SelfPlayCfg {
            games: 1,
            players: 4,
            width: 20,
            height: 14,
            city_states: 0,
            max_turns: 80,
            seed: 99,
            every: 40,
            ai: "strategic".to_string(),
            out: std::env::temp_dir()
                .join("civvis_invalid_counterfactual_test")
                .to_string_lossy()
                .to_string(),
            scalar_only: true,
            counterfactual: true,
            jobs: 1,
        };
        let error = match export(&cfg) {
            Ok(_) => panic!("learned roots must fail closed"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("strategic_score"));
    }

    #[test]
    fn export_writes_readable_shapes() {
        let dir = std::env::temp_dir().join("civvis_selfplay_test");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = SelfPlayCfg {
            games: 1,
            players: 2,
            width: 20,
            height: 14,
            city_states: 0,
            max_turns: 24,
            seed: 77,
            // Sample often: a two-player duel map can be decided in a handful
            // of turns, and the shapes under test are the same either way.
            every: 2,
            ai: "basic".to_string(),
            out: dir.to_string_lossy().to_string(),
            scalar_only: false,
            counterfactual: false,
            jobs: 2,
        };
        let stats = export(&cfg).expect("export");
        assert!(stats.samples > 0);
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        let samples = meta["samples"].as_u64().unwrap() as usize;
        let globals = meta["globals_shape"][1].as_u64().unwrap() as usize;
        // File lengths must match the advertised shapes exactly, or the
        // trainer silently reshapes garbage.
        let planes_bytes = std::fs::metadata(dir.join("planes.f32")).unwrap().len() as usize;
        assert_eq!(planes_bytes, samples * PLANES.len() * 14 * 20 * 4);
        let globals_bytes = std::fs::metadata(dir.join("globals.f32")).unwrap().len() as usize;
        assert_eq!(globals_bytes, samples * globals * 4);
        let labels_bytes = std::fs::metadata(dir.join("labels.f32")).unwrap().len() as usize;
        assert_eq!(labels_bytes, samples * 3 * 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scalar_only_export_keeps_game_groups_without_spatial_payloads() {
        let dir = std::env::temp_dir().join("civvis_scalar_selfplay_test");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = SelfPlayCfg {
            games: 2,
            players: 2,
            width: 20,
            height: 14,
            city_states: 0,
            max_turns: 24,
            seed: 177,
            every: 2,
            ai: "basic".to_string(),
            out: dir.to_string_lossy().to_string(),
            scalar_only: true,
            counterfactual: false,
            jobs: 2,
        };

        let stats = export(&cfg).expect("scalar export");
        assert!(stats.samples > 0);
        assert_eq!(std::fs::metadata(dir.join("planes.f32")).unwrap().len(), 0);
        assert_eq!(std::fs::metadata(dir.join("globals.f32")).unwrap().len(), 0);
        assert_eq!(
            std::fs::metadata(dir.join("labels.f32")).unwrap().len(),
            stats.samples as u64 * 3 * 4
        );
        let rows = std::fs::read_to_string(dir.join("dataset.csv")).unwrap();
        assert_eq!(rows.lines().count(), stats.samples);
        assert!(rows.lines().all(|row| row.split(',').count() == 27));
        let groups: std::collections::BTreeSet<&str> = rows
            .lines()
            .filter_map(|row| row.rsplit(',').next())
            .collect();
        assert_eq!(groups, ["0", "1"].into_iter().collect());
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["planes_shape"][1], 0);
        assert_eq!(meta["globals_shape"][1], 0);
        assert_eq!(meta["config"]["scalar_only"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
