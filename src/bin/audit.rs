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
use std::collections::{BTreeMap, HashSet};

use civvis::ai::{AdvancedAi, Ai};
use civvis::game::{Action, Game, WarRecord};
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
const WAR_MIN_TURNS: u32 = 10;

/// The minimum applies to a negotiated settlement, not to eliminating the
/// opposing civilization. A quick conquest is a decisive war, not diplomacy
/// undoing a declaration before its commitment has run.
fn negotiated_war_ended_early(war: &WarRecord) -> bool {
    let Some(ended) = war.ended else {
        return false;
    };
    war.highlights
        .last()
        .is_some_and(|highlight| highlight.kind == "peace")
        && ended.saturating_sub(war.started) < WAR_MIN_TURNS
}

/// Only negotiated peace creates the treaty whose cooldown this audit checks.
/// Emergency coalitions can compel two members to stop fighting, and conquest
/// can close a ledger record too; neither is a peace agreement between the
/// pair, so a later war must not be reported as violating a treaty that never
/// existed.
fn redeclared_inside_peace_treaty(previous: &WarRecord, next: &WarRecord) -> bool {
    let previous_pair = (
        previous.aggressor.min(previous.defender),
        previous.aggressor.max(previous.defender),
    );
    let next_pair = (
        next.aggressor.min(next.defender),
        next.aggressor.max(next.defender),
    );
    let Some(ended) = previous.ended else {
        return false;
    };
    previous_pair == next_pair
        && previous
            .highlights
            .last()
            .is_some_and(|highlight| highlight.kind == "peace")
        && next.started.saturating_sub(ended) < WAR_MIN_TURNS
}

fn rapid_recapture_window(war: &WarRecord) -> Option<(String, u32, u32)> {
    let mut captures: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for highlight in &war.highlights {
        if matches!(highlight.kind.as_str(), "city_captured" | "capital_captured") {
            if let Some(city) = &highlight.city {
                captures.entry(city.clone()).or_default().push(highlight.turn);
            }
        }
    }
    captures.into_iter().find_map(|(city, turns)| {
        turns
            .windows(3)
            .find(|window| window[2].saturating_sub(window[0]) <= WAR_MIN_TURNS)
            .map(|window| (city, window[0], window[2]))
    })
}

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
    let goals: HashSet<_> = legal_sites.iter().copied().collect();
    let exhaustive_step = g.route_step_to_any(id, &goals);
    format!(
        "; at {:?}, cities={}, legal_sites={}, reachable={}, exhaustive_step={:?}, shipbuilding={}, linked={:?}",
        unit.pos,
        g.player_city_ids(pid).len(),
        legal_sites.len(),
        reachable,
        exhaustive_step,
        g.players[pid].techs.contains("shipbuilding"),
        unit.linked_to,
    )
}

fn stalled_trader_context(g: &Game, id: u32) -> String {
    let unit = &g.units[&id];
    let pid = unit.owner;
    let traders = g
        .units
        .values()
        .filter(|candidate| candidate.owner == pid && candidate.kind == "trader")
        .count();
    let legal_routes = g
        .legal_actions(pid)
        .into_iter()
        .filter(|action| matches!(action, Action::TradeRoute { unit, .. } if *unit == id))
        .count();
    let city = g.city_at(unit.pos).map(|city| g.cities[&city].name.clone());
    format!(
        "; at {:?}, city={city:?}, capacity={}, active_routes={}, available_traders={traders}, legal_routes={legal_routes}",
        unit.pos,
        g.trade_capacity(pid),
        g.active_routes(pid),
    )
}

fn trader_is_waiting_for_capacity(kind: &str, active_routes: i64, capacity: i64) -> bool {
    kind == "trader" && active_routes >= capacity
}

fn idle_city_context(g: &Game, id: u32) -> String {
    let city = &g.cities[&id];
    let player = &g.players[city.owner];
    let producible = g.producible_items(city.owner, id);
    let districts: Vec<_> = city.districts.keys().cloned().collect();
    format!(
        "; pop={}, Gold={:.0}, GPT={:.1}, districts={districts:?}, buildings={}, producible={} {producible:?}",
        city.pop,
        player.gold,
        player.gold_per_turn,
        city.buildings.len(),
        producible.len(),
    )
}

