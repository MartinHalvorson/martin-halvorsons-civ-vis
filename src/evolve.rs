//! Genetic-algorithm search over AdvancedAi strategy and combat-doctrine weights.
//! `civvis evolve` runs generations forever (checkpointed every generation):
//! each genome plays vs the reigning champion on shared maps; the champion is
//! replaced when a genome clearly outperforms champion-level opposition.
//! Artifacts in evolved/: best.json (validated champion), archive.json
//! (opponent hall of fame), population.json (resume state), history.csv
//! (training and fixed-seed holdout fitness per generation).
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ai::{run_game, AdvancedAi, Ai, Weights};
use crate::game::{Action, Game};
use crate::rng::Rng;
use crate::setup::MapSize;

pub struct EvoCfg {
    pub generations: u32,
    pub pop: usize,
    pub games: usize, // games per genome per generation
    pub players: usize,
    pub width: i32,
    pub height: i32,
    pub max_turns: u32,
    pub seed: u64,
    pub threads: usize,
    pub dir: String,
}

#[derive(Serialize, Deserialize)]
pub struct Champion {
    pub gen: u32,
    pub fitness: f64,
    #[serde(default)]
    pub validation_score: f64,
    #[serde(default)]
    pub validation_games: usize,
    pub weights: Weights,
}

#[derive(Serialize, Deserialize)]
struct PopState {
    gen: u32,
    genomes: Vec<Weights>,
}

#[derive(Default, Serialize, Deserialize)]
struct ArchiveState {
    champions: Vec<Weights>,
}

pub fn load_champion(dir: &str) -> Option<Weights> {
    load_champion_record(Path::new(dir)).map(|champion| champion.weights)
}

