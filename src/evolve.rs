//! Genetic-algorithm search over BasicAi strategy weights.
//! `civvis evolve` runs generations forever (checkpointed every generation):
//! each genome plays vs the reigning champion on shared maps; the champion is
//! replaced when a genome clearly outperforms champion-level opposition.
//! Artifacts in evolved/: best.json (champion), population.json (resume
//! state), history.csv (fitness per generation).
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ai::{run_game, Ai, BasicAi, Weights};
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
    pub weights: Weights,
}

#[derive(Serialize, Deserialize)]
struct PopState {
    gen: u32,
    genomes: Vec<Weights>,
}

pub fn load_champion(dir: &str) -> Option<Weights> {
    let raw = fs::read_to_string(Path::new(dir).join("best.json")).ok()?;
    serde_json::from_str::<Champion>(&raw).ok().map(|c| c.weights)
}

/// Fitness of one game: 50 * major-score share (+100 on outright win).
/// 50 ≈ parity with the champion opponents filling the other seats.
fn eval_game(w: &Weights, champ: &Weights, seat: usize, cfg: &EvoCfg, seed: u64,
             long: bool) -> (f64, bool) {
    // mix game lengths so champions aren't tuned only for short score races
    let turns = if long { cfg.max_turns * 2 } else { cfg.max_turns };
    let city_states = MapSize::from_dimensions(cfg.width, cfg.height)
        .map(|size| size.default_city_states)
        .unwrap_or(2);
    let mut g = Game::new(cfg.players, cfg.width, cfg.height, seed, turns, city_states);
    let mut ais = make_table(&g, w, champ, seat);
    run_game(&mut g, &mut ais);
    let total: i64 = g.players.iter().filter(|p| !p.is_minor)
        .map(|p| g.score(p.id)).sum();
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
    (fit, won)
}

/// Table: candidate at `seat` + ONE frozen-default anchor + champions. The
/// anchor keeps selection tied to absolute strength — pure champion-vs-champion
/// tables drift into intransitive cycles (beat the champ, not the game).
fn make_table(g: &Game, w: &Weights, champ: &Weights, seat: usize) -> Vec<BasicAi> {
    let mut anchor_left = true;
    g.players.iter().map(|p| {
        if p.is_minor || p.is_barbarian {
            BasicAi::new()
        } else if p.id == seat {
            BasicAi::with_weights(w.clone())
        } else if anchor_left {
            anchor_left = false;
            BasicAi::new()
        } else {
            BasicAi::with_weights(champ.clone())
        }
    }).collect()
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
            pl.techs.len() as f32 / 30.0,
            pl.civics.len() as f32 / 15.0,
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
    let rival = g.players.iter()
        .filter(|p| p.id != pid && !p.is_minor && p.alive)
        .max_by_key(|p| g.score(p.id)).map(|p| p.id);
    f.extend(rival.map(&block).unwrap_or_else(|| vec![0.0; 12]));
    f.push(g.turn as f32 / g.max_turns.max(1) as f32);
    f
}

/// Play one game while sampling per-major position features every 16 turns;
/// rows labeled with the final outcome land in `rows`. Returns candidate won.
fn play_sampled(w: &Weights, champ: &Weights, seat: usize, cfg: &EvoCfg,
                seed: u64, long: bool, rows: &mut Vec<(Vec<f32>, bool)>) -> bool {
    let turns = if long { cfg.max_turns * 2 } else { cfg.max_turns };
    let city_states = MapSize::from_dimensions(cfg.width, cfg.height)
        .map(|size| size.default_city_states)
        .unwrap_or(2);
    let mut g = Game::new(cfg.players, cfg.width, cfg.height, seed, turns, city_states);
    let mut ais = make_table(&g, w, champ, seat);
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
fn sprt_confirm(cand: &Weights, champ: &Weights, cfg: &EvoCfg, gen: u32,
                rows: &mut Vec<(Vec<f32>, bool)>) -> (bool, u32, u32) {
    let (p0, p1) = (1.0 / cfg.players as f64, 0.40f64.max(1.6 / cfg.players as f64));
    let (lw, ll) = ((p1 / p0).ln(), ((1.0 - p1) / (1.0 - p0)).ln());
    let bound = 2.94;
    let (mut llr, mut w, mut l) = (0.0, 0u32, 0u32);
    for i in 0..200u64 {
        let seat = (i as usize) % cfg.players;
        let seed = 7_000_000 + gen as u64 * 10_000 + i;
        let won = if i % 4 == 0 {
            play_sampled(cand, champ, seat, cfg, seed, i % 3 == 2, rows)
        } else {
            eval_game(cand, champ, seat, cfg, seed, i % 3 == 2).1
        };
        if won { w += 1; llr += lw; } else { l += 1; llr += ll; }
        if llr >= bound {
            return (true, w, l);
        }
        if llr <= -bound {
            return (false, w, l);
        }
    }
    (false, w, l)
}

fn evaluate_all(pop: &[Weights], champ: &Weights, cfg: &EvoCfg, gen: u32) -> Vec<f64> {
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
                        f += eval_game(w, champ, seat, cfg, seed, gm % 3 == 2).0;
                    }
                    fi[j] = f / cfg.games as f64;
                }
            });
        }
    });
    fits
}

