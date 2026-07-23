//! Glicko-2 strategy league: persistent skill ratings for high-level AI
//! strategies, with periodic selection so strong strategies breed offspring
//! and confidently weak ones retire.
//!
//! `civvis league` plays rating periods ("rounds") of multiplayer games
//! between named strategies — built-in agents plus parameterized AdvancedAi
//! variants (a `Weights` genome and an optional fixed victory lane). Each
//! round is one Glicko-2 rating period: every finished game decomposes into
//! pairwise results by placement, all games in the round update ratings at
//! once, and a strategy that sat out has only its uncertainty grow. Glicko-2
//! rather than Elo because the roster churns: a newborn strategy enters at
//! high rating deviation and converges quickly, while retirement decisions
//! can demand low deviation so nothing is culled on a small sample.
//!
//! Artifacts in the league dir: league.json (full roster + ratings, the one
//! source of truth), ratings.csv (per-round rating history), matches.csv
//! (every game played).
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ai::{run_game, AdvancedAi, Ai, VictoryTarget, Weights};
use crate::game::Game;
use crate::rng::Rng;
use crate::setup::MapSize;

/// Glicko-2 works on an internal scale; ratings are stored and shown on the
/// familiar Elo-like scale (1500 start).
const SCALE: f64 = 173.7178;
const BASE_RATING: f64 = 1500.0;
const BASE_RD: f64 = 350.0;
const BASE_VOL: f64 = 0.06;
/// System constant: how much volatility can move per period. 0.5 is the
/// conservative end of Glickman's recommended 0.3..1.2.
const TAU: f64 = 0.5;
/// Retirement needs evidence: this many games and the deviation below this
/// bound, so an unlucky newcomer is never culled on noise.
const MIN_GAMES_TO_RETIRE: u32 = 20;
const MAX_RD_TO_RETIRE: f64 = 110.0;

/// How a seat materializes an `Ai`.
#[derive(Clone, Serialize, Deserialize)]
pub enum StrategyKind {
    /// One of `elo::BUILTIN_AIS`.
    Builtin { ai: String },
    /// Parameterized AdvancedAi: a genome plus an optional fixed victory
    /// lane (stored as text; `VictoryTarget` parses it).
    Advanced {
        weights: Weights,
        target: Option<String>,
    },
}

/// Glicko state of one strategy playing one particular civilization.
/// Opponents are measured by their *global* rating, so this answers "how
/// strong is this strategy when it draws this civ" on the same scale as
/// the overall table. Not every civ wants to play the same way, so the
/// same strategy legitimately carries different numbers per civ.
#[derive(Clone, Serialize, Deserialize)]
pub struct CivRating {
    pub rating: f64,
    pub rd: f64,
    pub vol: f64,
    pub games: u32,
    pub wins: u32,
}

impl Default for CivRating {
    fn default() -> Self {
        CivRating {
            rating: BASE_RATING,
            rd: BASE_RD,
            vol: BASE_VOL,
            games: 0,
            wins: 0,
        }
    }
}

/// A civ table needs this many games before its number outranks the
/// global rating for display and seating decisions.
pub const CIV_ELO_MIN_GAMES: u32 = 5;

#[derive(Clone, Serialize, Deserialize)]
pub struct Strategy {
    pub name: String,
    /// Player handle shown on leaderboards, themed after the strategy it
    /// plays (a science bot reads as one). Backfilled on load for leagues
    /// saved before usernames existed.
    #[serde(default)]
    pub username: String,
    pub kind: StrategyKind,
    pub rating: f64,
    pub rd: f64,
    pub vol: f64,
    pub games: u32,
    pub wins: u32,
    /// Per-civ rating tables (civ name -> Glicko state). Sparse: pairs only
    /// update in periods where they actually played — with a handful of
    /// games per round spread over many civs, growing every idle pair's
    /// deviation each round would pin them all at maximum uncertainty.
    #[serde(default)]
    pub civ_elo: BTreeMap<String, CivRating>,
    pub born_round: u32,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub retired: bool,
    /// Anchors are never retired; keeping fixed reference agents in every
    /// era pins the rating scale so numbers stay comparable across rounds.
    #[serde(default)]
    pub anchor: bool,
}

impl Strategy {
    fn new(name: &str, kind: StrategyKind, born_round: u32) -> Strategy {
        Strategy {
            name: name.to_string(),
            username: String::new(),
            kind,
            rating: BASE_RATING,
            rd: BASE_RD,
            vol: BASE_VOL,
            games: 0,
            wins: 0,
            civ_elo: BTreeMap::new(),
            born_round,
            parents: Vec::new(),
            retired: false,
            anchor: false,
        }
    }

    pub fn label(&self) -> String {
        match &self.kind {
            StrategyKind::Builtin { ai } => ai.clone(),
            StrategyKind::Advanced { target, .. } => match target {
                Some(lane) => format!("adv->{lane}"),
                None => "adv-genome".to_string(),
            },
        }
    }
}

/// Retired strategies stay in the roster (their history and lineage matter);
/// only active ones are scheduled.
#[derive(Serialize, Deserialize)]
pub struct League {
    pub round: u32,
    pub strategies: Vec<Strategy>,
}

impl League {
    pub fn active(&self) -> Vec<usize> {
        (0..self.strategies.len())
            .filter(|i| !self.strategies[*i].retired)
            .collect()
    }
}

pub struct LeagueCfg {
    /// Rating periods to play this invocation (state persists between runs).
    pub rounds: u32,
    pub games_per_round: u32,
    pub players_per_game: usize,
    pub width: i32,
    pub height: i32,
    pub max_turns: u32,
    pub num_city_states: usize,
    pub seed: u64,
    pub jobs: usize,
    pub dir: String,
    /// Breed and retire every this many rounds; 0 disables selection.
    pub evolve_every: u32,
    /// Active-roster cap that retirement trims back down to.
    pub max_pop: usize,
    pub verbose: bool,
}

