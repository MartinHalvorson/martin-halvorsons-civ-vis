//! JSON observation builder (fog-of-war view for a player) — feeds the GUI
//! and any external agent speaking the JSON protocol.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::game::{City, Game, RememberedCity, DIPLOMATIC_VICTORY_POINTS, EXOPLANET_DESTINATION};
use crate::world::Tile;
use crate::Pos;

pub fn observation(g: &Game, pid: usize) -> Value {
    obs_impl(g, pid, false, true)
}

/// Fog-free view of the whole world from `pid`'s empire perspective —
/// feeds the spectator (watch-the-AIs) GUI mode.
pub fn observation_spectator(g: &Game, pid: usize) -> Value {
    obs_impl(g, pid, true, false)
}

/// Currently visible and ever-explored tile sets for `pid`, including
/// Level-2+ military-alliance shared vision. Every fog-honest observation
/// surface (the JSON protocol and the tensor builder) derives from this one
/// contract.
pub fn visibility(g: &Game, pid: usize) -> (BTreeSet<Pos>, BTreeSet<Pos>) {
    let p = &g.players[pid];
    let vis = g.player_visibility(pid);
    let mut explored = p.explored.clone();
    for (partner, alliance) in &p.alliances {
        if alliance.ends > g.turn && alliance.kind == "military" && alliance.level >= 2 {
            explored.extend(g.players[*partner].explored.iter().copied());
        }
    }
    (vis, explored)
}

/// Read-only, fog-of-war view used when a spectator chooses a civilization's
/// perspective. It intentionally omits expensive interactive affordances such
/// as per-unit reachability because the AI remains in control of the seat.
pub fn observation_player_view(g: &Game, pid: usize) -> Value {
    obs_impl(g, pid, false, false)
}