fn mutate(w: &Weights, rng: &mut Rng, bounds: &[(f64, f64); 29]) -> Weights {
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
    let v: Vec<f64> = va.iter().zip(&vb)
        .map(|(x, y)| if rng.chance(0.5) { *x } else { *y })
        .collect();
    Weights::from_vec(&v)
}

pub fn evolve(cfg: &EvoCfg) {
    fs::create_dir_all(&cfg.dir).ok();
    let dir = Path::new(&cfg.dir);
    let mut rng = Rng::new(cfg.seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let bounds = Weights::bounds();

    let mut champ = load_champion(&cfg.dir).unwrap_or_default();
    if !dir.join("best.json").exists() {
        save_champ(dir, 0, 50.0, &champ);
    }
    let (gen0, mut pop) = fs::read_to_string(dir.join("population.json")).ok()
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

    println!("evolve: pop {} · {} games/genome · {}x{} {}p {}t · {} threads · resuming at gen {}",
             cfg.pop, cfg.games, cfg.width, cfg.height, cfg.players,
             cfg.max_turns, cfg.threads, gen0);
    for gen in gen0..gen0.saturating_add(cfg.generations) {
        let fits = evaluate_all(&pop, &champ, cfg, gen);
        let mut idx: Vec<usize> = (0..pop.len()).collect();
        idx.sort_by(|a, b| fits[*b].partial_cmp(&fits[*a]).unwrap());
        let (best, mean) = (idx[0], fits.iter().sum::<f64>() / fits.len() as f64);
        // parity vs the champion table ≈ 75 (50 score-share + 25% win rate);
        // screening only — promotion requires winning an SPRT match
        let mut promoted = false;
        let mut sprt_note = String::new();
        if fits[best] > 78.0 {
            let mut rows: Vec<(Vec<f32>, bool)> = Vec::new();
            let (ok, w, l) = sprt_confirm(&pop[best], &champ, cfg, gen, &mut rows);
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true)
                .open(dir.join("dataset.csv")) {
                for (feats, won) in &rows {
                    let line: Vec<String> = feats.iter()
                        .map(|x| format!("{x:.4}")).collect();
                    let _ = writeln!(f, "{},{}", line.join(","), *won as u8);
                }
            }
            promoted = ok;
            sprt_note = format!("  SPRT {w}-{l} {}", if ok { "ACCEPT → NEW CHAMPION" }
                                                     else { "reject" });
        }
        println!("gen {gen}: best {:.1} mean {:.1}{sprt_note}", fits[best], mean);
        if promoted {
            champ = pop[best].clone();
            save_champ(dir, gen, fits[best], &champ);
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true)
            .open(dir.join("history.csv")) {
            let _ = writeln!(f, "{gen},{:.2},{:.2},{}", fits[best], mean,
                             promoted as u8);
        }
        // next generation: elites survive, rest bred from the top half
        let elite = (cfg.pop / 4).max(2);
        let mut next: Vec<Weights> = idx[..elite].iter()
            .map(|i| pop[*i].clone()).collect();
        let half = (pop.len() / 2).max(1);
        while next.len() < cfg.pop {
            let a = &pop[idx[rng.below(half)]];
            let b = &pop[idx[rng.below(half)]];
            next.push(mutate(&crossover(a, b, &mut rng), &mut rng, &bounds));
        }
        pop = next;
        let st = PopState { gen: gen + 1, genomes: pop.clone() };
        let _ = fs::write(dir.join("population.json"),
                          serde_json::to_string_pretty(&st).unwrap());
    }
}

fn save_champ(dir: &Path, gen: u32, fitness: f64, w: &Weights) {
    let c = Champion { gen, fitness, weights: w.clone() };
    let _ = fs::write(dir.join("best.json"),
                      serde_json::to_string_pretty(&c).unwrap());
}