fn bounded_minor_idle(
    player_is_minor: bool,
    military: usize,
    producible: &[civvis::game::Item],
) -> bool {
    player_is_minor
        && military >= 3
        && !producible.is_empty()
        && producible
            .iter()
            .all(|item| matches!(item, civvis::game::Item::Unit { .. }))
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
            // Losing Merchant Republic or a temporary policy slot can leave
            // a Trader waiting behind routes that were already active. It
            // will take the next slot when one completes; report Traders only
            // when spare capacity exists and they still cannot find a job.
            && !trader_is_waiting_for_capacity(
                &unit.kind,
                g.active_routes(unit.owner),
                g.trade_capacity(unit.owner),
            )
            && !history.reported_unit.get(id).copied().unwrap_or(false)
        {
            history.reported_unit.insert(*id, true);
            let context = match unit.kind.as_str() {
                "settler" => stalled_settler_context(g, *id),
                "trader" => stalled_trader_context(g, *id),
                _ => String::new(),
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
        // Bombard-class attacks may legally deplete a city to exactly zero;
        // it remains standing until a melee-capable unit captures it.
        if city.hp < 0 {
            found.violation(
                "city below zero HP",
                format!("city {id} ({}) at {} HP", city.name, city.hp),
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
            let producible = g.producible_items(city.owner, *id);
            let military = g
                .player_unit_ids(city.owner)
                .into_iter()
                .filter(|unit| {
                    g.rules.units[g.units[unit].kind.as_str()].class == "military"
                })
                .count();
            if bounded_minor_idle(g.players[city.owner].is_minor, military, &producible) {
                // A one-city state with no remaining infrastructure or
                // project site deliberately stops at its bounded garrison.
                // Calling that a production defect would pressure the AI to
                // fill the map with units solely to silence the auditor.
                history.city_idle_since.remove(id);
                continue;
            }
            let since = history.city_idle_since.entry(*id).or_insert(g.turn);
            if g.turn - *since >= IDLE_TURNS
                && !history.reported_city.get(id).copied().unwrap_or(false)
            {
                history.reported_city.insert(*id, true);
                let context = idle_city_context(g, *id);
                found.symptom(
                    "city builds nothing for 25+ turns",
                    format!(
                        "city {id} ({}) of {} idle since turn {since}{context}",
                        city.name, g.players[city.owner].civ,
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
    let mut previous: BTreeMap<(usize, usize), &WarRecord> = BTreeMap::new();
    for war in &g.concluded_wars {
        let key = (war.aggressor.min(war.defender), war.aggressor.max(war.defender));
        let Some(ended) = war.ended else { continue };
        let sides = (
            g.players[war.aggressor].civ.clone(),
            g.players[war.defender].civ.clone(),
        );
        if negotiated_war_ended_early(war) {
            found.violation(
                "a war ended before the shipped minimum",
                format!(
                    "{} against {} ran turns {}-{ended}",
                    sides.0, sides.1, war.started
                ),
            );
        }
        if let Some(previous_war) = previous.insert(key, war) {
            if redeclared_inside_peace_treaty(previous_war, war) {
                let last = previous_war.ended.unwrap_or_default();
                found.violation(
                    "the same pair re-declared inside the peace treaty",
                    format!(
                        "{} against {} again on turn {} after peace on turn {last}",
                        sides.0, sides.1, war.started
                    ),
                );
            }
        }
        if let Some((city, first, last)) = rapid_recapture_window(war) {
            found.symptom(
                "the same city is captured three times in ten turns",
                format!(
                    "{} repeatedly captured {city} from {} on turns {first}-{last}",
                    sides.0, sides.1
                ),
            );
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use civvis::game::{Item, WarHighlight, WarRecord};

    use super::{
        bounded_minor_idle, negotiated_war_ended_early, rapid_recapture_window,
        redeclared_inside_peace_treaty, trader_is_waiting_for_capacity,
    };

    fn concluded_war(kind: &str) -> WarRecord {
        WarRecord {
            aggressor: 0,
            defender: 1,
            started: 20,
            ended: Some(24),
            losses: BTreeMap::new(),
            highlights: vec![
                WarHighlight {
                    turn: 20,
                    kind: "declared".to_string(),
                    actor: 0,
                    subject: 1,
                    city: None,
                },
                WarHighlight {
                    turn: 24,
                    kind: kind.to_string(),
                    actor: 0,
                    subject: 1,
                    city: None,
                },
            ],
        }
    }

    #[test]
    fn the_war_minimum_applies_only_to_negotiated_peace() {
        assert!(negotiated_war_ended_early(&concluded_war("peace")));
        assert!(!negotiated_war_ended_early(&concluded_war("conquest")));
        assert!(!negotiated_war_ended_early(&concluded_war("coalition")));
    }

    #[test]
    fn the_treaty_cooldown_applies_only_after_negotiated_peace() {
        let mut next = concluded_war("peace");
        next.started = 25;
        next.ended = Some(35);
        next.highlights[0].turn = 25;

        assert!(redeclared_inside_peace_treaty(
            &concluded_war("peace"),
            &next
        ));
        assert!(!redeclared_inside_peace_treaty(
            &concluded_war("coalition"),
            &next
        ));
        assert!(!redeclared_inside_peace_treaty(
            &concluded_war("conquest"),
            &next
        ));
    }

    #[test]
    fn rapid_loyalty_recaptures_are_visible_to_the_auditor() {
        let mut war = concluded_war("peace");
        war.highlights.splice(
            1..1,
            [20, 24, 27].map(|turn| WarHighlight {
                turn,
                kind: "capital_captured".to_string(),
                actor: 0,
                subject: 1,
                city: Some("Loop City".to_string()),
            }),
        );
        assert_eq!(
            rapid_recapture_window(&war),
            Some(("Loop City".to_string(), 20, 27))
        );
    }

    #[test]
    fn bounded_city_state_garrisons_are_not_reported_as_idle_production() {
        let units = vec![
            Item::Unit {
                unit: "builder".to_string(),
            },
            Item::Unit {
                unit: "warrior".to_string(),
            },
        ];
        assert!(bounded_minor_idle(true, 3, &units));
        assert!(!bounded_minor_idle(false, 3, &units));
        assert!(!bounded_minor_idle(true, 2, &units));

        let mut investment = units;
        investment.push(Item::Project {
            project: "campus_research_grants".to_string(),
        });
        assert!(!bounded_minor_idle(true, 3, &investment));
    }

    #[test]
    fn a_trader_waiting_behind_full_capacity_is_not_idle() {
        assert!(trader_is_waiting_for_capacity("trader", 1, 1));
        assert!(trader_is_waiting_for_capacity("trader", 2, 1));
        assert!(!trader_is_waiting_for_capacity("trader", 0, 1));
        assert!(!trader_is_waiting_for_capacity("builder", 1, 1));
    }
}