fn obs_impl(g: &Game, pid: usize, omniscient: bool, interactive: bool) -> Value {
    let p = &g.players[pid];
    let viewers: Vec<usize> = if omniscient {
        vec![pid]
    } else {
        g.visibility_viewers(pid).into_iter().collect()
    };
    let vis: BTreeSet<Pos> = if omniscient {
        g.map.tiles.keys().copied().collect()
    } else {
        g.player_visibility(pid)
    };
    let mut explored = if omniscient {
        vis.clone()
    } else {
        p.explored.clone()
    };
    if !omniscient {
        for viewer in &viewers {
            explored.extend(g.players[*viewer].explored.iter().copied());
        }
    }
    let tiles: Vec<Value> = explored
        .iter()
        .filter_map(|pos| {
            let (tile, owner) = if omniscient || vis.contains(pos) {
                let tile = g.map.get(*pos)?;
                let owner = tile
                    .owner_city
                    .and_then(|city| g.cities.get(&city))
                    .map(|city| city.owner);
                (tile, owner)
            } else {
                let memory = viewers
                    .iter()
                    .filter_map(|viewer| g.players[*viewer].remembered_tiles.get(pos))
                    .max_by_key(|memory| memory.seen_turn)?;
                (&memory.tile, memory.owner)
            };
            Some(tile_json(g, pid, tile, owner, omniscient))
        })
        .collect();
    let units: Vec<Value> = g
        .units
        .values()
        .filter(|u| {
            let observed_pos = u.air_patrol_pos.unwrap_or(u.pos);
            omniscient
                || u.owner == pid
                || (vis.contains(&observed_pos)
                    && viewers
                        .iter()
                        .any(|viewer| g.unit_visible_to(u.id, *viewer)))
        })
        .map(|u| {
            let mut v = serde_json::to_value(u).unwrap();
            v["embarked"] = json!(g.is_embarked(u));
            // Reachability is an interactive-player affordance. Computing it
            // for every unit of the currently observed AI can dominate late-
            // game spectator responses even though spectate mode has no legal
            // movement actions.
            if u.owner == pid && interactive {
                v["reachable"] = json!(g
                    .reachable(u.id)
                    .iter()
                    .map(|p| json!([p.0, p.1]))
                    .collect::<Vec<_>>());
                if let Some((target, gold, _)) = g.unit_gold_upgrade_offer(pid, u.id) {
                    v["upgrade"] = json!({ "to": target, "gold": gold });
                }
            }
            // Whether the unit has been left behind by the ruleset is worth
            // showing even when the upgrade itself is out of reach this turn.
            if u.owner == pid || omniscient {
                v["obsolete"] = json!(g.unit_is_obsolete(u.owner, &u.kind));
            }
            v
        })
        .collect();
    let spies: Vec<Value> = g
        .spies
        .values()
        .filter(|spy| omniscient || spy.owner == pid || spy.captured_by == Some(pid))
        .map(|spy| serde_json::to_value(spy).unwrap())
        .collect();
    let mut empire = [0.0f64; 6]; // food, prod, gold, sci, cul, faith
    for city in g.cities.values().filter(|city| city.owner == pid) {
        let yields = g.city_yields(city.id);
        empire[0] += yields.food;
        empire[1] += yields.production;
        empire[2] += yields.gold;
        empire[3] += yields.science;
        empire[4] += yields.culture;
        empire[5] += yields.faith;
    }

    enum KnownCity<'a> {
        Live(&'a City),
        Remembered(&'a RememberedCity),
    }
    let mut known_cities: BTreeMap<u32, KnownCity<'_>> = BTreeMap::new();
    if !omniscient {
        for viewer in &viewers {
            for memory in g.players[*viewer].remembered_cities.values() {
                if explored.contains(&memory.pos) && !vis.contains(&memory.pos) {
                    known_cities
                        .entry(memory.id)
                        .and_modify(|known| {
                            if matches!(known, KnownCity::Remembered(old) if memory.seen_turn > old.seen_turn)
                            {
                                *known = KnownCity::Remembered(memory);
                            }
                        })
                        .or_insert(KnownCity::Remembered(memory));
                }
            }
        }
    }
    for city in g.cities.values() {
        if omniscient || city.owner == pid || vis.contains(&city.pos) {
            known_cities.insert(city.id, KnownCity::Live(city));
        }
    }
    let cities: Vec<Value> = known_cities
        .into_values()
        .map(|known| match known {
            KnownCity::Remembered(city) => remembered_city_json(city),
            KnownCity::Live(city) => live_city_json(g, pid, city, omniscient),
        })
        .collect();
    let camps: Vec<Value> = tiles
        .iter()
        .filter(|tile| tile["improvement"] == "barbarian_camp")
        .map(|tile| tile["pos"].clone())
        .collect();
    let leading_score = g
        .players
        .iter()
        .filter(|player| !player.is_minor && !player.is_barbarian)
        .map(|player| g.score(player.id))
        .max()
        .unwrap_or(0);
    json!({
        "turn": g.turn,
        "max_turns": g.max_turns,
        "seed": g.seed,
        "game_speed": g.game_speed.id(),
        "world_era": g.world_era,
        "climate_phase": g.climate_phase,
        "climate_points": g.climate_points(),
        "player": pid,
        "current": g.current,
        "map": {
            "size": g.map_size().id,
            "size_name": g.map_size().name,
            "script": g.map_script.id(),
            "width": g.map.width,
            "height": g.map.height,
            "default_players": g.map_size().default_players,
            "max_players": g.map_size().max_players,
            "default_city_states": g.map_size().default_city_states,
            "max_city_states": g.map_size().max_city_states,
            "max_religions": g.max_religions(),
            "natural_wonders": g.map_size().natural_wonders,
            "continents": g.map_size().continents,
            "tiles": tiles,
        },
        "visible": vis.iter().filter(|v| g.map.tiles.contains_key(v))
            .map(|v| json!([v.0, v.1])).collect::<Vec<_>>(),
        "camps": camps,
        "units": units,
        "spies": spies,
        "cities": cities,
        "me": {
            "gold": round1(p.gold), "faith": round1(p.faith),
            "gold_per_turn": round1(p.gold_per_turn),
            "bankruptcy_amenity_penalty": p.bankruptcy_amenity_penalty,
            "techs": p.techs, "research": p.research,
            "research_progress": round1(p.research_progress),
            "civics": p.civics, "civic": p.civic,
            "civic_progress": round1(p.civic_progress),
            "government": p.government,
            "anarchy_turns": p.anarchy_turns,
            "pending_government": p.pending_government,
            "past_governments": p.past_governments,
            "influence": round1(p.influence),
            "envoys_free": p.envoys_free,
            "envoys": p.envoys,
            "diplomatic_favor": round1(p.diplomatic_favor),
            "power_fuel_consumed": p.power_fuel_consumed,
            "co2_emissions": round1(p.co2_emissions),
            "global_co2": round1(g.global_co2_emissions()),
            "trade_capacity": g.trade_capacity(pid),
            "gpp": p.gpp,
            "gp_claimed": p.gp_claimed,
            "great_people": p.great_people,
            "era_score": p.era_score,
            "normal_age_threshold": p.normal_age_threshold,
            "golden_age_threshold": p.golden_age_threshold,
            "dedications": p.dedications,
            "dedication_choices": p.dedication_choices,
            "available_dedications": g.available_dedications(pid),
            "governors": p.governors,
            "governor_roster": p.governor_roster,
            "governor_titles": g.governor_titles(pid),
            "governor_titles_available": g.governor_titles_available(pid),
            "dvp": p.dvp,
            "grievances": p.grievances,
            "denounced_until": p.denounced_until,
            "friends_until": p.friends_until,
            "open_borders_until": p.open_borders_until,
            "alliances": p.alliances,
            "age": p.age,
            "tourism": round1(p.tourism_lifetime),
            "religious_tourism": round1(p.religious_tourism_lifetime),
            "tourism_pressure": g.players.iter()
                .filter(|target| target.id != pid && !target.is_minor && !target.is_barbarian)
                .map(|target| (target.id.to_string(), round1(g.tourism_pressure_against(pid, target.id))))
                .collect::<BTreeMap<_, _>>(),
            "monopoly_gold_per_turn": round1(g.monopoly_bonuses(pid).0),
            "monopoly_tourism_pct": round1(g.monopoly_bonuses(pid).1),
            "secret_society": p.secret_society,
            "domestic_tourists": g.domestic_tourists(pid),
            "foreign_tourists": g.foreign_tourists(pid),
            "science_projects": p.science_projects,
            "exoplanet_distance": round1(p.exoplanet_distance),
            "exoplanet_speed": round1(g.exoplanet_speed(pid)),
            "pantheon": p.pantheon,
            "religion": p.religion,
            "religion_beliefs": p.religion_beliefs,
            "prophet_pending": p.prophet_pending,
            "routes": g.routes.iter().filter(|r| r.owner == pid)
                .map(|r| json!({"origin": r.origin, "dest": r.dest, "ends": r.ends}))
                .collect::<Vec<_>>(),
            "resources": g.rules.resources.iter()
                .filter(|(_, spec)| matches!(spec.class.as_str(), "luxury" | "strategic"))
                .filter(|(resource, _)| g.resource_visible_to(pid, resource))
                .map(|(resource, spec)| json!({
                    "id": resource,
                    "class": spec.class,
                    "native": g.connected_resource_count(pid, resource),
                    "available": g.resource_access_count(pid, resource),
                    "controlled": (spec.class == "luxury")
                        .then(|| g.controlled_resource_count(pid, resource)),
                    "stockpile": (spec.class == "strategic")
                        .then(|| round1(g.strategic_stockpile(pid, resource))),
                    "capacity": (spec.class == "strategic")
                        .then(|| round1(g.strategic_stockpile_capacity(pid))),
                    "per_turn": (spec.class == "strategic")
                        .then(|| round1(g.strategic_resource_rate(pid, resource))),
                    "shortage": (spec.class == "strategic").then(|| {
                        p.strategic_resource_shortages
                            .get(resource)
                            .copied()
                            .unwrap_or(0)
                    }),
                }))
                .collect::<Vec<_>>(),
            "policies": p.policies,
            "policy_slots": g.gov_slots(pid),
            "available_policies": g.available_policies(pid),
            "boosted_techs": p.boosted_techs,
            "boosted_civics": p.boosted_civics,
            "yields": {
                "food": round1(empire[0]), "production": round1(empire[1]),
                "gold": round1(empire[2]), "science": round1(empire[3]),
                "culture": round1(empire[4]), "faith": round1(empire[5]),
            },
        },
        "players": g.players.iter().map(|o| {
            // Civ VI's diplomacy ribbon keeps every major's broad empire
            // output visible.  These are aggregate public indicators rather
            // than hidden city details, and make spectator comparisons useful.
            let mut output = crate::rules::Yields::default();
            for cid in g.player_city_ids(o.id) {
                output.add(g.city_yields(cid));
            }
            let military = g.military_power(o.id).round() as i64;
            json!({
                "id": o.id, "civ": o.civ,
                "leader": g.rules.civs.get(&o.civ).map(|c| c.leader.clone()),
                // A leader's agenda is public knowledge in Civ VI once you
                // have met them, and so is roughly how they feel about you.
                "agenda": g.agenda_of(o.id).map(|agenda| json!({
                    "name": agenda.name,
                    "description": agenda.description,
                })),
                "opinion_of_me": round1(g.agenda_opinion(o.id, pid)),
                "alive": o.alive,
                "is_minor": o.is_minor, "is_barbarian": o.is_barbarian,
                "cs_type": if o.is_minor && !o.is_barbarian {
                    Some(Game::cs_type(&o.civ))
                } else {
                    None
                },
                "suzerain": if o.is_minor && !o.is_barbarian {
                    g.suzerain_of(o.id)
                } else {
                    None
                },
                "my_envoys": g.envoys_at(pid, o.id),
                "dvp": o.dvp,
                "domestic_tourists": g.domestic_tourists(o.id),
                "foreign_tourists": g.foreign_tourists(o.id),
                "science_projects": o.science_projects,
                "exoplanet_distance": round1(o.exoplanet_distance),
                "government": o.government,
                "anarchy_turns": o.anarchy_turns,
                "score": g.score(o.id),
                "cities": g.player_city_ids(o.id).len(),
                "suzerain_count": g.players.iter()
                    .filter(|minor| minor.alive && minor.is_minor && !minor.is_barbarian)
                    .filter(|minor| g.suzerain_of(minor.id) == Some(o.id))
                    .count(),
                "wonder_count": g.player_city_ids(o.id).iter()
                    .map(|city| g.cities[city].wonders.len())
                    .sum::<usize>(),
                "victories": if !o.is_minor && !o.is_barbarian {
                    Some(victory_progress_json(g, o.id, leading_score))
                } else {
                    None
                },
                "gold": round1(o.gold),
                "gold_per_turn": round1(o.gold_per_turn),
                "bankruptcy_amenity_penalty": o.bankruptcy_amenity_penalty,
                "faith": round1(o.faith),
                "yields": yields_json(&output),
                "military": military,
                "at_war_with_me": g.is_at_war(pid, o.id),
                "grievances_against_me": o.grievances.get(&pid).copied().unwrap_or(0.0),
                "my_grievances": p.grievances.get(&o.id).copied().unwrap_or(0.0),
                "friend": g.are_friends(pid, o.id),
                "alliance": g.alliance_with(pid, o.id),
                "open_borders_to_me": g.has_open_borders(pid, o.id),
                "my_open_borders_to_them": g.has_open_borders(o.id, pid),
            })
        }).collect::<Vec<_>>(),
        "quick_deals": if omniscient { Vec::new() } else { g.quick_deals(pid) },
        "active_trade_deals": g.active_trade_deals.iter()
            .filter(|deal| deal.from == pid || deal.to == pid)
            .collect::<Vec<_>>(),
        "pending_deals": g.pending_deals.iter()
            .filter(|deal| deal.from == pid || deal.to == pid)
            .collect::<Vec<_>>(),
        "congress": g.congress,
        "active_congress_effects": g.active_congress_effects,
        "pending_emergencies": g.pending_emergencies,
        "active_emergencies": g.active_emergencies,
        "barbarian_alerts": g.barb_alerted_until.iter()
            .filter(|(camp, _)| vis.contains(camp))
            .map(|(camp, until)| json!({
                "camp": [camp.0, camp.1],
                "target": g.barb_camp_targets.get(camp).map(|target| [target.0, target.1]),
                "until": until,
            }))
            .collect::<Vec<_>>(),
        // Who is fighting whom, since when, and at what cost. War is the one
        // part of the world every civilization can see from the outside, and
        // the diplomacy panel above already names every player, so this is
        // shown whole rather than through the viewer's fog.
        "wars": wars_json(g),
        "winner": g.winner,
        "victory_type": g.victory_type,
        // What has happened to this civilization lately, newest last. An
        // omniscient viewer watches whichever seat it is observing, so the
        // spectator log follows the same seat as the rest of the frame.
        "events": recent_events(g, pid),
    })
}

/// Every war in progress, longest-running first, followed by the most recent
/// peaces. Highlights are trimmed to the last few per war: a client wants the
/// shape of the war, and the full account of a fifty-turn conquest would cost
/// more bandwidth every frame than the rest of the observation together.
fn wars_json(g: &Game) -> Vec<Value> {
    const RECENT_PEACES: usize = 4;
    const HIGHLIGHTS: usize = 8;
    let war_json = |war: &crate::game::WarRecord| {
        let side = |player: usize| {
            let losses = war.losses_for(player);
            json!({
                "player": player,
                "units_lost": losses.units,
                "cities_lost": losses.cities,
            })
        };
        json!({
            "aggressor": war.aggressor,
            "defender": war.defender,
            "started": war.started,
            "ended": war.ended,
            "turns": war.ended.unwrap_or(g.turn).saturating_sub(war.started),
            "sides": [side(war.aggressor), side(war.defender)],
            "highlights": war.highlights[war.highlights.len().saturating_sub(HIGHLIGHTS)..]
                .iter()
                .map(|highlight| json!({
                    "turn": highlight.turn,
                    "kind": highlight.kind,
                    "actor": highlight.actor,
                    "subject": highlight.subject,
                    "city": highlight.city,
                }))
                .collect::<Vec<_>>(),
        })
    };
    let mut wars: Vec<&crate::game::WarRecord> = g.wars.values().collect();
    wars.sort_by_key(|war| (war.started, war.aggressor, war.defender));
    wars.iter()
        .map(|war| war_json(war))
        .chain(
            g.concluded_wars
                .iter()
                .rev()
                .take(RECENT_PEACES)
                .map(|war| war_json(war)),
        )
        .collect()
}

/// The tail of a civilization's event stream. Bounded because an observation
/// is sent every frame and a long game accumulates thousands.
fn recent_events(g: &Game, pid: usize) -> Vec<Value> {
    const RECENT: usize = 60;
    let events = g.events_for(pid);
    events[events.len().saturating_sub(RECENT)..]
        .iter()
        .map(|event| {
            json!({
                "turn": event.turn,
                "category": event.category,
                "text": event.text,
                "pos": event.pos.map(|pos| [pos.0, pos.1]),
            })
        })
        .collect()
}

/// Public victory-screen metrics. Each progress value is normalized to
/// 0..100 for sorting and meter width, while the underlying counts let the UI
/// describe the actual victory requirement instead of showing a vague percent.
fn victory_progress_json(g: &Game, pid: usize, leading_score: i64) -> Value {
    let player = &g.players[pid];
    let all_majors: Vec<usize> = g
        .players
        .iter()
        .filter(|candidate| !candidate.is_minor && !candidate.is_barbarian)
        .map(|candidate| candidate.id)
        .collect();
    let living_majors: Vec<usize> = all_majors
        .iter()
        .copied()
        .filter(|candidate| g.players[*candidate].alive)
        .collect();

    let science_projects = [
        "launch_earth_satellite",
        "launch_moon_landing",
        "launch_mars_colony",
        "exoplanet_expedition",
    ];
    let completed_projects = science_projects
        .iter()
        .filter(|project| player.science_projects.contains(**project))
        .count();
    let science_progress = if player.science_projects.contains("exoplanet_expedition") {
        75.0 + 25.0 * player.exoplanet_distance / EXOPLANET_DESTINATION
    } else {
        match completed_projects {
            0 => 0.0,
            1 => 25.0,
            2 => 45.0,
            _ => 65.0,
        }
    }
    .clamp(0.0, 100.0);
    // The space race is the last stretch of a road that starts at the first
    // technology, and for most of a game the tree is the only part of it a
    // watcher can see moving.
    let techs_known = player.techs.len();
    let tech_total = g.rules.techs.len();

    let rival_domestic = living_majors
        .iter()
        .filter(|candidate| **candidate != pid)
        .map(|candidate| g.domestic_tourists(*candidate))
        .max()
        .unwrap_or(0);
    let culture_target = rival_domestic + 1;
    let domestic_tourists = g.domestic_tourists(pid);
    // The culture a civilization keeps at home is the other half of this
    // race: it is what every rival's tourism has to out-run.
    let leading_domestic = all_majors
        .iter()
        .map(|candidate| g.domestic_tourists(*candidate))
        .max()
        .unwrap_or(0);
    let foreign_tourists = g.foreign_tourists(pid);
    let culture_progress = if culture_target > 0 {
        100.0 * foreign_tourists as f64 / culture_target as f64
    } else {
        0.0
    }
    .clamp(0.0, 100.0);

    let converted_civs = player.religion.as_ref().map_or(0, |religion| {
        living_majors
            .iter()
            .filter(|candidate| {
                let cities = g.player_city_ids(**candidate);
                let following = cities
                    .iter()
                    .filter(|city| g.city_religion(&g.cities[city]) == Some(religion.as_str()))
                    .count();
                !cities.is_empty() && following * 2 > cities.len()
            })
            .count()
    });
    let religious_target = living_majors.len();
    let religious_progress = if religious_target > 0 {
        100.0 * converted_civs as f64 / religious_target as f64
    } else {
        0.0
    };

    // Domination counts every original capital in the world, a civilization's
    // own included — `check_domination` treats the candidate's own seat as
    // satisfied, so in a six-player game everybody starts the race at one of
    // six rather than at nothing.
    let capital_target = all_majors.len();
    let controlled_capitals = all_majors
        .iter()
        .filter(|original_owner| {
            **original_owner == pid
                || g.cities
                    .values()
                    .find(|city| city.is_capital && city.original_owner == **original_owner)
                    .map_or(!g.players[**original_owner].alive, |capital| {
                        capital.owner == pid
                    })
        })
        .count();
    let domination_progress = if capital_target > 0 {
        100.0 * controlled_capitals as f64 / capital_target as f64
    } else {
        0.0
    };

    let diplomatic_points = player.dvp.max(0);
    let diplomatic_progress =
        100.0 * diplomatic_points as f64 / DIPLOMATIC_VICTORY_POINTS.max(1) as f64;
    let score = g.score(pid);
    let score_progress = if leading_score > 0 {
        100.0 * score.max(0) as f64 / leading_score as f64
    } else {
        0.0
    };

    json!({
        "science": {
            "progress": round1(science_progress),
            "projects": completed_projects,
            "project_target": science_projects.len(),
            "distance": round1(player.exoplanet_distance),
            "distance_target": EXOPLANET_DESTINATION,
            "techs": techs_known,
            "tech_total": tech_total,
        },
        "culture": {
            "progress": round1(culture_progress),
            "tourists": foreign_tourists,
            "target": culture_target,
            "domestic": domestic_tourists,
            "rival_domestic": rival_domestic,
            "leading_domestic": leading_domestic,
        },
        "religious": {
            "progress": round1(religious_progress),
            "converted": converted_civs,
            "target": religious_target,
        },
        "diplomatic": {
            "progress": round1(diplomatic_progress.clamp(0.0, 100.0)),
            "points": diplomatic_points,
            "target": DIPLOMATIC_VICTORY_POINTS,
        },
        "domination": {
            "progress": round1(domination_progress),
            "capitals": controlled_capitals,
            "target": capital_target,
        },
        "score": {
            "progress": round1(score_progress.clamp(0.0, 100.0)),
            "points": score,
            "leader": leading_score,
        },
    })
}

fn tile_json(g: &Game, pid: usize, tile: &Tile, owner: Option<usize>, omniscient: bool) -> Value {
    let resource = tile
        .resource
        .as_ref()
        .filter(|resource| omniscient || g.resource_visible_to(pid, resource));
    json!({
        "pos": [tile.pos.0, tile.pos.1],
        "terrain": tile.terrain,
        "feature": tile.feature,
        "hills": tile.hills,
        "resource": resource,
        "improvement": tile.improvement,
        "pillaged": tile.pillaged,
        "district": tile.district,
        "wonder": tile.wonder,
        "owner": owner,
        "river": tile.has_river(),
        "river_edges": tile.river_edges,
        "road": tile.road,
        "cliff_edges": tile.cliff_edges,
        "continent": tile.continent,
        "coastal_lowland": tile.coastal_lowland,
        "flooded": tile.flooded,
        "submerged": tile.submerged,
    })
}

struct PublicCity<'a> {
    id: u32,
    name: &'a str,
    owner: usize,
    pos: Pos,
    pop: i32,
    hp: i32,
    is_capital: bool,
    original_owner: usize,
    captured_from: Option<usize>,
    occupied_from: Option<usize>,
    wall_hp: i32,
    wall_max: i32,
    encampment_hp: i32,
    encampment_wall_hp: i32,
    encampment_pillaged: bool,
    religion: Option<&'a str>,
}

