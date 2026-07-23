//! Elo tournament harness: evaluate AI strategies against each other.
//! Each game scores as pairwise Elo results by final placement
//! (K/(n-1) scaling for multiplayer).
use std::collections::BTreeMap;

use crate::ai::{run_game, AdvancedAi, Ai, BasicAi, RandomAi};
use crate::game::Game;
use crate::rng::Rng;
use crate::setup::MapSize;

pub const BUILTIN_AIS: [&str; 9] = [
    "advanced",
    "advanced_evolved",
    "advanced_v1",
    "basic",
    "random",
    "evolved",
    "neural",
    "strategic",
    "policy",
];

pub fn expected(ra: f64, rb: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((rb - ra) / 400.0))
}

/// Each rating's chance of *winning outright* against the rest of the table,
/// summing to 1.
///
/// A multiplayer table has one winner, so averaging the pairwise `expected`
/// scores does not answer "who wins this game": those averages sit around
/// 0.5 each and sum to n/2, which reads as a six-way table of 50% favourites.
/// Weighting by each rating's Elo strength `10^(r/400)` and normalizing gives
/// a real distribution, and for two players it collapses back to `expected`.
pub fn win_shares(ratings: &[f64]) -> Vec<f64> {
    // Offset by the strongest rating before exponentiating: the ratio is
    // unchanged and no term can overflow, however far the table has drifted.
    let top = ratings.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = ratings
        .iter()
        .map(|r| 10f64.powf((r - top) / 400.0))
        .collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return vec![1.0 / ratings.len().max(1) as f64; ratings.len()];
    }
    weights.iter().map(|w| w / total).collect()
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
        "advanced" => Box::new(AdvancedAi::new()),
        "advanced_evolved" => Box::new(
            crate::evolve::load_champion("evolved")
                .map(AdvancedAi::with_weights)
                .unwrap_or_else(AdvancedAi::new),
        ),
        "advanced_v1" => Box::new(AdvancedAi::legacy()),
        "random" => Box::new(RandomAi::new(seed)),
        "evolved" => Box::new(
            crate::evolve::load_champion("evolved")
                .map(AdvancedAi::with_weights)
                .unwrap_or_default(),
        ),
        "neural" => {
            let w = crate::evolve::load_champion("evolved").unwrap_or_default();
            match crate::valuenet::ValueNet::load("evolved") {
                Some(n) => Box::new(crate::neural::NeuralAi::new(w, n)),
                None => Box::new(BasicAi::with_weights(w)),
            }
        }
        "policy" => Box::new(crate::policy::PolicyAi::with_weights(
            crate::evolve::load_champion("evolved").unwrap_or_default(),
        )),
        "strategic" => Box::new(crate::strategic::StrategicAi::with_weights(
            crate::evolve::load_champion("evolved").unwrap_or_default(),
        )),
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
    /// How many games to play at once. Seating and ratings stay in game
    /// order, so this changes only how long the tournament takes.
    pub jobs: usize,
}

impl Default for TourneyCfg {
    fn default() -> Self {
        let size = MapSize::for_players(4);
        TourneyCfg {
            games: 20,
            players_per_game: 4,
            width: size.width,
            height: size.height,
            max_turns: 150,
            num_city_states: size.default_city_states,
            seed: 0,
            k: 24.0,
            verbose: true,
            jobs: crate::parallel::default_jobs(),
        }
    }
}