impl Default for LeagueCfg {
    fn default() -> Self {
        let size = MapSize::for_players(4);
        LeagueCfg {
            rounds: 10,
            games_per_round: 16,
            players_per_game: 4,
            width: size.width,
            height: size.height,
            // Full natural length: at 150 the turn cap converts most games
            // into score victories, which structurally favors score-lane
            // strategies; at 250 every victory lane can actually fire.
            max_turns: 250,
            num_city_states: size.default_city_states,
            seed: 1,
            jobs: crate::parallel::default_jobs(),
            dir: "league".to_string(),
            evolve_every: 4,
            max_pop: 12,
            verbose: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Glicko-2 core (Glickman 2013, "Example of the Glicko-2 system").

#[derive(Clone, Copy)]
struct Glicko {
    mu: f64,
    phi: f64,
    sigma: f64,
}

fn to_internal(s: &Strategy) -> Glicko {
    Glicko {
        mu: (s.rating - BASE_RATING) / SCALE,
        phi: s.rd / SCALE,
        sigma: s.vol,
    }
}

fn g(phi: f64) -> f64 {
    1.0 / (1.0 + 3.0 * phi * phi / (std::f64::consts::PI * std::f64::consts::PI)).sqrt()
}

fn expect(mu: f64, mu_j: f64, phi_j: f64) -> f64 {
    1.0 / (1.0 + (-g(phi_j) * (mu - mu_j)).exp())
}

/// One rating period for one player. `results` are (opponent, score) with
/// opponents at their PRE-period values, score 1/0.5/0. Empty results = the
/// player sat out: rating stays, uncertainty grows (capped at the base RD so
/// a long-idle strategy never looks more unknown than a newborn).
fn rate(p: Glicko, results: &[(Glicko, f64)]) -> Glicko {
    if results.is_empty() {
        let phi = (p.phi * p.phi + p.sigma * p.sigma).sqrt();
        return Glicko {
            phi: phi.min(BASE_RD / SCALE),
            ..p
        };
    }
    let mut v_inv = 0.0;
    let mut d_sum = 0.0;
    for (o, score) in results {
        let gj = g(o.phi);
        let ej = expect(p.mu, o.mu, o.phi);
        v_inv += gj * gj * ej * (1.0 - ej);
        d_sum += gj * (score - ej);
    }
    let v = 1.0 / v_inv;
    let delta = v * d_sum;
    let (phi2, delta2) = (p.phi * p.phi, delta * delta);

    // New volatility: solve f(x)=0 by the paper's Illinois-style iteration.
    let a = (p.sigma * p.sigma).ln();
    let f = |x: f64| {
        let ex = x.exp();
        ex * (delta2 - phi2 - v - ex) / (2.0 * (phi2 + v + ex) * (phi2 + v + ex))
            - (x - a) / (TAU * TAU)
    };
    let mut lo = a;
    let mut hi = if delta2 > phi2 + v {
        (delta2 - phi2 - v).ln()
    } else {
        let mut k = 1.0;
        while f(a - k * TAU) < 0.0 {
            k += 1.0;
        }
        a - k * TAU
    };
    let mut flo = f(lo);
    let mut fhi = f(hi);
    while (hi - lo).abs() > 1e-6 {
        let mid = lo + (lo - hi) * flo / (fhi - flo);
        let fmid = f(mid);
        if fmid * fhi <= 0.0 {
            lo = hi;
            flo = fhi;
        } else {
            flo /= 2.0;
        }
        hi = mid;
        fhi = fmid;
    }
    let sigma = (lo / 2.0).exp();
    let phi_star = (phi2 + sigma * sigma).sqrt();
    let phi = 1.0 / (1.0 / (phi_star * phi_star) + 1.0 / v).sqrt();
    Glicko {
        mu: p.mu + phi * phi * d_sum,
        phi,
        sigma,
    }
}

fn apply_internal(s: &mut Strategy, g: Glicko) {
    s.rating = BASE_RATING + SCALE * g.mu;
    s.rd = SCALE * g.phi;
    s.vol = g.sigma;
}

// ---------------------------------------------------------------------------
// Seat -> Ai materialization.

fn make_ai(kind: &StrategyKind, seed: u64) -> Box<dyn Ai> {
    match kind {
        StrategyKind::Builtin { ai } => crate::elo::builtin_ai(ai, seed),
        StrategyKind::Advanced { weights, target } => {
            match target.as_deref().and_then(|t| t.parse::<VictoryTarget>().ok()) {
                Some(t) => Box::new(AdvancedAi::with_weights_and_target(weights.clone(), t)),
                None => Box::new(AdvancedAi::with_weights(weights.clone())),
            }
        }
    }
}

/// The genome a strategy contributes to breeding, if it has one. Built-in
/// advanced flavours breed from the weights they actually play with; agents
/// with no `Weights` genome (random, neural, ...) cannot be parents.
fn genome_of(kind: &StrategyKind) -> Option<Weights> {
    match kind {
        StrategyKind::Advanced { weights, .. } => Some(weights.clone()),
        StrategyKind::Builtin { ai } => match ai.as_str() {
            "advanced" | "advanced_v1" | "basic" => Some(Weights::default()),
            "advanced_evolved" | "evolved" => {
                Some(crate::evolve::load_champion("evolved").unwrap_or_default())
            }
            _ => None,
        },
    }
}

fn target_of(kind: &StrategyKind) -> Option<String> {
    match kind {
        StrategyKind::Advanced { target, .. } => target.clone(),
        StrategyKind::Builtin { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Player handles.

/// Handles per victory lane, so a username announces its strategy.
fn username_pool(lane: Option<&str>) -> &'static [&'static str] {
    match lane {
        Some("science") => &[
            "TechPriest", "LabRat", "BeakerBaron", "Eureka", "MoonshotMax", "QuantumLeap",
        ],
        Some("culture") => &[
            "CultureVulture", "OperaGhost", "PoetLaureate", "Wonderstruck", "TourismTycoon",
            "MuseTamer",
        ],
        Some("religious") => &[
            "ProphetMotive", "HolyRoller", "ZealotZed", "ApostlePaula", "FaithHealer",
            "TitheCollector",
        ],
        Some("diplomatic") => &[
            "SilverTongue", "Peacemonger", "Suzerain", "GrandBroker", "EnvoyElite",
            "CityStateFan",
        ],
        Some("domination") => &[
            "Warmonger", "SiegeLord", "BloodAndIron", "LegionLarry", "RaiderRex",
            "CapitalCollector",
        ],
        Some("score") => &[
            "PointHoarder", "ScoreKeeper", "TallyHo", "GrindKing", "MaxiMin", "NumbersNed",
        ],
        _ => &[
            "WildCard", "DarkHorse", "Maverick", "FreeSpirit", "Opportunist", "JackKnife",
        ],
    }
}

/// Founders keep fixed, recognizable handles across every league.
fn founder_username(name: &str) -> Option<&'static str> {
    Some(match name {
        "advanced" => "JackOfAllTrades",
        "basic" => "TrainingWheels",
        "advanced_v1" => "OldGuard",
        "evolved-champ" => "Darwin",
        "adv-science" => "TechPriest",
        "adv-culture" => "CultureVulture",
        "adv-religious" => "ProphetMotive",
        "adv-diplomatic" => "SilverTongue",
        "adv-domination" => "Warmonger",
        "adv-score" => "PointHoarder",
        _ => return None,
    })
}

fn unique_username(base: &str, taken: &std::collections::BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn lane_of(kind: &StrategyKind) -> Option<String> {
    target_of(kind)
}

/// Give every handle-less strategy a themed username. Founders get their
/// fixed handles; everyone else draws from their lane's pool, seeded by
/// their own name so backfill is deterministic whatever the roster order.
fn ensure_usernames(league: &mut League) {
    let mut taken: std::collections::BTreeSet<String> = league
        .strategies
        .iter()
        .map(|s| s.username.clone())
        .filter(|u| !u.is_empty())
        .collect();
    for i in 0..league.strategies.len() {
        if !league.strategies[i].username.is_empty() {
            continue;
        }
        let base = match founder_username(&league.strategies[i].name) {
            Some(handle) => handle.to_string(),
            None => {
                let seed = league.strategies[i]
                    .name
                    .bytes()
                    .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                        (h ^ b as u64).wrapping_mul(0x1_0000_0001_b3)
                    });
                let pool = username_pool(lane_of(&league.strategies[i].kind).as_deref());
                pool[Rng::new(seed).below(pool.len())].to_string()
            }
        };
        let handle = unique_username(&base, &taken);
        taken.insert(handle.clone());
        league.strategies[i].username = handle;
    }
}

// ---------------------------------------------------------------------------
// League lifecycle.

/// Founding roster: anchor reference agents, the six fixed victory lanes
/// (the "particular higher-level strategies" the league exists to compare),
/// and the GA champion if one has been evolved on this machine.
fn seed_league(dir: &str) -> League {
    let mut strategies = Vec::new();
    let mut builtin = |name: &str, ai: &str, anchor: bool| {
        let mut s = Strategy::new(
            name,
            StrategyKind::Builtin { ai: ai.to_string() },
            0,
        );
        s.anchor = anchor;
        strategies.push(s);
    };
    builtin("advanced", "advanced", true);
    builtin("basic", "basic", true);
    builtin("advanced_v1", "advanced_v1", false);
    for lane in VictoryTarget::ALL {
        strategies.push(Strategy::new(
            &format!("adv-{}", lane.as_str()),
            StrategyKind::Advanced {
                weights: Weights::default(),
                target: Some(lane.as_str().to_string()),
            },
            0,
        ));
    }
    if let Some(w) = crate::evolve::load_champion("evolved") {
        strategies.push(Strategy::new(
            "evolved-champ",
            StrategyKind::Advanced {
                weights: w,
                target: None,
            },
            0,
        ));
    }
    let mut league = League {
        round: 0,
        strategies,
    };
    ensure_usernames(&mut league);
    save_league(dir, &league);
    league
}

pub fn load_league(dir: &str) -> Option<League> {
    let raw = fs::read_to_string(Path::new(dir).join("league.json")).ok()?;
    let mut league: League = serde_json::from_str(&raw).ok()?;
    ensure_usernames(&mut league);
    Some(league)
}

/// Write via a temp file + rename so a crash mid-write cannot lose the roster.
pub fn save_league(dir: &str, league: &League) {
    let path = Path::new(dir);
    let _ = fs::create_dir_all(path);
    let tmp = path.join("league.json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(league).unwrap()).is_ok() {
        let _ = fs::rename(&tmp, path.join("league.json"));
    }
}

/// Per-round RNG derived from (seed, round) so a resumed league plays the
/// same schedule it would have played in one continuous run.
fn round_rng(seed: u64, round: u32) -> Rng {
    Rng::new(seed ^ 0x1EA6_0000 ^ (round as u64).wrapping_mul(0x9E37_79B9))
}

/// A round's tables: shuffle the active roster and deal it into tables of
/// `players_per_game`, repeating passes until `games_per_round` tables exist.
/// Everyone plays a near-equal amount and mixing is uniform; with rosters
/// this small (<=~16) proximity matchmaking would only slow convergence.
fn schedule(active: &[usize], cfg: &LeagueCfg, rng: &mut Rng) -> Vec<Vec<usize>> {
    assert!(!active.is_empty());
    let mut tables = Vec::new();
    let mut order: Vec<usize> = Vec::new();
    while tables.len() < cfg.games_per_round as usize {
        if order.len() < cfg.players_per_game {
            let mut pass = active.to_vec();
            for i in (1..pass.len()).rev() {
                pass.swap(i, rng.below(i + 1));
            }
            order.extend(pass);
        }
        let take = cfg.players_per_game.min(order.len());
        let mut table: Vec<usize> = order.drain(..take).collect();
        while table.len() < cfg.players_per_game {
            table.push(active[rng.below(active.len())]);
        }
        // A table of clones rates nobody; force a second strategy in.
        if active.len() > 1 && table.iter().all(|s| *s == table[0]) {
            let others: Vec<usize> = active.iter().copied().filter(|s| *s != table[0]).collect();
            let seat = rng.below(table.len());
            table[seat] = others[rng.below(others.len())];
        }
        tables.push(table);
    }
    tables
}

struct Outcome {
    /// Strategy indices, winner first then by score.
    placements: Vec<usize>,
    /// Civ each placement played, aligned with `placements`.
    civs: Vec<String>,
    seed: u64,
    turn: u32,
    victory: String,
}

fn play_round(league: &League, tables: &[Vec<usize>], cfg: &LeagueCfg, round: u32) -> Vec<Outcome> {
    let games = crate::parallel::map(tables.len(), cfg.jobs.max(1), |gi| {
        let table = &tables[gi];
        let seed = cfg
            .seed
            .wrapping_mul(1_000_003)
            .wrapping_add(round as u64 * 4096 + gi as u64);
        let mut game = Game::new(
            cfg.players_per_game,
            cfg.width,
            cfg.height,
            seed,
            cfg.max_turns,
            cfg.num_city_states,
        );
        let mut ais: Vec<Box<dyn Ai>> = game
            .players
            .iter()
            .map(|p| {
                if p.id < cfg.players_per_game {
                    make_ai(&league.strategies[table[p.id]].kind, seed + p.id as u64)
                } else {
                    crate::elo::builtin_ai("basic", seed + p.id as u64)
                }
            })
            .collect();
        run_game(&mut game, &mut ais);
        (seed, game)
    });
    games
        .into_iter()
        .enumerate()
        .map(|(gi, (seed, game))| {
            let winner = game.winner.unwrap();
            let mut ranked: Vec<usize> = (0..cfg.players_per_game).collect();
            ranked.sort_by_key(|pid| (*pid != winner, -game.score(*pid), *pid));
            Outcome {
                placements: ranked.iter().map(|pid| tables[gi][*pid]).collect(),
                civs: ranked
                    .iter()
                    .map(|pid| game.players[*pid].civ.clone())
                    .collect(),
                seed,
                turn: game.turn,
                victory: game.victory_type.clone().unwrap_or_default(),
            }
        })
        .collect()
}

/// One Glicko-2 rating period: every game becomes pairwise results against
/// opponents' pre-round ratings, then all active strategies update at once.
///
/// `age_idle` decides what happens to strategies that sat the period out. A
/// league round schedules the whole roster, so anyone missing really did idle
/// and their deviation should grow. A single recorded game is a period only
/// six seats could enter, so ageing the rest would pin the roster at maximum
/// uncertainty within an afternoon — the same reason civ tables are sparse.
fn apply_round(league: &mut League, outcomes: &[Outcome], age_idle: bool) {
    let pre: Vec<Glicko> = league.strategies.iter().map(to_internal).collect();
    let mut results: BTreeMap<usize, Vec<(Glicko, f64)>> = BTreeMap::new();
    let mut civ_results: BTreeMap<(usize, &str), Vec<(Glicko, f64)>> = BTreeMap::new();
    for outcome in outcomes {
        let p = &outcome.placements;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                if p[i] == p[j] {
                    continue; // a strategy cannot rate itself
                }
                results.entry(p[i]).or_default().push((pre[p[j]], 1.0));
                results.entry(p[j]).or_default().push((pre[p[i]], 0.0));
                civ_results
                    .entry((p[i], outcome.civs[i].as_str()))
                    .or_default()
                    .push((pre[p[j]], 1.0));
                civ_results
                    .entry((p[j], outcome.civs[j].as_str()))
                    .or_default()
                    .push((pre[p[i]], 0.0));
            }
        }
        for (rank, s) in p.iter().enumerate() {
            let strategy = &mut league.strategies[*s];
            strategy.games += 1;
            if rank == 0 {
                strategy.wins += 1;
            }
            let on_civ = strategy
                .civ_elo
                .entry(outcome.civs[rank].clone())
                .or_default();
            on_civ.games += 1;
            if rank == 0 {
                on_civ.wins += 1;
            }
        }
    }
    let civ_updates: Vec<((usize, String), Glicko)> = civ_results
        .into_iter()
        .map(|((si, civ), res)| {
            let cur = &league.strategies[si].civ_elo[civ];
            let state = Glicko {
                mu: (cur.rating - BASE_RATING) / SCALE,
                phi: cur.rd / SCALE,
                sigma: cur.vol,
            };
            ((si, civ.to_string()), rate(state, &res))
        })
        .collect();
    for ((si, civ), updated) in civ_updates {
        let on_civ = league.strategies[si].civ_elo.get_mut(&civ).unwrap();
        on_civ.rating = BASE_RATING + SCALE * updated.mu;
        on_civ.rd = SCALE * updated.phi;
        on_civ.vol = updated.sigma;
    }
    let empty = Vec::new();
    for i in 0..league.strategies.len() {
        if league.strategies[i].retired {
            continue;
        }
        let played = results.get(&i);
        if played.is_none() && !age_idle {
            continue;
        }
        let updated = rate(pre[i], played.unwrap_or(&empty));
        apply_internal(&mut league.strategies[i], updated);
    }
}

/// Rate one finished game as its own rating period and persist it, so a
/// server playing rated seats actually moves the table instead of showing a
/// snapshot forever.
///
/// `placements` is (strategy name, civ played) ordered winner first, then by
/// score. The roster is re-read from `dir` and seats are resolved by *name*
/// rather than by the index the caller seated from: a live server holds its
/// league in memory for the length of a game, and writing that stale copy
/// back would undo any result recorded in the meantime. Returns the updated
/// league, or `None` if the roster is unreadable or no longer holds every
/// name (a retired or renamed entrant leaves the game unrated rather than
/// rating the wrong strategy).
pub fn record_game(
    dir: &str,
    placements: &[(String, String)],
    seed: u64,
    turn: u32,
    victory: &str,
) -> Option<League> {
    if placements.len() < 2 {
        return None;
    }
    let mut league = load_league(dir)?;
    let seats: Option<Vec<usize>> = placements
        .iter()
        .map(|(name, _)| league.strategies.iter().position(|s| &s.name == name))
        .collect();
    let outcome = Outcome {
        placements: seats?,
        civs: placements.iter().map(|(_, civ)| civ.clone()).collect(),
        seed,
        turn,
        victory: victory.to_string(),
    };
    let names: Vec<String> = placements
        .iter()
        .map(|(name, civ)| format!("{name}@{civ}"))
        .collect();
    let round = league.round;
    apply_round(&mut league, &[outcome], false);
    league.round += 1;
    append_csv(
        dir,
        "matches.csv",
        "round,seed,turns,victory,placements",
        &[format!(
            "{round},{seed},{turn},{victory},{}",
            names.join("|")
        )],
    );
    let rating_lines: Vec<String> = placements
        .iter()
        .filter_map(|(name, _)| league.strategies.iter().find(|s| &s.name == name))
        .map(|s| {
            format!(
                "{},{},{:.1},{:.1},{:.4},{},{}",
                league.round, s.name, s.rating, s.rd, s.vol, s.games, s.wins
            )
        })
        .collect();
    append_csv(
        dir,
        "ratings.csv",
        "round,name,rating,rd,vol,games,wins",
        &rating_lines,
    );
    save_league(dir, &league);
    Some(league)
}

/// Selection: breed offspring from the top of the table, then retire the
/// confidently weakest until the active roster fits `max_pop` again. Anchors
/// and under-measured strategies are never retired.
fn evolve_league(league: &mut League, cfg: &LeagueCfg, rng: &mut Rng) -> (Vec<String>, Vec<String>) {
    let bounds = Weights::bounds();
    let mut parents: Vec<usize> = league
        .active()
        .into_iter()
        .filter(|i| genome_of(&league.strategies[*i].kind).is_some())
        .collect();
    parents.sort_by(|a, b| {
        league.strategies[*b]
            .rating
            .partial_cmp(&league.strategies[*a].rating)
            .unwrap()
    });
    let pool = (parents.len() / 2).max(1).min(parents.len());
    let mut born = Vec::new();
    if !parents.is_empty() {
        let births = (cfg.max_pop / 4).max(1);
        for _ in 0..births {
            let pa = parents[rng.below(pool)];
            let pb = parents[rng.below(pool)];
            let wa = genome_of(&league.strategies[pa].kind).unwrap();
            let wb = genome_of(&league.strategies[pb].kind).unwrap();
            let child = crate::evolve::mutate(
                &crate::evolve::crossover(&wa, &wb, rng),
                rng,
                &bounds,
            );
            // Lane inheritance: mostly keep a parent's victory lane so lane
            // identity persists under refinement; sometimes explore.
            let target = if rng.chance(0.6) {
                target_of(&league.strategies[pa].kind)
            } else if rng.chance(0.5) {
                target_of(&league.strategies[pb].kind)
            } else if rng.chance(0.5) {
                Some(
                    VictoryTarget::ALL[rng.below(VictoryTarget::ALL.len())]
                        .as_str()
                        .to_string(),
                )
            } else {
                None
            };
            let name = format!("g{}-{}", league.round, league.strategies.len());
            let kind = StrategyKind::Advanced {
                weights: child,
                target,
            };
            let taken: std::collections::BTreeSet<String> = league
                .strategies
                .iter()
                .map(|s| s.username.clone())
                .collect();
            let pool = username_pool(lane_of(&kind).as_deref());
            let handle = unique_username(pool[rng.below(pool.len())], &taken);
            let mut s = Strategy::new(&name, kind, league.round);
            s.username = handle.clone();
            s.parents = vec![
                league.strategies[pa].name.clone(),
                league.strategies[pb].name.clone(),
            ];
            born.push(handle);
            league.strategies.push(s);
        }
    }
    let mut retired = Vec::new();
    loop {
        let active = league.active();
        if active.len() <= cfg.max_pop {
            break;
        }
        let candidate = active
            .into_iter()
            .filter(|i| {
                let s = &league.strategies[*i];
                !s.anchor && s.games >= MIN_GAMES_TO_RETIRE && s.rd <= MAX_RD_TO_RETIRE
            })
            .min_by(|a, b| {
                league.strategies[*a]
                    .rating
                    .partial_cmp(&league.strategies[*b].rating)
                    .unwrap()
            });
        match candidate {
            Some(i) => {
                league.strategies[i].retired = true;
                retired.push(league.strategies[i].username.clone());
            }
            None => break, // nobody is confidently weak yet; keep the crowd
        }
    }
    (born, retired)
}

/// The rating to show (and seat by) for a strategy on a given civ: the
/// civ table once it has evidence, else the global one.
/// Returns (rating, rd, is_civ_specific).
pub fn display_elo(s: &Strategy, civ: &str) -> (f64, f64, bool) {
    match s.civ_elo.get(civ) {
        Some(c) if c.games >= CIV_ELO_MIN_GAMES => (c.rating, c.rd, true),
        _ => (s.rating, s.rd, false),
    }
}

/// Seat a table whose civs are already known (civs are fixed per seat in
/// `Game::new`): each seat takes the strongest still-unused active strategy
/// *for its civ*, so different civs field different specialists. Reuses
/// strategies only when the roster is smaller than the table.
pub fn seat_by_civ(league: &League, civs: &[String]) -> Vec<usize> {
    let active = league.active();
    assert!(!active.is_empty(), "league has no active strategies");
    let mut used: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    civs.iter()
        .map(|civ| {
            let fresh = active.iter().copied().filter(|i| !used.contains(i));
            let pool: Vec<usize> = if used.len() < active.len() {
                fresh.collect()
            } else {
                active.clone()
            };
            let pick = pool
                .into_iter()
                .max_by(|a, b| {
                    let ea = display_elo(&league.strategies[*a], civ).0;
                    let eb = display_elo(&league.strategies[*b], civ).0;
                    ea.partial_cmp(&eb).unwrap().then(b.cmp(a))
                })
                .unwrap();
            used.insert(pick);
            pick
        })
        .collect()
}

/// Materialize a strategy as a `Send` AI for the game server's fleet.
pub fn make_send_ai(kind: &StrategyKind, seed: u64) -> Box<dyn Ai + Send> {
    match kind {
        StrategyKind::Builtin { ai } => match ai.as_str() {
            "basic" => Box::new(crate::ai::BasicAi::new()),
            "advanced_v1" => Box::new(AdvancedAi::legacy()),
            "random" => Box::new(crate::ai::RandomAi::new(seed)),
            "advanced_evolved" | "evolved" => Box::new(
                crate::evolve::load_champion("evolved")
                    .map(AdvancedAi::with_weights)
                    .unwrap_or_else(AdvancedAi::new),
            ),
            _ => Box::new(AdvancedAi::new()),
        },
        StrategyKind::Advanced { weights, target } => {
            match target.as_deref().and_then(|t| t.parse::<VictoryTarget>().ok()) {
                Some(t) => Box::new(AdvancedAi::with_weights_and_target(weights.clone(), t)),
                None => Box::new(AdvancedAi::with_weights(weights.clone())),
            }
        }
    }
}

/// One civ's leaderboard: who plays this civ best, by its civ table.
pub fn civ_standings(league: &League, civ: &str) -> String {
    let mut rows: Vec<(&Strategy, &CivRating)> = league
        .strategies
        .iter()
        .filter_map(|s| s.civ_elo.get(civ).map(|c| (s, c)))
        .filter(|(_, c)| c.games > 0)
        .collect();
    if rows.is_empty() {
        return format!("no rated games for {civ} yet\n");
    }
    rows.sort_by(|a, b| b.1.rating.partial_cmp(&a.1.rating).unwrap());
    let mut out = format!("{civ} leaderboard (round {}):\n", league.round);
    for (rank, (s, c)) in rows.iter().enumerate() {
        out.push_str(&format!(
            "  {:>2}. {:<18} {:6.0} elo ±{:<4.0} games={:<4} wins={:<3} winrate={:3.0}%  {:<14}{}{}\n",
            rank + 1,
            s.username,
            c.rating,
            c.rd,
            c.games,
            c.wins,
            100.0 * c.wins as f64 / c.games.max(1) as f64,
            s.label(),
            if c.games < CIV_ELO_MIN_GAMES {
                "  provisional"
            } else {
                ""
            },
            if s.retired { "  (retired)" } else { "" },
        ));
    }
    out
}

/// Every civ's current champion strategy, one line per civ.
pub fn civ_summary(league: &League) -> String {
    let mut civs: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    for s in &league.strategies {
        civs.extend(s.civ_elo.keys());
    }
    if civs.is_empty() {
        return "no per-civ ratings yet (play some rounds first)\n".to_string();
    }
    let mut out = format!("Best player per civ (round {}):\n", league.round);
    for civ in civs {
        let best = league
            .strategies
            .iter()
            .filter(|s| !s.retired)
            .filter_map(|s| {
                s.civ_elo
                    .get(civ)
                    .filter(|c| c.games >= CIV_ELO_MIN_GAMES)
                    .map(|c| (s, c))
            })
            .max_by(|a, b| a.1.rating.partial_cmp(&b.1.rating).unwrap());
        match best {
            Some((s, c)) => out.push_str(&format!(
                "  {:<10} {:<18} {:6.0} elo ±{:<4.0} ({} games, {:.0}% wins, {})\n",
                civ,
                s.username,
                c.rating,
                c.rd,
                c.games,
                100.0 * c.wins as f64 / c.games.max(1) as f64,
                s.label(),
            )),
            None => out.push_str(&format!("  {civ:<10} (no settled rating yet)\n")),
        }
    }
    out
}

fn append_csv(dir: &str, file: &str, header: &str, lines: &[String]) {
    let path = Path::new(dir).join(file);
    let fresh = !path.exists();
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        if fresh {
            let _ = writeln!(f, "{header}");
        }
        for line in lines {
            let _ = writeln!(f, "{line}");
        }
    }
}