fn public_city_json(city: PublicCity<'_>) -> Value {
    json!({
        "id": city.id,
        "name": city.name,
        "owner": city.owner,
        "pos": [city.pos.0, city.pos.1],
        "pop": city.pop,
        "hp": city.hp,
        "is_capital": city.is_capital,
        "original_owner": city.original_owner,
        "captured_from": city.captured_from,
        "occupied_from": city.occupied_from,
        "wall_hp": city.wall_hp,
        "wall_max": city.wall_max,
        "encampment_hp": city.encampment_hp,
        "encampment_wall_hp": city.encampment_wall_hp,
        "encampment_pillaged": city.encampment_pillaged,
        "religion": city.religion,
    })
}

fn remembered_city_json(city: &RememberedCity) -> Value {
    public_city_json(PublicCity {
        id: city.id,
        name: &city.name,
        owner: city.owner,
        pos: city.pos,
        pop: city.pop,
        hp: city.hp,
        is_capital: city.is_capital,
        original_owner: city.original_owner,
        captured_from: city.captured_from,
        occupied_from: city.occupied_from,
        wall_hp: city.wall_hp,
        wall_max: city.wall_max,
        encampment_hp: city.encampment_hp,
        encampment_wall_hp: city.encampment_wall_hp,
        encampment_pillaged: city.encampment_pillaged,
        religion: city.religion.as_deref(),
    })
}