pub fn run_tournament<F>(names: &[String], make: F, cfg: &TourneyCfg) -> EloPool
where
    F: Fn(&str, u64) -> Box<dyn Ai> + Sync,
{
    assert!(!names.is_empty(), "no entrants");
    // Seating is drawn from one stream and ratings are updated in game order,
    // so both stay on this thread; only the games themselves are spread out.
    // A tournament therefore produces the same table however many cores run
    // it.
    let mut rng = Rng::new(cfg.seed.wrapping_add(0x5EED));
    let mut pool = EloPool::new(names, 1000.0);
    let draws: Vec<(u64, Vec<String>)> = (0..cfg.games)
        .map(|gi| {
            let gseed = cfg.seed * 100_000 + gi as u64;
            let mut seats: Vec<String> = (0..cfg.players_per_game)
                .map(|_| names[rng.below(names.len())].clone())
                .collect();
            if names.len() > 1 && seats.iter().all(|s| *s == seats[0]) {
                let others: Vec<&String> = names.iter().filter(|n| **n != seats[0]).collect();
                let i = rng.below(cfg.players_per_game);
                seats[i] = others[rng.below(others.len())].clone();
            }
            (gseed, seats)
        })
        .collect();

    let played = crate::parallel::map(draws.len(), cfg.jobs, |gi| {
        let (gseed, seats) = &draws[gi];
        let mut game = Game::new(
            cfg.players_per_game,
            cfg.width,
            cfg.height,
            *gseed,
            cfg.max_turns,
            cfg.num_city_states,
        );
        let mut ais: Vec<Box<dyn Ai>> = game
            .players
            .iter()
            .map(|p| {
                if p.id < cfg.players_per_game {
                    make(&seats[p.id], gseed + p.id as u64)
                } else {
                    builtin_ai("basic", gseed + p.id as u64)
                }
            })
            .collect();
        run_game(&mut game, &mut ais);
        game
    });

    for (gi, game) in played.into_iter().enumerate() {
        let seats = &draws[gi].1;
        let winner = game.winner.unwrap();
        let mut ranked: Vec<usize> = (0..cfg.players_per_game).collect();
        ranked.sort_by_key(|pid| (*pid != winner, -game.score(*pid), *pid));
        let placements: Vec<String> = ranked.iter().map(|pid| seats[*pid].clone()).collect();
        pool.record(&placements, cfg.k);
        if cfg.verbose {
            let wname = if winner < cfg.players_per_game {
                seats[winner].clone()
            } else {
                game.players[winner].civ.clone()
            };
            println!(
                "game {gi:3}  winner={wname:<10} ({}, {}, t{})  seats={seats:?}",
                game.players[winner].civ,
                game.victory_type.clone().unwrap_or_default(),
                game.turn
            );
        }
    }
    pool
}

pub fn leaderboard(pool: &EloPool) -> String {
    let mut rows: Vec<(&String, f64)> = pool.ratings.iter().map(|(n, r)| (n, *r)).collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(b.0)));
    let mut out = String::from("Elo leaderboard:\n");
    for (name, r) in rows {
        let g = pool.games[name.as_str()];
        let w = pool.wins[name.as_str()];
        out.push_str(&format!(
            "  {:<14} {:7.1}   games={:<4} wins={:<4} winrate={:.0}%\n",
            name,
            r,
            g,
            w,
            100.0 * w as f64 / g.max(1) as f64
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number the HUD prints as a win chance has to behave like one: the
    /// table sums to 1, the favourite leads, and a two-player table agrees
    /// with the ordinary Elo expectation.
    #[test]
    fn win_shares_are_a_distribution_over_the_table() {
        let table = [1914.0, 1865.0, 1836.0, 1847.0, 1766.0, 1755.0];
        let shares = win_shares(&table);
        let total: f64 = shares.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "shares sum to {total}");
        assert!(shares.iter().all(|s| *s > 0.0 && *s < 1.0));
        // Ordering follows rating, and the favourite beats an even split.
        assert!(shares[0] > shares[3] && shares[3] > shares[1].min(shares[2]));
        assert!(shares[0] > 1.0 / table.len() as f64);
        assert!(shares[5] < 1.0 / table.len() as f64);

        let pair = win_shares(&[1600.0, 1400.0]);
        assert!((pair[0] - expected(1600.0, 1400.0)).abs() < 1e-12);
        assert!((pair[1] - expected(1400.0, 1600.0)).abs() < 1e-12);

        // Equal ratings split evenly, and a drifted table cannot overflow.
        for s in win_shares(&[1500.0; 4]) {
            assert!((s - 0.25).abs() < 1e-12);
        }
        let wide = win_shares(&[40_000.0, 0.0]);
        assert!((wide[0] + wide[1] - 1.0).abs() < 1e-9 && wide[0] > 0.999);
    }
}
