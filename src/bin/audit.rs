//! Plays full games and reports what looked wrong while they ran.
//!
//! `soak` answers "did the game finish"; this answers "was the game any
//! good". It walks every turn of a live game and records both hard invariant
//! breaks (state the rules should never produce) and soft symptoms - idle
//! units, cities producing nothing, treasuries nobody spends - which are the
//! shape most engine and AI defects actually take from the outside.
//!
//! Usage: audit [--games N] [--start-seed N] [--players N] [--turns N]
//!              [--width N] [--height N] [--city-states N] [--quiet]
use std::collections::BTreeMap;

use civvis::ai::{AdvancedAi, Ai};
use civvis::game::{Action, Game};
use civvis::setup::MapSize;

fn number(args: &[String], flag: &str, default: i64) -> i64 {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// How long a unit or city may sit doing nothing before it is worth reporting.
const IDLE_TURNS: u32 = 25;

#[derive(Default)]
struct Findings {
    /// Rules the engine broke, keyed by a short signature so one recurring
    /// fault reports as one line with a count rather than thousands.
    violations: BTreeMap<String, (usize, String)>,
    /// Symptoms that are legal but suggest something is not working.
    symptoms: BTreeMap<String, (usize, String)>,
}

impl Findings {
    fn violation(&mut self, key: impl Into<String>, detail: impl Into<String>) {
        let entry = self
            .violations
            .entry(key.into())
            .or_insert_with(|| (0, detail.into()));
        entry.0 += 1;
    }

    fn symptom(&mut self, key: impl Into<String>, detail: impl Into<String>) {
        let entry = self
            .symptoms
            .entry(key.into())
            .or_insert_with(|| (0, detail.into()));
        entry.0 += 1;
    }
}

/// State that only means something across turns, so it cannot be judged from
/// a single snapshot.
#[derive(Default)]
struct History {
    unit_still_since: BTreeMap<u32, (u32, civvis::Pos)>,
    city_idle_since: BTreeMap<u32, u32>,
    reported_unit: BTreeMap<u32, bool>,
    reported_city: BTreeMap<u32, bool>,
}

fn stalled_settler_context(g: &Game, id: u32) -> String {
    let unit = &g.units[&id];
    let pid = unit.owner;
    let legal_sites: Vec<_> = g
        .map
        .tiles
        .iter()
        .filter(|(position, tile)| {
            !g.rules.is_water(tile)
                && g.rules.is_passable(tile)
                && !g
                    .cities
                    .values()
                    .any(|city| g.wdist(city.pos, **position) < 4)
                && tile
                    .owner_city
                    .is_none_or(|city| g.cities[&city].owner == pid)
        })
        .map(|(position, _)| *position)
        .collect();
    let reachable = legal_sites
        .iter()
        .filter(|position| **position == unit.pos || g.route_step(id, **position, 0).is_some())
        .count();
    format!(
        "; at {:?}, cities={}, legal_sites={}, reachable={}, shipbuilding={}, linked={:?}",
        unit.pos,
        g.player_city_ids(pid).len(),
        legal_sites.len(),
        reachable,
        g.players[pid].techs.contains("shipbuilding"),
        unit.linked_to,
    )
}

fn audit_turn(g: &Game, history: &mut History, found: &mut Findings) {
    for (id, unit) in &g.units {
        if unit.hp <= 0 || unit.hp > 100 {
            found.violation(
                "unit hp out of range",
                format!("unit {id} ({}) at hp {}", unit.kind, unit.hp),
            );
        }
        if unit.moves_left < -f64::EPSILON {
            found.violation(
                "negative movement",
                format!("unit {id} ({}) at {} MP", unit.kind, unit.moves_left),
            );
        }
        if g.map.get(unit.pos).is_none() {
            found.violation(
                "unit off the map",
                format!("unit {id} ({}) at {:?}", unit.kind, unit.pos),
            );
        }
        if !g.players[unit.owner].alive {
            found.violation(
                "unit outlives its owner",
                format!("unit {id} ({}) owned by {}", unit.kind, unit.owner),
            );
        }

        // A unit that never moves is usually a pathing or target-selection
        // dead end rather than a deliberate garrison, so only flag the ones
        // that are not fortified in place.
        let entry = history
            .unit_still_since
            .entry(*id)
            .or_insert((g.turn, unit.pos));
        if entry.1 != unit.pos {
            *entry = (g.turn, unit.pos);
        } else if g.turn - entry.0 >= IDLE_TURNS
            && !unit.fortified
            && !history.reported_unit.get(id).copied().unwrap_or(false)
        {
            history.reported_unit.insert(*id, true);
            let context = if unit.kind == "settler" {
                stalled_settler_context(g, *id)
            } else {
                String::new()
            };
            found.symptom(
                format!("{} sits still {IDLE_TURNS}+ turns", unit.kind),
                format!(
                    "unit {id} ({}) of {} unmoved since turn {}{context}",
                    unit.kind, g.players[unit.owner].civ, entry.0,
                ),
            );
        }
    }

    for (id, city) in &g.cities {
        if city.pop < 1 {
            found.violation("city below one Citizen", format!("city {id} ({})", city.name));
        }
        if !g.players[city.owner].alive {
            found.violation(
                "city outlives its owner",
                format!("city {id} ({}) owned by {}", city.name, city.owner),
            );
        }
        if city.loyalty < -f64::EPSILON || city.loyalty > 100.0 + f64::EPSILON {
            found.violation(
                "loyalty out of range",
                format!("city {id} ({}) at {} Loyalty", city.name, city.loyalty),
            );
        }
        if city.hp <= 0 {
            found.violation(
                "city at zero HP",
                format!("city {id} ({}) still standing", city.name),
            );
        }
        let max_walls = g.city_max_wall_hp(city);
        if city.wall_hp > max_walls {
            found.violation(
                "walls above their pool",
                format!("city {id} ({}) at {}/{max_walls}", city.name, city.wall_hp),
            );
        }
        let total: f64 = city.pressure.values().sum::<f64>() + city.atheist_pressure;
        if total <= 0.0 {
            found.violation(
                "city with no religious standing at all",
                format!("city {id} ({})", city.name),
            );
        }

        // An empty queue is a city converting Production into nothing.
        if city.queue.is_empty() {
            let since = history.city_idle_since.entry(*id).or_insert(g.turn);
            if g.turn - *since >= IDLE_TURNS
                && !history.reported_city.get(id).copied().unwrap_or(false)
            {
                history.reported_city.insert(*id, true);
                found.symptom(
                    "city builds nothing for 25+ turns",
                    format!(
                        "city {id} ({}) of {} idle since turn {since}",
                        city.name, g.players[city.owner].civ
                    ),
                );
            }
        } else {
            history.city_idle_since.remove(id);
        }
    }

    for player in &g.players {
        if player.is_barbarian {
            continue;
        }
        if player.alive && g.player_city_ids(player.id).is_empty() {
            let has_settler = g
                .player_unit_ids(player.id)
                .into_iter()
                .any(|unit| g.units[&unit].kind == "settler");
            if !has_settler {
                found.violation(
                    "player alive with no cities and no settler",
                    format!("player {} ({})", player.id, player.civ),
                );
            }
        }
        if player.gold < -f64::EPSILON {
            found.violation(
                "treasury below zero",
                format!("player {} ({}) at {}", player.id, player.civ, player.gold),
            );
        }
    }
}

/// End-of-game checks: things that are only wrong once the game is over.
fn audit_result(g: &Game, found: &mut Findings) {
    let Some(winner) = g.winner else {
        found.violation("game ended with no winner", String::new());
        return;
    };
    if g.players[winner].is_minor {
        found.violation(
            "a minor won the game",
            format!("{} took the game", g.players[winner].civ),
        );
    }
    if g.victory_type.is_none() {
        found.violation("winner with no victory type", String::new());
    }

    // The war log is only readable if a war is one entry. Two shipped rules
    // hold that: a war runs ten turns before it can be settled, and the peace
    // binds for ten more. A record that breaks either means the log is filling
    // with fragments of the same war again.
    let mut previous: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for war in &g.concluded_wars {
        let key = (war.aggressor.min(war.defender), war.aggressor.max(war.defender));
        let Some(ended) = war.ended else { continue };
        let sides = (
            g.players[war.aggressor].civ.clone(),
            g.players[war.defender].civ.clone(),
        );
        if ended.saturating_sub(war.started) < 10 {
            found.violation(
                "a war ended before the shipped minimum",
                format!(
                    "{} against {} ran turns {}-{ended}",
                    sides.0, sides.1, war.started
                ),
            );
        }
        if let Some(last) = previous.insert(key, ended) {
            if war.started.saturating_sub(last) < 10 {
                found.violation(
                    "the same pair re-declared inside the peace treaty",
                    format!(
                        "{} against {} again on turn {} after peace on turn {last}",
                        sides.0, sides.1, war.started
                    ),
                );
            }
        }
    }

    for player in &g.players {
        if player.is_barbarian || !player.alive {
            continue;
        }
        let cities = g.player_city_ids(player.id).len();
        // Treasury nobody ever spends is the signature of an AI that has run
        // out of things it knows how to buy.
        if player.gold > 1_000.0 {
            found.symptom(
                if player.is_minor {
                    "city-state hoards Gold it never spends"
                } else {
                    "civilization hoards Gold it never spends"
                },
                format!(
                    "{} finished on {:.0} Gold with {cities} cities",
                    player.civ, player.gold
                ),
            );
        }
        if !player.is_minor && player.techs.len() <= 2 {
            found.symptom(
                "civilization researched almost nothing",
                format!("{} ended on {} techs", player.civ, player.techs.len()),
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let games = number(&args, "--games", 3);
    let start = number(&args, "--start-seed", 0);
    let players = number(&args, "--players", 8).max(1);
    let size = MapSize::for_players(players as usize);
    let width = number(&args, "--width", size.width as i64) as i32;
    let height = number(&args, "--height", size.height as i64) as i32;
    let city_states = number(&args, "--city-states", size.default_city_states as i64) as usize;
    let turns = number(&args, "--turns", 300) as u32;
    let quiet = args.iter().any(|arg| arg == "--quiet");

    let mut totals = Findings::default();
    for seed in start..start + games {
        let mut g = Game::new(
            players as usize,
            width,
            height,
            seed as u64,
            turns,
            city_states,
        );
        let mut ais = AdvancedAi::fleet(&g);
        let mut history = History::default();
        let mut found = Findings::default();
        let mut last_turn = g.turn;
        while g.winner.is_none() {
            let pid = g.current;
            ais[pid].take_turn(&mut g, pid);
            if g.winner.is_none() && g.current == pid {
                let _ = g.apply(pid, &Action::EndTurn);
            }
            if g.turn != last_turn {
                last_turn = g.turn;
                audit_turn(&g, &mut history, &mut found);
            }
        }
        audit_result(&g, &mut found);

        if !quiet {
            println!(
                "seed {seed:<5} t{:<4} {:<10} {:<10} violations={} symptoms={}",
                g.turn,
                g.victory_type.clone().unwrap_or_default(),
                g.winner.map(|w| g.players[w].civ.clone()).unwrap_or_default(),
                found.violations.values().map(|entry| entry.0).sum::<usize>(),
                found.symptoms.values().map(|entry| entry.0).sum::<usize>(),
            );
            for (key, (count, detail)) in &found.violations {
                println!("    VIOLATION x{count:<5} {key} - e.g. {detail}");
            }
            for (key, (count, detail)) in &found.symptoms {
                println!("    symptom   x{count:<5} {key} - e.g. {detail}");
            }
        }
        for (key, (count, detail)) in found.violations {
            let entry = totals
                .violations
                .entry(key)
                .or_insert_with(|| (0, detail.clone()));
            entry.0 += count;
        }
        for (key, (count, detail)) in found.symptoms {
            let entry = totals
                .symptoms
                .entry(key)
                .or_insert_with(|| (0, detail.clone()));
            entry.0 += count;
        }
    }

    println!("\n=== {games} games ===");
    if totals.violations.is_empty() {
        println!("no rule violations");
    }
    for (key, (count, detail)) in &totals.violations {
        println!("VIOLATION x{count:<6} {key} - e.g. {detail}");
    }
    for (key, (count, detail)) in &totals.symptoms {
        println!("symptom   x{count:<6} {key} - e.g. {detail}");
    }
    if !totals.violations.is_empty() {
        std::process::exit(1);
    }
}