fn live_city_json(g: &Game, pid: usize, city: &City, omniscient: bool) -> Value {
    let mut value = public_city_json(PublicCity {
        id: city.id,
        name: &city.name,
        owner: city.owner,
        pos: city.pos,
        pop: city.pop,
        hp: city.hp,
        is_capital: city.is_capital,
        original_owner: city.original_owner,
        captured_from: city.captured_from,
        occupied_from: city.occupied_from,
        wall_hp: city.wall_hp,
        wall_max: g.city_max_wall_hp(city),
        encampment_hp: city.encampment_hp,
        encampment_wall_hp: city.encampment_wall_hp,
        encampment_pillaged: city.encampment_pillaged,
        religion: g.city_religion(city),
    });
    if city.owner != pid && !omniscient {
        return value;
    }

    let citizens = g.city_citizen_plan(city.id);
    let yields = g.city_yields(city.id);
    let private = json!({
        "food": round1(city.food),
        "production": round1(city.production),
        "queue": city.queue,
        "buildings": city.buildings,
        "products": city.products,
        "product_capacity": g.product_capacity(city),
        "districts": city.districts,
        "wonders": city.wonders,
        "owned_tiles": city.owned_tiles.iter()
            .map(|tile| json!([tile.0, tile.1])).collect::<Vec<_>>(),
        "yields": yields_json(&yields),
        "housing": g.city_housing(city),
        "amenity_surplus": g.city_amenity_surplus(city),
        "power_demand": g.city_power_demand(city),
        "power_supply": g.city_power_supply(city),
        "powered": g.city_is_powered(city),
        "reactor_age": city.reactor_age,
        "reactor_accident_risk": round1(100.0 * g.reactor_accident_risk(city.id)),
        "growth_need": g.growth_cost(city.pop),
        "queue_cost": city.queue.first()
            .map(|item| g.item_cost_for_city(city.owner, city.id, item)),
        "can_strike": g.city_can_strike(city),
        "loyalty": round1(city.loyalty),
        "governor": g.players[city.owner].governors.contains(&city.id),
        "citizens": {
            "focus": citizens.strategy.focus,
            "weights": yields_json(&citizens.strategy.weights),
            "food_target": round1(citizens.strategy.food_target),
            "worked_tiles": citizens.worked_tiles.iter()
                .map(|tile| json!([tile.0, tile.1])).collect::<Vec<_>>(),
            "specialists": citizens.specialists,
        },
    });
    merge(&mut value, private);
    value
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn yields_json(ys: &crate::rules::Yields) -> Value {
    json!({
        "food": round1(ys.food), "production": round1(ys.production),
        "gold": round1(ys.gold), "science": round1(ys.science),
        "culture": round1(ys.culture), "faith": round1(ys.faith),
    })
}

fn merge(base: &mut Value, ext: Value) {
    if let (Some(b), Some(e)) = (base.as_object_mut(), ext.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_exposes_compact_hud_and_victory_race_metrics() {
        let game = Game::new_full(2, 20, 14, 81_004, 120, 1, false);
        let observed = observation_spectator(&game, 0);
        assert_eq!(observed["max_turns"], serde_json::json!(120));

        let player = observed["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|player| player["id"] == serde_json::json!(0))
            .unwrap();
        assert_eq!(
            player["cities"],
            serde_json::json!(game.player_city_ids(0).len()),
        );
        assert!(player["suzerain_count"].is_number());
        assert!(player["wonder_count"].is_number());

        let victories = player["victories"].as_object().unwrap();
        for victory in [
            "science",
            "culture",
            "religious",
            "diplomatic",
            "domination",
            "score",
        ] {
            assert!(victories[victory]["progress"].is_number(), "{victory}");
        }
    }

    /// The victory ribbon draws two things per race where a race has two:
    /// the technology tree behind the space programme, and the culture a
    /// civilization keeps at home behind the tourism it exports. Domination
    /// counts every original capital in the world, a civilization's own
    /// included, which is how `check_domination` reads the board.
    #[test]
    fn victory_metrics_carry_the_tree_the_home_culture_and_every_capital() {
        let players = 6;
        let game = Game::new_full(players, 26, 18, 81_005, 120, 1, false);
        let observed = observation_spectator(&game, 0);
        let victories = observed["players"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|player| player["victories"].is_object())
            .map(|player| player["victories"].clone())
            .collect::<Vec<_>>();
        assert_eq!(victories.len(), players);

        for victory in &victories {
            assert_eq!(victory["domination"]["capitals"], serde_json::json!(1));
            assert_eq!(victory["domination"]["target"], serde_json::json!(players));
            assert_eq!(
                victory["science"]["tech_total"],
                serde_json::json!(game.rules.techs.len()),
            );
            assert!(victory["science"]["techs"].is_number());
            assert!(victory["culture"]["domestic"].is_number());
            assert!(victory["culture"]["leading_domestic"].is_number());
        }
    }
}