fn load_champion_record(dir: &Path) -> Option<Champion> {
    let raw = fs::read_to_string(dir.join("best.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Fitness of one game: score share + combat-achievement share + outright win.
/// The smaller combat term gives coordinated tactics a learning signal before
/// they decide a whole match; champion promotion still depends only on wins.
fn eval_game(
    w: &Weights,
    opponents: &[Weights],
    seat: usize,
    cfg: &EvoCfg,
    seed: u64,
    long: bool,
) -> (f64, bool) {
    // mix game lengths so champions aren't tuned only for short score races
    let turns = if long {
        cfg.max_turns * 2
    } else {
        cfg.max_turns
    };
    let city_states = MapSize::from_dimensions(cfg.width, cfg.height)
        .map(|size| size.default_city_states)
        .unwrap_or(2);
    let mut g = Game::new(cfg.players, cfg.width, cfg.height, seed, turns, city_states);
    let mut ais = make_table(&g, w, opponents, seat);
    run_game(&mut g, &mut ais);
    let total: i64 = g
        .players
        .iter()
        .filter(|p| !p.is_minor)
        .map(|p| g.score(p.id))
        .sum();
    // normalized so an average-of-table score = 50; winning adds 100
    let mut fit = if total > 0 {
        50.0 * cfg.players as f64 * g.score(seat) as f64 / total as f64
    } else {
        0.0
    };
    let won = g.winner == Some(seat);
    if won {
        fit += 100.0;
    }
    let achievements: Vec<f64> = g
        .players
        .iter()
        .filter(|player| !player.is_minor)
        .map(|player| {
            let kills = player.counters.get("kills").copied().unwrap_or(0) as f64;
            let captures = player.counters.get("captures").copied().unwrap_or(0) as f64;
            kills + captures * 3.0
        })
        .collect();
    let total_achievements: f64 = achievements.iter().sum();
    if total_achievements > 0.0 {
        fit += 12.0 * cfg.players as f64 * achievements[seat] / total_achievements;
    }
    (fit, won)
}

/// Table: candidate at `seat` + ONE frozen-default anchor + champions. The
/// anchor keeps selection tied to absolute strength — pure champion-vs-champion
/// tables drift into intransitive cycles (beat the champ, not the game).
fn make_table(g: &Game, w: &Weights, opponents: &[Weights], seat: usize) -> Vec<AdvancedAi> {
    assert!(!opponents.is_empty());
    // Keep one absolute-strength anchor only when the table has another rival
    // seat available. At a duel table the sole opponent must be the champion;
    // otherwise evolution would never actually play its promotion target.
    let major_opponents = g
        .players
        .iter()
        .filter(|player| !player.is_minor && !player.is_barbarian && player.id != seat)
        .count();
    let mut anchor_left = major_opponents >= 2;
    let mut opponent_index = 0;
    g.players
        .iter()
        .map(|p| {
            if p.is_minor || p.is_barbarian {
                AdvancedAi::new()
            } else if p.id == seat {
                AdvancedAi::with_weights(w.clone())
            } else if anchor_left {
                anchor_left = false;
                AdvancedAi::legacy()
            } else {
                let weights = opponents[opponent_index % opponents.len()].clone();
                opponent_index += 1;
                AdvancedAi::with_weights(weights)
            }
        })
        .collect()
}

/// Per-player position features for value-net training (NNUE-style dataset).
/// All roughly 0..1-normalized; self block, best-opponent block, then turn.
pub fn features(g: &Game, pid: usize) -> Vec<f32> {
    let block = |p: usize| -> Vec<f32> {
        let cids = g.player_city_ids(p);
        let pop: i32 = cids.iter().map(|c| g.cities[c].pop).sum();
        let tiles: usize = cids.iter().map(|c| g.cities[c].owned_tiles.len()).sum();
        let mut yields = [0.0f64; 3]; // sci, cul, gold
        for c in &cids {
            let y = g.city_yields(*c);
            yields[0] += y.science;
            yields[1] += y.culture;
            yields[2] += y.gold;
        }
        let pl = &g.players[p];
        vec![
            cids.len() as f32 / 10.0,
            pop as f32 / 60.0,
            tiles as f32 / 80.0,
            pl.techs.len() as f32 / g.rules.techs.len() as f32,
            pl.civics.len() as f32 / g.rules.civics.len() as f32,
            g.military_power(p) as f32 / 200.0,
            g.player_unit_ids(p).len() as f32 / 20.0,
            yields[0] as f32 / 50.0,
            yields[1] as f32 / 50.0,
            yields[2] as f32 / 80.0,
            (pl.gold as f32 / 500.0).min(2.0),
            g.score(p) as f32 / 400.0,
        ]
    };
    let mut f = block(pid);
    let rival = g
        .players
        .iter()
        .filter(|p| p.id != pid && !p.is_minor && p.alive)
        .max_by_key(|p| g.score(p.id))
        .map(|p| p.id);
    f.extend(rival.map(&block).unwrap_or_else(|| vec![0.0; 12]));
    f.push(g.turn as f32 / g.max_turns.max(1) as f32);
    f
}

/// Play one game while sampling per-major position features every 16 turns;
/// rows labeled with the final outcome land in `rows`. Returns candidate won.
fn play_sampled(
    w: &Weights,
    champ: &Weights,
    seat: usize,
    cfg: &EvoCfg,
    seed: u64,
    long: bool,
    rows: &mut Vec<(Vec<f32>, bool)>,
) -> bool {
    let turns = if long {
        cfg.max_turns * 2
    } else {
        cfg.max_turns
    };
    let city_states = MapSize::from_dimensions(cfg.width, cfg.height)
        .map(|size| size.default_city_states)
        .unwrap_or(2);
    let mut g = Game::new(cfg.players, cfg.width, cfg.height, seed, turns, city_states);
    let mut ais = make_table(&g, w, std::slice::from_ref(champ), seat);
    let mut pending: Vec<(Vec<f32>, usize)> = Vec::new();
    let mut last_sample = 0;
    while g.winner.is_none() {
        let pid = g.current;
        ais[pid].take_turn(&mut g, pid);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
        if g.turn >= last_sample + 16 && g.winner.is_none() {
            last_sample = g.turn;
            for p in g.players.iter().filter(|p| !p.is_minor && p.alive) {
                pending.push((features(&g, p.id), p.id));
            }
        }
    }
    let winner = g.winner.unwrap();
    rows.extend(pending.into_iter().map(|(f, p)| (f, p == winner)));
    winner == seat
}

/// Fishtest-style SPRT match vs the champion: H0 win rate 0.25 (parity at a
/// 4-seat table), H1 0.40, α=β≈0.05. Returns (accepted, wins, losses).
/// Side effect: a quarter of the games feed the value-net position dataset.
fn sprt_confirm(
    cand: &Weights,
    champ: &Weights,
    cfg: &EvoCfg,
    gen: u32,
    rows: &mut Vec<(Vec<f32>, bool)>,
) -> (bool, u32, u32) {
    let (p0, p1) = (
        1.0 / cfg.players as f64,
        0.40f64.max(1.6 / cfg.players as f64),
    );
    let (lw, ll) = ((p1 / p0).ln(), ((1.0 - p1) / (1.0 - p0)).ln());
    let bound = 2.94;
    let (mut llr, mut w, mut l) = (0.0, 0u32, 0u32);
    for i in 0..200u64 {
        let seat = (i as usize) % cfg.players;
        let seed = 7_000_000 + gen as u64 * 10_000 + i;
        let won = if i % 4 == 0 {
            play_sampled(cand, champ, seat, cfg, seed, i % 3 == 2, rows)
        } else {
            eval_game(
                cand,
                std::slice::from_ref(champ),
                seat,
                cfg,
                seed,
                i % 3 == 2,
            )
            .1
        };
        if won {
            w += 1;
            llr += lw;
        } else {
            l += 1;
            llr += ll;
        }
        if llr >= bound {
            return (true, w, l);
        }
        if llr <= -bound {
            return (false, w, l);
        }
    }
    (false, w, l)
}

fn evaluate_all(pop: &[Weights], opponents: &[Weights], cfg: &EvoCfg, gen: u32) -> Vec<f64> {
    let n = pop.len();
    let mut fits = vec![0.0f64; n];
    let chunk = n.div_ceil(cfg.threads.max(1));
    std::thread::scope(|s| {
        for (pi, fi) in pop.chunks(chunk).zip(fits.chunks_mut(chunk)) {
            s.spawn(move || {
                for (j, w) in pi.iter().enumerate() {
                    let mut f = 0.0;
                    for gm in 0..cfg.games {
                        let seat = gm % cfg.players;
                        // same seeds for every genome → paired comparison
                        let seed = cfg.seed + gen as u64 * 1_000 + gm as u64;
                        f += eval_game(w, opponents, seat, cfg, seed, gm % 3 == 2).0;
                    }
                    fi[j] = f / cfg.games as f64;
                }
            });
        }
    });
    fits
}

fn mutate(w: &Weights, rng: &mut Rng, bounds: &[(f64, f64)]) -> Weights {
    let mut v = w.to_vec();
    for (i, g) in v.iter_mut().enumerate() {
        let (lo, hi) = bounds[i];
        if rng.chance(0.08) {
            *g = rng.uniform(lo, hi); // occasional full reroll
        } else if rng.chance(0.35) {
            *g += rng.uniform(-0.12, 0.12) * (hi - lo);
        }
        *g = g.clamp(lo, hi);
    }
    Weights::from_vec(&v)
}

fn crossover(a: &Weights, b: &Weights, rng: &mut Rng) -> Weights {
    let (va, vb) = (a.to_vec(), b.to_vec());
    let v: Vec<f64> = va
        .iter()
        .zip(&vb)
        .map(|(x, y)| if rng.chance(0.5) { *x } else { *y })
        .collect();
    Weights::from_vec(&v)
}

fn next_generation(
    pop: &[Weights],
    fits: &[f64],
    target_size: usize,
    rng: &mut Rng,
    bounds: &[(f64, f64)],
) -> Vec<Weights> {
    assert_eq!(pop.len(), fits.len());
    if pop.is_empty() || target_size == 0 {
        return Vec::new();
    }
    let mut ranked: Vec<usize> = (0..pop.len()).collect();
    ranked.sort_by(|a, b| fits[*b].partial_cmp(&fits[*a]).unwrap());
    // Elitism guarantees a measured improvement cannot disappear in its own
    // generation; the remainder recombines and mutates the fitter half.
    let elite = (target_size / 4).max(1).min(pop.len()).min(target_size);
    let mut next: Vec<Weights> = ranked[..elite]
        .iter()
        .map(|index| pop[*index].clone())
        .collect();
    let parent_pool = (pop.len() / 2).max(1);
    while next.len() < target_size {
        let a = &pop[ranked[rng.below(parent_pool)]];
        let b = &pop[ranked[rng.below(parent_pool)]];
        next.push(mutate(&crossover(a, b, rng), rng, bounds));
    }
    next
}

fn validation_game_count(cfg: &EvoCfg) -> usize {
    cfg.games
        .max(cfg.players)
        .min(cfg.players.saturating_mul(4).max(1))
}

/// Fixed-seed holdout score against the default coordinated agent. Unlike
/// per-generation training fitness, this number is directly comparable over
/// time and catches champion-overfitting or strategic forgetting.
fn validation_score(w: &Weights, cfg: &EvoCfg, games: usize) -> f64 {
    let baseline = Weights::default();
    let opponents = std::slice::from_ref(&baseline);
    (0..games)
        .map(|game| {
            let seat = game % cfg.players;
            let seed = (cfg.seed ^ 0xD1B5_4A32_D192_ED03).wrapping_add(game as u64);
            eval_game(w, opponents, seat, cfg, seed, game % 3 == 2).0
        })
        .sum::<f64>()
        / games.max(1) as f64
}

fn load_archive(dir: &Path) -> Vec<Weights> {
    fs::read_to_string(dir.join("archive.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<ArchiveState>(&raw).ok())
        .map(|state| state.champions)
        .unwrap_or_default()
}

fn opponent_pool(champ: &Weights, archive: &[Weights]) -> Vec<Weights> {
    let mut pool = vec![champ.clone()];
    for past in archive.iter().rev() {
        if !pool.iter().any(|weights| weights == past) {
            pool.push(past.clone());
        }
        if pool.len() >= 8 {
            break;
        }
    }
    pool
}

fn save_archive(dir: &Path, champions: &[Weights]) {
    let keep_from = champions.len().saturating_sub(16);
    let state = ArchiveState {
        champions: champions[keep_from..].to_vec(),
    };
    let _ = fs::write(
        dir.join("archive.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    );
}

pub fn evolve(cfg: &EvoCfg) {
    fs::create_dir_all(&cfg.dir).ok();
    let dir = Path::new(&cfg.dir);
    let mut rng = Rng::new(cfg.seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let bounds = Weights::bounds();

    let saved_champion = load_champion_record(dir);
    let mut champ = saved_champion
        .as_ref()
        .map(|record| record.weights.clone())
        .unwrap_or_default();
    let validation_games = validation_game_count(cfg);
    let mut champ_validation = validation_score(&champ, cfg, validation_games);
    let mut archive = load_archive(dir);
    if archive.is_empty() {
        archive.push(Weights::default());
    }
    if !archive.iter().any(|weights| weights == &champ) {
        archive.push(champ.clone());
    }
    let saved_gen = saved_champion
        .as_ref()
        .map(|record| record.gen)
        .unwrap_or(0);
    let saved_fitness = saved_champion
        .as_ref()
        .map(|record| record.fitness)
        .unwrap_or(50.0);
    save_champ(
        dir,
        saved_gen,
        saved_fitness,
        champ_validation,
        validation_games,
        &champ,
    );
    save_archive(dir, &archive);
    let (gen0, mut pop) = fs::read_to_string(dir.join("population.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<PopState>(&s).ok())
        .map(|p| (p.gen, p.genomes))
        .unwrap_or((0, Vec::new()));
    if pop.is_empty() {
        pop.push(champ.clone());
        pop.push(Weights::default());
    }
    while pop.len() < cfg.pop {
        pop.push(mutate(&champ, &mut rng, &bounds));
    }
    pop.truncate(cfg.pop);

    println!(
        "evolve: pop {} · {} games/genome · {}x{} {}p {}t · {} threads · resuming at gen {}",
        cfg.pop, cfg.games, cfg.width, cfg.height, cfg.players, cfg.max_turns, cfg.threads, gen0
    );
    for gen in gen0..gen0.saturating_add(cfg.generations) {
        let opponents = opponent_pool(&champ, &archive);
        let fits = evaluate_all(&pop, &opponents, cfg, gen);
        let mut idx: Vec<usize> = (0..pop.len()).collect();
        idx.sort_by(|a, b| fits[*b].partial_cmp(&fits[*a]).unwrap());
        let (best, mean) = (idx[0], fits.iter().sum::<f64>() / fits.len() as f64);
        let candidate_validation = validation_score(&pop[best], cfg, validation_games);
        // Training fitness screens candidates; promotion additionally requires
        // both a sequential match win and no regression on fixed holdout maps.
        let mut promoted = false;
        let mut sprt_note = String::new();
        let screening_threshold = 65.0 + 100.0 / cfg.players.max(1) as f64;
        if fits[best] > screening_threshold {
            let mut rows: Vec<(Vec<f32>, bool)> = Vec::new();
            let (ok, w, l) = sprt_confirm(&pop[best], &champ, cfg, gen, &mut rows);
            if let Ok(mut f) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("dataset.csv"))
            {
                for (feats, won) in &rows {
                    let line: Vec<String> = feats.iter().map(|x| format!("{x:.4}")).collect();
                    let _ = writeln!(f, "{},{}", line.join(","), *won as u8);
                }
            }
            let holdout_ok = candidate_validation + 1e-9 >= champ_validation;
            promoted = ok && holdout_ok;
            sprt_note = format!(
                "  SPRT {w}-{l} {} · holdout {:.1}/{:.1} {}",
                if promoted {
                    "ACCEPT → NEW CHAMPION"
                } else if ok {
                    "win, validation veto"
                } else {
                    "reject"
                },
                candidate_validation,
                champ_validation,
                if holdout_ok { "pass" } else { "regression" },
            );
        }
        println!(
            "gen {gen}: best {:.1} mean {:.1} · validation {:.1}/{:.1}{sprt_note}",
            fits[best], mean, candidate_validation, champ_validation
        );
        if promoted {
            champ = pop[best].clone();
            champ_validation = candidate_validation;
            archive.push(champ.clone());
            save_archive(dir, &archive);
            save_champ(
                dir,
                gen,
                fits[best],
                champ_validation,
                validation_games,
                &champ,
            );
        }
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("history.csv"))
        {
            let _ = writeln!(
                f,
                "{gen},{:.2},{:.2},{:.2},{:.2},{}",
                fits[best], mean, candidate_validation, champ_validation, promoted as u8
            );
        }
        pop = next_generation(&pop, &fits, cfg.pop, &mut rng, &bounds);
        let st = PopState {
            gen: gen + 1,
            genomes: pop.clone(),
        };
        let _ = fs::write(
            dir.join("population.json"),
            serde_json::to_string_pretty(&st).unwrap(),
        );
    }
}

fn save_champ(
    dir: &Path,
    gen: u32,
    fitness: f64,
    validation_score: f64,
    validation_games: usize,
    w: &Weights,
) {
    let c = Champion {
        gen,
        fitness,
        validation_score,
        validation_games,
        weights: w.clone(),
    };
    let _ = fs::write(
        dir.join("best.json"),
        serde_json::to_string_pretty(&c).unwrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_genome_round_trips_all_doctrine_genes() {
        let weights = Weights::default();
        let genes = weights.to_vec();
        assert_eq!(genes.len(), Weights::bounds().len());
        assert_eq!(Weights::from_vec(&genes), weights);

        // Old champion files omit coordinated-combat fields. Container-level
        // serde defaults keep those long-running checkpoints forward-compatible.
        let restored: Weights = serde_json::from_str("{}").unwrap();
        assert_eq!(restored.command_radius, weights.command_radius);
        assert_eq!(restored.focus_fire, weights.focus_fire);
    }

    #[test]
    fn breeding_preserves_improvement_and_bounds_every_gene() {
        let bounds = Weights::bounds();
        let lower = Weights::from_vec(&bounds.iter().map(|(lo, _)| *lo).collect::<Vec<_>>());
        let upper = Weights::from_vec(&bounds.iter().map(|(_, hi)| *hi).collect::<Vec<_>>());
        let pop = vec![lower.clone(), Weights::default(), upper.clone(), lower];
        let mut rng = Rng::new(91);
        let next = next_generation(&pop, &[1.0, 2.0, 100.0, 0.0], 16, &mut rng, &bounds);

        assert_eq!(next[0], upper, "elitism must preserve measured improvement");
        assert!(next.iter().all(|genome| {
            genome
                .to_vec()
                .iter()
                .zip(bounds.iter())
                .all(|(gene, (lo, hi))| *gene >= *lo && *gene <= *hi)
        }));
        assert!(
            next.iter().skip(1).any(|genome| {
                genome.command_radius != pop[0].command_radius
                    || genome.focus_fire != pop[0].focus_fire
                    || genome.muster_readiness != pop[0].muster_readiness
            }),
            "offspring should explore coordinated-combat doctrine"
        );
    }

    #[test]
    fn duel_tables_face_the_champion_and_larger_tables_keep_an_anchor() {
        let candidate = Weights::default();
        let mut champion = Weights::default();
        champion.focus_fire = 7.25;

        let duel = Game::new_full(2, 20, 14, 92, 20, 0, false);
        let duel_table = make_table(&duel, &candidate, std::slice::from_ref(&champion), 0);
        assert!(duel_table[1].coordinates_forces());
        assert_eq!(duel_table[1].strategy_weights().focus_fire, 7.25);

        let four = Game::new_full(4, 26, 16, 93, 20, 0, false);
        let four_table = make_table(&four, &candidate, std::slice::from_ref(&champion), 0);
        assert!(
            !four_table[1].coordinates_forces(),
            "one seat is the frozen anchor"
        );
        assert!(four_table[2].coordinates_forces());
        assert_eq!(four_table[2].strategy_weights().focus_fire, 7.25);
    }

    #[test]
    fn holdout_metric_is_fixed_and_archive_pool_preserves_diversity() {
        let cfg = EvoCfg {
            generations: 1,
            pop: 4,
            games: 2,
            players: 2,
            width: 20,
            height: 14,
            max_turns: 16,
            seed: 94,
            threads: 1,
            dir: String::new(),
        };
        let weights = Weights::default();
        let first = validation_score(&weights, &cfg, 2);
        let second = validation_score(&weights, &cfg, 2);
        assert_eq!(
            first, second,
            "holdout seeds must be generation-independent"
        );

        let archive: Vec<Weights> = (0..12)
            .map(|index| {
                let mut weights = Weights::default();
                weights.focus_fire = index as f64 / 2.0;
                weights
            })
            .collect();
        let pool = opponent_pool(&archive[11], &archive);
        assert_eq!(pool[0], archive[11]);
        assert_eq!(pool.len(), 8);
        assert!(pool.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn old_champion_records_default_new_validation_metadata() {
        let champion: Champion =
            serde_json::from_str(r#"{"gen":3,"fitness":91.5,"weights":{}}"#).unwrap();
        assert_eq!(champion.validation_score, 0.0);
        assert_eq!(champion.validation_games, 0);
    }
}
