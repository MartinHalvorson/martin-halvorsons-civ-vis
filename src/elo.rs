//! Elo tournament harness: evaluate AI strategies against each other.
//! Each game scores as pairwise Elo results by final placement
//! (K/(n-1) scaling for multiplayer).
use std::collections::BTreeMap;

use crate::ai::{run_game, Ai, BasicAi, RandomAi};
use crate::game::Game;
use crate::rng::Rng;

pub const BUILTIN_AIS: [&str; 3] = ["basic", "random", "evolved"];

pub fn expected(ra: f64, rb: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((rb - ra) / 400.0))
}

pub struct EloPool {
    pub ratings: BTreeMap<String, f64>,
    pub games: BTreeMap<String, u32>,
    pub wins: BTreeMap<String, u32>,
}

impl EloPool {
    pub fn new(names: &[String], base: f64) -> EloPool {
        EloPool {
            ratings: names.iter().map(|n| (n.clone(), base)).collect(),
            games: names.iter().map(|n| (n.clone(), 0)).collect(),
            wins: names.iter().map(|n| (n.clone(), 0)).collect(),
        }
    }

    pub fn record(&mut self, placements: &[String], k: f64) {
        let n = placements.len();
        if n < 2 {
            return;
        }
        let mut delta: BTreeMap<&str, f64> = BTreeMap::new();
        for i in 0..n {
            for j in (i + 1)..n {
                let (a, b) = (placements[i].as_str(), placements[j].as_str());
                if a == b {
                    continue;
                }
                let ea = expected(self.ratings[a], self.ratings[b]);
                let gain = k / (n as f64 - 1.0) * (1.0 - ea);
                *delta.entry(a).or_insert(0.0) += gain;
                *delta.entry(b).or_insert(0.0) -= gain;
            }
        }
        for (name, d) in delta {
            *self.ratings.get_mut(name).unwrap() += d;
        }
        for (idx, name) in placements.iter().enumerate() {
            *self.games.get_mut(name.as_str()).unwrap() += 1;
            if idx == 0 {
                *self.wins.get_mut(name.as_str()).unwrap() += 1;
            }
        }
    }
}

pub fn builtin_ai(name: &str, seed: u64) -> Box<dyn Ai> {
    match name {
        "random" => Box::new(RandomAi::new(seed)),
        "evolved" => Box::new(
            crate::evolve::load_champion("evolved")
                .map(BasicAi::with_weights)
                .unwrap_or_else(BasicAi::new)),
        _ => Box::new(BasicAi::new()),
    }
}

pub struct TourneyCfg {
    pub games: u32,
    pub players_per_game: usize,
    pub width: i32,
    pub height: i32,
    pub max_turns: u32,
    pub num_city_states: usize,
    pub seed: u64,
    pub k: f64,
    pub verbose: bool,
}

impl Default for TourneyCfg {
    fn default() -> Self {
        TourneyCfg { games: 20, players_per_game: 4, width: 24, height: 16,
                     max_turns: 150, num_city_states: 2, seed: 0, k: 24.0,
                     verbose: true }
    }
}

pub fn run_tournament<F>(names: &[String], make: F, cfg: &TourneyCfg) -> EloPool
where
    F: Fn(&str, u64) -> Box<dyn Ai>,
{
    assert!(!names.is_empty(), "no entrants");
    let mut rng = Rng::new(cfg.seed.wrapping_add(0x5EED));
    let mut pool = EloPool::new(names, 1000.0);
    for gi in 0..cfg.games {
        let gseed = cfg.seed * 100_000 + gi as u64;
        let mut seats: Vec<String> = (0..cfg.players_per_game)
            .map(|_| names[rng.below(names.len())].clone())
            .collect();
        if names.len() > 1 && seats.iter().all(|s| *s == seats[0]) {
            let others: Vec<&String> = names.iter()
                .filter(|n| **n != seats[0]).collect();
            let i = rng.below(cfg.players_per_game);
            seats[i] = others[rng.below(others.len())].clone();
        }
        let mut game = Game::new(cfg.players_per_game, cfg.width, cfg.height,
                                 gseed, cfg.max_turns, cfg.num_city_states);
        let mut ais: Vec<Box<dyn Ai>> = game.players.iter().map(|p| {
            if p.id < cfg.players_per_game {
                make(&seats[p.id], gseed + p.id as u64)
            } else {
                builtin_ai("basic", gseed + p.id as u64)
            }
        }).collect();
        run_game(&mut game, &mut ais);
        let winner = game.winner.unwrap();
        let mut ranked: Vec<usize> = (0..cfg.players_per_game).collect();
        ranked.sort_by_key(|pid| (*pid != winner, -game.score(*pid), *pid));
        let placements: Vec<String> = ranked.iter()
            .map(|pid| seats[*pid].clone()).collect();
        pool.record(&placements, cfg.k);
        if cfg.verbose {
            let wname = if winner < cfg.players_per_game {
                seats[winner].clone()
            } else {
                game.players[winner].civ.clone()
            };
            println!("game {gi:3}  winner={wname:<10} ({}, {}, t{})  seats={seats:?}",
                     game.players[winner].civ,
                     game.victory_type.clone().unwrap_or_default(), game.turn);
        }
    }
    pool
}

pub fn leaderboard(pool: &EloPool) -> String {
    let mut rows: Vec<(&String, f64)> = pool.ratings.iter()
        .map(|(n, r)| (n, *r)).collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(b.0)));
    let mut out = String::from("Elo leaderboard:\n");
    for (name, r) in rows {
        let g = pool.games[name.as_str()];
        let w = pool.wins[name.as_str()];
        out.push_str(&format!(
            "  {:<14} {:7.1}   games={:<4} wins={:<4} winrate={:.0}%\n",
            name, r, g, w, 100.0 * w as f64 / g.max(1) as f64));
    }
    out
}