pub fn standings(league: &League) -> String {
    let mut order: Vec<&Strategy> = league.strategies.iter().collect();
    order.sort_by(|a, b| {
        a.retired
            .cmp(&b.retired)
            .then(b.rating.partial_cmp(&a.rating).unwrap())
    });
    let mut out = format!("League players after round {}:\n", league.round);
    for (rank, s) in order.iter().enumerate() {
        let status = if s.retired {
            "retired"
        } else if s.anchor {
            "anchor"
        } else {
            "active"
        };
        out.push_str(&format!(
            "  {:>2}. {:<18} {:6.0} elo ±{:<4.0} {:<14} games={:<5} wins={:<4} winrate={:3.0}%  born r{:<3} {:<7} [{}]\n",
            rank + 1,
            s.username,
            s.rating,
            s.rd,
            s.label(),
            s.games,
            s.wins,
            100.0 * s.wins as f64 / s.games.max(1) as f64,
            s.born_round,
            status,
            s.name,
        ));
    }
    out
}

pub fn run_league(cfg: &LeagueCfg) -> League {
    let mut league = load_league(&cfg.dir).unwrap_or_else(|| seed_league(&cfg.dir));
    for _ in 0..cfg.rounds {
        let round = league.round;
        let mut rng = round_rng(cfg.seed, round);
        let active = league.active();
        let tables = schedule(&active, cfg, &mut rng);
        let outcomes = play_round(&league, &tables, cfg, round);
        let match_lines: Vec<String> = outcomes
            .iter()
            .map(|o| {
                let names: Vec<String> = o
                    .placements
                    .iter()
                    .zip(&o.civs)
                    .map(|(s, civ)| format!("{}@{civ}", league.strategies[*s].name))
                    .collect();
                format!(
                    "{round},{},{},{},{}",
                    o.seed,
                    o.turn,
                    o.victory,
                    names.join("|")
                )
            })
            .collect();
        apply_round(&mut league, &outcomes, true);
        league.round += 1;
        let mut news = (Vec::new(), Vec::new());
        if cfg.evolve_every > 0 && league.round % cfg.evolve_every == 0 {
            news = evolve_league(&mut league, cfg, &mut rng);
        }
        let rating_lines: Vec<String> = league
            .active()
            .into_iter()
            .map(|i| {
                let s = &league.strategies[i];
                format!(
                    "{},{},{:.1},{:.1},{:.4},{},{}",
                    league.round, s.name, s.rating, s.rd, s.vol, s.games, s.wins
                )
            })
            .collect();
        append_csv(
            &cfg.dir,
            "matches.csv",
            "round,seed,turns,victory,placements",
            &match_lines,
        );
        append_csv(
            &cfg.dir,
            "ratings.csv",
            "round,name,rating,rd,vol,games,wins",
            &rating_lines,
        );
        save_league(&cfg.dir, &league);
        if cfg.verbose {
            let leader = league
                .active()
                .into_iter()
                .max_by(|a, b| {
                    league.strategies[*a]
                        .rating
                        .partial_cmp(&league.strategies[*b].rating)
                        .unwrap()
                })
                .unwrap();
            println!(
                "round {:>3}: {} games; leader {} {:.1} ±{:.1}{}{}",
                round,
                outcomes.len(),
                league.strategies[leader].username,
                league.strategies[leader].rating,
                league.strategies[leader].rd,
                if news.0.is_empty() {
                    String::new()
                } else {
                    format!("; born {:?}", news.0)
                },
                if news.1.is_empty() {
                    String::new()
                } else {
                    format!("; retired {:?}", news.1)
                },
            );
        }
    }
    if cfg.verbose {
        println!();
        print!("{}", standings(&league));
    }
    league
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from Glickman's Glicko-2 paper: 1500/200/0.06
    /// beating 1400/30 then losing to 1550/100 and 1700/300 in one period
    /// must land on 1464.06 / 151.52 / 0.05999.
    #[test]
    fn glicko2_matches_glickman_paper_example() {
        let player = Glicko {
            mu: 0.0,
            phi: 200.0 / SCALE,
            sigma: 0.06,
        };
        let opponent = |r: f64, rd: f64| Glicko {
            mu: (r - 1500.0) / SCALE,
            phi: rd / SCALE,
            sigma: 0.06,
        };
        let results = vec![
            (opponent(1400.0, 30.0), 1.0),
            (opponent(1550.0, 100.0), 0.0),
            (opponent(1700.0, 300.0), 0.0),
        ];
        let out = rate(player, &results);
        let rating = 1500.0 + SCALE * out.mu;
        let rd = SCALE * out.phi;
        assert!((rating - 1464.06).abs() < 0.1, "rating {rating}");
        assert!((rd - 151.52).abs() < 0.1, "rd {rd}");
        assert!((out.sigma - 0.05999).abs() < 0.0002, "vol {}", out.sigma);
    }

    #[test]
    fn idle_period_grows_uncertainty_but_not_rating() {
        let player = Glicko {
            mu: 0.5,
            phi: 80.0 / SCALE,
            sigma: 0.06,
        };
        let out = rate(player, &[]);
        assert_eq!(out.mu, 0.5);
        assert!(out.phi > player.phi);
        assert!(out.phi <= BASE_RD / SCALE);
    }

    #[test]
    fn winners_gain_and_losers_lose() {
        let mut league = League {
            round: 0,
            strategies: vec![
                Strategy::new("a", StrategyKind::Builtin { ai: "basic".into() }, 0),
                Strategy::new("b", StrategyKind::Builtin { ai: "basic".into() }, 0),
            ],
        };
        let outcomes = vec![Outcome {
            placements: vec![0, 1],
            civs: vec!["Rome".into(), "Egypt".into()],
            seed: 0,
            turn: 10,
            victory: "score".into(),
        }];
        apply_round(&mut league, &outcomes, true);
        assert!(league.strategies[0].rating > BASE_RATING);
        assert!(league.strategies[1].rating < BASE_RATING);
        assert_eq!(league.strategies[0].wins, 1);
        assert_eq!(league.strategies[0].games, 1);
        // the same result also lands on each side's civ table
        let rome = &league.strategies[0].civ_elo["Rome"];
        let egypt = &league.strategies[1].civ_elo["Egypt"];
        assert!(rome.rating > BASE_RATING && rome.games == 1 && rome.wins == 1);
        assert!(egypt.rating < BASE_RATING && egypt.games == 1 && egypt.wins == 0);
        assert!(league.strategies[0].civ_elo.get("Egypt").is_none());
    }

    /// A finished game rated on its own moves only the strategies that
    /// played it. Ageing the rest would be right for a league round, which
    /// schedules everyone, but a six-seat game is not an idle period for the
    /// twenty strategies that could never have entered it.
    #[test]
    fn a_single_recorded_game_leaves_absent_strategies_alone() {
        let builtin = |ai: &str| StrategyKind::Builtin { ai: ai.into() };
        let mut league = League {
            round: 7,
            strategies: vec![
                Strategy::new("a", builtin("advanced"), 0),
                Strategy::new("b", builtin("basic"), 0),
                Strategy::new("bench", builtin("random"), 0),
            ],
        };
        let bench_before = (league.strategies[2].rating, league.strategies[2].rd);
        let outcomes = vec![Outcome {
            placements: vec![0, 1],
            civs: vec!["Rome".into(), "Egypt".into()],
            seed: 3,
            turn: 90,
            victory: "science".into(),
        }];
        apply_round(&mut league, &outcomes, false);
        assert!(league.strategies[0].rating > BASE_RATING);
        assert!(league.strategies[1].rating < BASE_RATING);
        let bench = &league.strategies[2];
        assert_eq!((bench.rating, bench.rd), bench_before);
        assert_eq!(bench.games, 0);
    }

    /// `record_game` is the live server's whole path to a moving table: it
    /// must persist, keep counting across games, and rate by name so a
    /// roster that changed under a long game is not overwritten with a
    /// stale one.
    #[test]
    fn recording_a_game_persists_and_accumulates() {
        let dir = std::env::temp_dir().join(format!(
            "civvis-league-record-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let dir = dir.to_str().unwrap();
        let _ = fs::remove_dir_all(dir);
        let builtin = |ai: &str| StrategyKind::Builtin { ai: ai.into() };
        let mut seeded = League {
            round: 12,
            strategies: vec![
                Strategy::new("a", builtin("advanced"), 0),
                Strategy::new("b", builtin("basic"), 0),
            ],
        };
        seeded.strategies[1].rating = 1600.0;
        save_league(dir, &seeded);

        let placements = vec![
            ("a".to_string(), "Rome".to_string()),
            ("b".to_string(), "Egypt".to_string()),
        ];
        let first = record_game(dir, &placements, 5, 120, "culture").expect("rated");
        assert_eq!(first.round, 13);
        assert!(first.strategies[0].rating > BASE_RATING);
        assert!(first.strategies[1].rating < 1600.0);
        assert_eq!(first.strategies[0].civ_elo["Rome"].wins, 1);

        // Reloaded from disk, not from the caller's copy.
        let second = record_game(dir, &placements, 6, 130, "culture").expect("rated");
        assert_eq!(second.round, 14);
        assert_eq!(second.strategies[0].games, 2);
        assert!(second.strategies[0].rating > first.strategies[0].rating);
        assert_eq!(
            load_league(dir).unwrap().strategies[0].rating,
            second.strategies[0].rating
        );
        let matches = fs::read_to_string(Path::new(dir).join("matches.csv")).unwrap();
        assert_eq!(matches.lines().count(), 3, "header plus one row per game");
        assert!(matches.contains("a@Rome|b@Egypt"));

        // A name the roster no longer carries leaves the table untouched.
        let unknown = vec![
            ("a".to_string(), "Rome".to_string()),
            ("ghost".to_string(), "Egypt".to_string()),
        ];
        assert!(record_game(dir, &unknown, 7, 140, "score").is_none());
        assert_eq!(load_league(dir).unwrap().round, 14);
        let _ = fs::remove_dir_all(dir);
    }

    /// Seating by civ prefers each civ's settled specialist and never
    /// doubles a strategy up while unused ones remain.
    #[test]
    fn seat_by_civ_prefers_civ_specialists() {
        let mut league = League {
            round: 0,
            strategies: vec![
                Strategy::new("gen", StrategyKind::Builtin { ai: "advanced".into() }, 0),
                Strategy::new(
                    "rome-expert",
                    StrategyKind::Advanced {
                        weights: Weights::default(),
                        target: Some("domination".into()),
                    },
                    0,
                ),
            ],
        };
        league.strategies[0].rating = 1650.0; // globally stronger
        league.strategies[1].rating = 1450.0;
        league.strategies[1].civ_elo.insert(
            "Rome".into(),
            CivRating {
                rating: 1750.0,
                games: CIV_ELO_MIN_GAMES,
                ..CivRating::default()
            },
        );
        let seats = seat_by_civ(&league, &["Rome".into(), "Egypt".into()]);
        assert_eq!(seats, vec![1, 0], "Rome goes to its specialist");
        let (elo, _, civ_specific) = display_elo(&league.strategies[1], "Rome");
        assert!(civ_specific && (elo - 1750.0).abs() < 1e-9);
        // below the evidence bar the global rating stands in
        let (elo, _, civ_specific) = display_elo(&league.strategies[1], "Egypt");
        assert!(!civ_specific && (elo - 1450.0).abs() < 1e-9);
    }

    #[test]
    fn schedule_covers_roster_and_fills_tables() {
        let cfg = LeagueCfg {
            games_per_round: 6,
            players_per_game: 4,
            ..LeagueCfg::default()
        };
        let active: Vec<usize> = (0..9).collect();
        let mut rng = Rng::new(7);
        let tables = schedule(&active, &cfg, &mut rng);
        assert_eq!(tables.len(), 6);
        let mut seen = std::collections::BTreeSet::new();
        for t in &tables {
            assert_eq!(t.len(), 4);
            assert!(t.iter().any(|s| *s != t[0]), "table of clones");
            seen.extend(t.iter().copied());
        }
        // two dealt passes over 9 strategies fill 24 seats: everyone plays
        assert_eq!(seen.len(), 9);
    }

    #[test]
    fn selection_breeds_from_leaders_and_retires_confident_losers() {
        let mut league = League {
            round: 8,
            strategies: Vec::new(),
        };
        for i in 0..6 {
            let mut s = Strategy::new(
                &format!("s{i}"),
                StrategyKind::Advanced {
                    weights: Weights::default(),
                    target: None,
                },
                0,
            );
            s.rating = 1600.0 - 40.0 * i as f64;
            s.rd = 60.0;
            s.games = 30;
            league.strategies.push(s);
        }
        league.strategies[0].anchor = true;
        // an under-measured newcomer that must survive despite a bad rating
        let mut newborn = Strategy::new(
            "newborn",
            StrategyKind::Advanced {
                weights: Weights::default(),
                target: None,
            },
            7,
        );
        newborn.rating = 1200.0;
        newborn.rd = 300.0;
        newborn.games = 3;
        league.strategies.push(newborn);

        let cfg = LeagueCfg {
            max_pop: 7,
            ..LeagueCfg::default()
        };
        ensure_usernames(&mut league);
        let handle = |league: &League, name: &str| {
            league
                .strategies
                .iter()
                .find(|s| s.name == name)
                .unwrap()
                .username
                .clone()
        };
        let newborn_handle = handle(&league, "newborn");
        let anchor_handle = handle(&league, "s0");
        let mut rng = Rng::new(3);
        let (born, retired) = evolve_league(&mut league, &cfg, &mut rng);
        assert!(!born.is_empty());
        assert!(!retired.contains(&newborn_handle));
        assert!(!retired.contains(&anchor_handle), "anchor retired");
        // offspring exist, are active, carry lineage, and have a handle
        let child = league
            .strategies
            .iter()
            .find(|s| born.contains(&s.username))
            .unwrap();
        assert!(!child.username.is_empty());
        assert_eq!(child.parents.len(), 2);
        assert!(!child.retired);
        assert_eq!(child.rd, BASE_RD);
        // roster trimmed back to cap (retirees had games and low rd)
        assert!(league.active().len() <= cfg.max_pop.max(7));
    }

    /// Usernames are themed to the lane, unique, stable for founders, and
    /// deterministically backfilled onto rosters saved before the field
    /// existed (the same league always regrows the same handles).
    #[test]
    fn usernames_are_themed_unique_and_deterministic() {
        let mut league = League {
            round: 0,
            strategies: vec![
                Strategy::new("advanced", StrategyKind::Builtin { ai: "advanced".into() }, 0),
                Strategy::new(
                    "adv-science",
                    StrategyKind::Advanced {
                        weights: Weights::default(),
                        target: Some("science".into()),
                    },
                    0,
                ),
                Strategy::new(
                    "g4-9",
                    StrategyKind::Advanced {
                        weights: Weights::default(),
                        target: Some("science".into()),
                    },
                    4,
                ),
                Strategy::new(
                    "g4-10",
                    StrategyKind::Advanced {
                        weights: Weights::default(),
                        target: Some("domination".into()),
                    },
                    4,
                ),
            ],
        };
        ensure_usernames(&mut league);
        assert_eq!(league.strategies[0].username, "JackOfAllTrades");
        assert_eq!(league.strategies[1].username, "TechPriest");
        assert!(username_pool(Some("science"))
            .iter()
            .any(|p| league.strategies[2].username.starts_with(p)));
        assert!(username_pool(Some("domination"))
            .iter()
            .any(|p| league.strategies[3].username.starts_with(p)));
        let handles: std::collections::BTreeSet<&String> =
            league.strategies.iter().map(|s| &s.username).collect();
        assert_eq!(handles.len(), league.strategies.len(), "handle collision");
        // backfill is a pure function of names: rerunning changes nothing
        let before: Vec<String> = league.strategies.iter().map(|s| s.username.clone()).collect();
        ensure_usernames(&mut league);
        let after: Vec<String> = league.strategies.iter().map(|s| s.username.clone()).collect();
        assert_eq!(before, after);
        // the leaderboard lists every player's handle with elo next to it
        let table = standings(&league);
        assert!(table.contains("TechPriest"));
        assert!(table.contains("1500 elo"));
    }

    /// Same seed, fresh dirs -> byte-identical league state, so `--jobs`
    /// and reruns cannot change ratings.
    #[test]
    fn league_runs_are_deterministic() {
        let base = std::env::temp_dir().join(format!("civvis-league-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let run = |sub: &str, jobs: usize| {
            let cfg = LeagueCfg {
                rounds: 2,
                games_per_round: 3,
                players_per_game: 2,
                width: 20,
                height: 14,
                max_turns: 25,
                num_city_states: 1,
                seed: 11,
                jobs,
                dir: base.join(sub).to_string_lossy().into_owned(),
                evolve_every: 2,
                max_pop: 6,
                verbose: false,
            };
            let league = run_league(&cfg);
            serde_json::to_string(&league).unwrap()
        };
        let a = run("a", 1);
        let b = run("b", 4);
        assert_eq!(a, b);
        let _ = fs::remove_dir_all(&base);
    }
}
