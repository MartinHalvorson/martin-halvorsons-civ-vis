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
//!
//! Read in Python with
//! `np.fromfile(...).reshape(meta["planes_shape"])`.
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::ai::{Ai, VictoryTarget};
use crate::elo::builtin_ai;
use crate::game::{Action, Game, GameOptions};
use crate::evolve::features as scalar_features;
use crate::obs_tensor::{obs_tensor, PLANES};

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
}

pub struct SelfPlayStats {
    pub games: usize,
    pub samples: usize,
    pub decisive: usize,
}

/// Play the configured games, exporting one sample per living major every
/// `every` turns. Returns what was written.
pub fn export(cfg: &SelfPlayCfg) -> std::io::Result<SelfPlayStats> {
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

    for game_index in 0..cfg.games {
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

        // (features, globals, pid) held until the outcome is known.
        let mut pending: Vec<(Vec<f32>, Vec<f32>, Vec<f32>, usize, f32)> = Vec::new();
        let mut last_sampled: Option<u32> = None;
        while g.winner.is_none() && g.turn <= cfg.max_turns {
            let pid = g.current;
            if g.turn % cfg.every == 0 && last_sampled != Some(g.turn) {
                last_sampled = Some(g.turn);
                let fraction = g.turn as f32 / cfg.max_turns.max(1) as f32;
                for player in 0..g.players.len() {
                    if g.players[player].is_minor
                        || g.players[player].is_barbarian
                        || !g.players[player].alive
                    {
                        continue;
                    }
                    let t = obs_tensor(&g, player);
                    if global_names.is_empty() {
                        global_names = t.global_names.clone();
                        globals_len = t.global.len();
                    }
                    pending.push((t.data, t.global, scalar_features(&g, player), player, fraction));
                }
            }
            ais[pid].take_turn(&mut g, pid);
            if g.winner.is_none() && g.current == pid {
                let _ = g.apply(pid, &Action::EndTurn);
            }
        }

        if g.winner.is_some() {
            decisive += 1;
        }
        for (planes, globals, scalars, pid, fraction) in pending {
            let won = if g.winner == Some(pid) { 1.0f32 } else { 0.0f32 };
            for value in &planes {
                planes_out.write_all(&value.to_le_bytes())?;
            }
            for value in &globals {
                globals_out.write_all(&value.to_le_bytes())?;
            }
            labels_out.write_all(&won.to_le_bytes())?;
            labels_out.write_all(&fraction.to_le_bytes())?;
            labels_out.write_all(&(game_index as f32).to_le_bytes())?;
            let row: Vec<String> = scalars.iter().map(|v| format!("{v:.4}")).collect();
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
    planes_out.flush()?;
    globals_out.flush()?;
    labels_out.flush()?;
    csv_out.flush()?;

    let meta = serde_json::json!({
        "samples": samples,
        "games": cfg.games,
        "decisive_games": decisive,
        "planes_shape": [samples, PLANES.len(), cfg.height, cfg.width],
        "globals_shape": [samples, globals_len],
        "labels_shape": [samples, 3],
        "labels": ["won", "turn_fraction", "game"],
        "plane_names": PLANES,
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
        },
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
            every: 8,
            ai: "basic".to_string(),
            out: dir.to_string_lossy().to_string(),
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
}
