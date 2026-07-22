//! JSON observation builder (fog-of-war view for a player) — feeds the GUI
//! and any external agent speaking the JSON protocol.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::game::{growth_threshold, Game};
use crate::Pos;

pub fn observation(g: &Game, pid: usize) -> Value {
    obs_impl(g, pid, false)
}

/// Fog-free view of the whole world from `pid`'s empire perspective —
/// feeds the spectator (watch-the-AIs) GUI mode.
pub fn observation_spectator(g: &Game, pid: usize) -> Value {
    obs_impl(g, pid, true)
}

fn obs_impl(g: &Game, pid: usize, omniscient: bool) -> Value {
    let p = &g.players[pid];
    let mut vis: BTreeSet<Pos> = BTreeSet::new();
    if omniscient {
        vis.extend(g.map.tiles.keys().cloned());
    } else {
        for uid in g.player_unit_ids(pid) {
            let u = &g.units[&uid];
            let sight = g.unit_sight(uid);
            vis.extend(g.wdisk(u.pos, sight));
        }
        for cid in g.player_city_ids(pid) {
            let c = &g.cities[&cid];
            vis.extend(g.wdisk(c.pos, 2));
            vis.extend(c.owned_tiles.iter().cloned());
        }
    }
    let explored: &BTreeSet<Pos> = if omniscient { &vis } else { &p.explored };
    let tiles: Vec<Value> = explored
        .iter()
        .filter_map(|pos| {
            let t = g.map.get(*pos)?;
            let owner = t
                .owner_city
                .and_then(|oc| g.cities.get(&oc))
                .map(|c| c.owner);
            let resource = t
                .resource
                .as_ref()
                .filter(|resource| omniscient || g.resource_visible_to(pid, resource));
            Some(json!({
                "pos": [pos.0, pos.1], "terrain": t.terrain, "feature": t.feature,
                "hills": t.hills, "resource": resource,
                "improvement": t.improvement, "pillaged": t.pillaged,
                "district": t.district,
                "wonder": t.wonder,
                "owner": owner, "river": t.has_river(),
                "river_edges": t.river_edges, "road": t.road,
                "cliff_edges": t.cliff_edges,
                "continent": t.continent,
                "coastal_lowland": t.coastal_lowland,
                "flooded": t.flooded,
                "submerged": t.submerged,
            }))
        })
        .collect();
    let units: Vec<Value> = g
        .units
        .values()
        .filter(|u| {
            (u.owner == pid || vis.contains(&u.pos)) && (omniscient || g.unit_visible_to(u.id, pid))
        })
        .map(|u| {
            let mut v = serde_json::to_value(u).unwrap();
            if u.owner == pid {
                v["reachable"] = json!(g
                    .reachable(u.id)
                    .iter()
                    .map(|p| json!([p.0, p.1]))
                    .collect::<Vec<_>>());
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
    let mut cities: Vec<Value> = Vec::new();
    for c in g.cities.values() {
        if !explored.contains(&c.pos) {
            continue;
        }
        let mut d = json!({
            "id": c.id, "name": c.name, "owner": c.owner,
            "pos": [c.pos.0, c.pos.1], "pop": c.pop, "hp": c.hp,
            "is_capital": c.is_capital,
            "original_owner": c.original_owner,
            "captured_from": c.captured_from,
            "occupied_from": c.occupied_from,
            "wall_hp": c.wall_hp, "wall_max": g.city_max_wall_hp(c),
            "encampment_hp": c.encampment_hp,
            "encampment_wall_hp": c.encampment_wall_hp,
            "encampment_pillaged": c.encampment_pillaged,
            "religion": g.city_religion(c),
        });
        if c.owner == pid {
            let citizens = g.city_citizen_plan(c.id);
            let ys = g.city_yields(c.id);
            empire[0] += ys.food;
            empire[1] += ys.production;
            empire[2] += ys.gold;
            empire[3] += ys.science;
            empire[4] += ys.culture;
            empire[5] += ys.faith;
            let ext = json!({
                "food": round1(c.food), "production": round1(c.production),
                "queue": c.queue, "buildings": c.buildings,
                "products": c.products,
                "product_capacity": g.product_capacity(c),
                "districts": c.districts,
                "wonders": c.wonders,
                "owned_tiles": c.owned_tiles.iter()
                    .map(|t| json!([t.0, t.1])).collect::<Vec<_>>(),
                "yields": yields_json(&ys),
                "housing": g.city_housing(c),
                "amenity_surplus": g.city_amenity_surplus(c),
                "power_demand": g.city_power_demand(c),
                "power_supply": g.city_power_supply(c),
                "powered": g.city_is_powered(c),
                "reactor_age": c.reactor_age,
                "reactor_accident_risk": round1(100.0 * g.reactor_accident_risk(c.id)),
                "growth_need": growth_threshold(c.pop),
                "queue_cost": c.queue.first().map(|it| g.item_cost_for_city(c.owner, c.id, it)),
                "can_strike": g.city_can_strike(c),
                "loyalty": round1(c.loyalty),
                "governor": g.players[c.owner].governors.contains(&c.id),
                "citizens": {
                    "focus": citizens.strategy.focus,
                    "weights": yields_json(&citizens.strategy.weights),
                    "food_target": round1(citizens.strategy.food_target),
                    "worked_tiles": citizens.worked_tiles.iter()
                        .map(|t| json!([t.0, t.1])).collect::<Vec<_>>(),
                    "specialists": citizens.specialists,
                },
            });
            merge(&mut d, ext);
        }
        cities.push(d);
    }
    json!({
        "turn": g.turn,
        "seed": g.seed,
        "world_era": g.world_era,
        "climate_phase": g.climate_phase,
        "climate_points": g.climate_points(),
        "player": pid,
        "current": g.current,
        "map": {
            "size": g.map_size().id,
            "size_name": g.map_size().name,
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
        "camps": g.barb_camps.keys().filter(|cp| explored.contains(cp))
            .map(|cp| json!([cp.0, cp.1])).collect::<Vec<_>>(),
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
                "score": g.score(o.id),
                "cities": g.player_city_ids(o.id).len(),
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
            .filter(|(camp, _)| explored.contains(camp))
            .map(|(camp, until)| json!({
                "camp": [camp.0, camp.1],
                "target": g.barb_camp_targets.get(camp).map(|target| [target.0, target.1]),
                "until": until,
            }))
            .collect::<Vec<_>>(),
        "winner": g.winner,
        "victory_type": g.victory_type,
    })
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
