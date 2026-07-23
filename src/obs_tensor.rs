//! Fog-honest spatial observation tensor for machine-learning agents.
//!
//! `obs_tensor(game, pid)` renders the world as fixed-shape `f32` feature
//! planes over the full (wrapped) map grid plus a global scalar vector. The
//! fog contract is identical to the JSON protocol: everything derives from
//! `obs::visibility` and the engine's per-object visibility gates, so an
//! agent trained on this tensor never sees state a human player could not.
//!
//! Layout: `data[(plane * height + row) * width + col]`, with `(col, row)`
//! from `hex::axial_to_offset` of the canonical tile position. Plane and
//! global slot names ship alongside the data so training code can bind
//! features by name instead of index.
use std::collections::BTreeSet;

use crate::game::Game;
use crate::hex;
use crate::obs::visibility;
use crate::Pos;

/// Static-terrain and dynamic-state feature planes, in tensor order.
pub const PLANES: [&str; 25] = [
    "visible",        // currently in sight
    "explored",       // ever seen (all planes below are gated on this)
    "water",          // coast/lake/ocean terrain
    "hills",
    "mountain",
    "forestlike",     // woods or rainforest feature
    "river",          // river edge count / 6
    "cliff",          // cliff edge count / 6
    "road",
    "resource",       // a resource this player can see (tech-gated)
    "improvement",    // active (unpillaged) improvement
    "pillaged",       // pillaged improvement or district
    "district",       // completed district
    "wonder",         // completed world wonder
    "territory_mine",
    "territory_other",
    "city_mine",
    "city_other",
    "city_hp",        // remaining city HP fraction (garrison + walls share)
    "unit_mine_mil",  // strongest own military unit strength / 100
    "unit_mine_civ",  // own civilian/support/religious presence
    "unit_enemy_mil", // visible at-war military strength / 100
    "unit_other",     // visible units of civs not at war with us
    "unit_mine_hp",   // strongest own military unit HP fraction
    "unit_enemy_hp",  // visible at-war military HP fraction
];

/// Names for the global scalar vector: empire block, then a fixed block of
/// [`MAX_RIVALS`] rival slots (met/alive/war/score-share), then era/turn.
pub const MAX_RIVALS: usize = 7;

pub struct ObsTensor {
    pub width: i32,
    pub height: i32,
    /// `PLANES.len() * height * width` values in plane-major order.
    pub data: Vec<f32>,
    pub planes: &'static [&'static str],
    pub global: Vec<f32>,
    pub global_names: Vec<String>,
}

impl ObsTensor {
    pub fn at(&self, plane: usize, pos: Pos) -> f32 {
        let canon = hex::canon(pos, self.width);
        let (col, row) = hex::axial_to_offset(canon.0, canon.1);
        self.data[(plane * self.height as usize + row as usize) * self.width as usize
            + col as usize]
    }

    pub fn plane_index(name: &str) -> usize {
        PLANES.iter().position(|p| *p == name).expect("unknown plane")
    }
}

pub fn obs_tensor(g: &Game, pid: usize) -> ObsTensor {
    let (vis, explored) = visibility(g, pid);
    let (w, h) = (g.map.width as usize, g.map.height as usize);
    let mut data = vec![0.0f32; PLANES.len() * w * h];
    let plane = |name: &str| ObsTensor::plane_index(name);
    let index_of = |pos: Pos| -> usize {
        let canon = hex::canon(pos, g.map.width);
        let (col, row) = hex::axial_to_offset(canon.0, canon.1);
        row as usize * w + col as usize
    };
    let put = |data: &mut Vec<f32>, name: &str, pos: Pos, value: f32| {
        let slot = plane(name) * w * h + index_of(pos);
        if value > data[slot] {
            data[slot] = value;
        }
    };

    for pos in &explored {
        let Some(tile) = g.map.get(*pos) else { continue };
        put(&mut data, "explored", *pos, 1.0);
        if vis.contains(pos) {
            put(&mut data, "visible", *pos, 1.0);
        }
        match tile.terrain.as_str() {
            "coast" | "lake" | "ocean" => put(&mut data, "water", *pos, 1.0),
            "mountain" => put(&mut data, "mountain", *pos, 1.0),
            _ => {}
        }
        if tile.hills {
            put(&mut data, "hills", *pos, 1.0);
        }
        if matches!(tile.feature.as_deref(), Some("woods") | Some("rainforest")) {
            put(&mut data, "forestlike", *pos, 1.0);
        }
        let rivers = tile.river_edges.iter().filter(|e| **e).count();
        if rivers > 0 {
            put(&mut data, "river", *pos, rivers as f32 / 6.0);
        }
        let cliffs = tile.cliff_edges.iter().filter(|e| **e).count();
        if cliffs > 0 {
            put(&mut data, "cliff", *pos, cliffs as f32 / 6.0);
        }
        if tile.road > 0 {
            put(&mut data, "road", *pos, 1.0);
        }
        if let Some(resource) = &tile.resource {
            if g.resource_visible_to(pid, resource) {
                put(&mut data, "resource", *pos, 1.0);
            }
        }
        if tile.improvement.is_some() && !tile.pillaged {
            put(&mut data, "improvement", *pos, 1.0);
        }
        if tile.pillaged {
            put(&mut data, "pillaged", *pos, 1.0);
        }
        if tile.district.is_some() {
            put(&mut data, "district", *pos, 1.0);
        }
        if tile.wonder.is_some() {
            put(&mut data, "wonder", *pos, 1.0);
        }
        if let Some(cid) = tile.owner_city {
            if let Some(city) = g.cities.get(&cid) {
                let name = if city.owner == pid { "territory_mine" } else { "territory_other" };
                put(&mut data, name, *pos, 1.0);
            }
        }
    }

    // Cities appear once explored (matching the JSON protocol); their live
    // HP is only refreshed while visible, but a fixed tensor cannot carry a
    // stale-memory channel yet, so HP is emitted for explored cities as the
    // engine's current value gated to visibility.
    for city in g.cities.values() {
        if !explored.contains(&city.pos) {
            continue;
        }
        let name = if city.owner == pid { "city_mine" } else { "city_other" };
        put(&mut data, name, city.pos, 1.0);
        if vis.contains(&city.pos) || city.owner == pid {
            let pool = (city.hp + city.wall_hp).max(0) as f32;
            let cap = (100 + g.city_max_wall_hp(city).max(0)) as f32;
            put(&mut data, "city_hp", city.pos, (pool / cap).clamp(0.0, 1.0));
        }
    }

    for unit in g.units.values() {
        let mine = unit.owner == pid;
        if !mine && !(vis.contains(&unit.pos) && g.unit_visible_to(unit.id, pid)) {
            continue;
        }
        let military = g.rules.units[unit.kind.as_str()].class == "military";
        let hp = (unit.hp as f32 / 100.0).clamp(0.0, 1.0);
        if mine {
            if military {
                let s = (g.unit_strength(unit, false) as f32 / 100.0).clamp(0.0, 1.0);
                put(&mut data, "unit_mine_mil", unit.pos, s);
                put(&mut data, "unit_mine_hp", unit.pos, hp);
            } else {
                put(&mut data, "unit_mine_civ", unit.pos, 1.0);
            }
        } else if g.is_at_war(pid, unit.owner) && military {
            let s = (g.unit_strength(unit, false) as f32 / 100.0).clamp(0.0, 1.0);
            put(&mut data, "unit_enemy_mil", unit.pos, s);
            put(&mut data, "unit_enemy_hp", unit.pos, hp);
        } else {
            put(&mut data, "unit_other", unit.pos, 1.0);
        }
    }

    let (global, global_names) = global_block(g, pid, &vis);
    ObsTensor {
        width: g.map.width,
        height: g.map.height,
        data,
        planes: &PLANES,
        global,
        global_names,
    }
}

/// Own-empire exacts plus public/observed rival facts. Rival slots are
/// ordered by player id, skipping minors and ourselves, and are all-zero
/// until that civilization has been met.
fn global_block(g: &Game, pid: usize, vis: &BTreeSet<Pos>) -> (Vec<f32>, Vec<String>) {
    let p = &g.players[pid];
    let mut names: Vec<String> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let push = |names: &mut Vec<String>, out: &mut Vec<f32>, n: &str, v: f32| {
        names.push(n.to_string());
        out.push(v);
    };

    let cids = g.player_city_ids(pid);
    let mut yields = [0.0f64; 4]; // science, culture, gold, faith rates
    for cid in &cids {
        let y = g.city_yields(*cid);
        yields[0] += y.science;
        yields[1] += y.culture;
        yields[2] += y.gold;
        yields[3] += y.faith;
    }
    push(&mut names, &mut out, "turn", g.turn as f32 / g.max_turns.max(1) as f32);
    push(&mut names, &mut out, "cities", cids.len() as f32 / 12.0);
    push(&mut names, &mut out, "population",
        cids.iter().map(|c| g.cities[c].pop).sum::<i32>() as f32 / 80.0);
    push(&mut names, &mut out, "techs",
        p.techs.len() as f32 / g.rules.techs.len() as f32);
    push(&mut names, &mut out, "civics",
        p.civics.len() as f32 / g.rules.civics.len() as f32);
    push(&mut names, &mut out, "science_rate", yields[0] as f32 / 200.0);
    push(&mut names, &mut out, "culture_rate", yields[1] as f32 / 200.0);
    push(&mut names, &mut out, "gold_rate", yields[2] as f32 / 300.0);
    push(&mut names, &mut out, "faith_rate", yields[3] as f32 / 100.0);
    push(&mut names, &mut out, "treasury", p.gold as f32 / 2000.0);
    push(&mut names, &mut out, "faith", p.faith as f32 / 1000.0);
    push(&mut names, &mut out, "military_power",
        g.military_power(pid) as f32 / 500.0);
    push(&mut names, &mut out, "units",
        g.player_unit_ids(pid).len() as f32 / 30.0);
    push(&mut names, &mut out, "score", g.score(pid) as f32 / 1500.0);
    push(&mut names, &mut out, "explored_share",
        p.explored.len() as f32 / g.map.tiles.len().max(1) as f32);

    let rivals: Vec<usize> = g
        .players
        .iter()
        .filter(|o| o.id != pid && !o.is_minor && !o.is_barbarian)
        .map(|o| o.id)
        .collect();
    // The engine has no first-contact state (all majors are mutually known),
    // so rival slots are populated for every major. Alive/war status and
    // score match the in-game public ranking screens; military is only what
    // our eyes can currently count.
    for slot in 0..MAX_RIVALS {
        let prefix = format!("rival{slot}");
        match rivals.get(slot) {
            Some(&other) => {
                let o = &g.players[other];
                push(&mut names, &mut out, &format!("{prefix}_alive"),
                    o.alive as u8 as f32);
                push(&mut names, &mut out, &format!("{prefix}_war"),
                    g.is_at_war(pid, other) as u8 as f32);
                push(&mut names, &mut out, &format!("{prefix}_score"),
                    g.score(other) as f32 / 1500.0);
                let seen: f32 = g
                    .units
                    .values()
                    .filter(|u| {
                        u.owner == other
                            && g.rules.units[u.kind.as_str()].class == "military"
                            && vis.contains(&u.pos)
                            && g.unit_visible_to(u.id, pid)
                    })
                    .map(|u| g.unit_strength(u, false) as f32)
                    .sum();
                push(&mut names, &mut out, &format!("{prefix}_seen_military"),
                    seen / 500.0);
            }
            None => {
                for field in ["alive", "war", "score", "seen_military"] {
                    push(&mut names, &mut out, &format!("{prefix}_{field}"), 0.0);
                }
            }
        }
    }
    (out, names)
}

#[cfg(test)]
mod tests {
    use super::{obs_tensor, ObsTensor, MAX_RIVALS, PLANES};
    use crate::ai::{run_game, AdvancedAi};
    use crate::game::Game;

    #[test]
    fn shape_and_determinism() {
        let g = Game::new(4, 24, 16, 11, 60, 2);
        let t = obs_tensor(&g, 0);
        assert_eq!(t.data.len(), PLANES.len() * 24 * 16);
        assert_eq!(t.global.len(), t.global_names.len());
        assert_eq!(t.global.len(), 15 + MAX_RIVALS * 4);
        let again = obs_tensor(&g, 0);
        assert_eq!(t.data, again.data);
        assert_eq!(t.global, again.global);
        assert!(t.data.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0));
        assert!(t.global.iter().all(|v| v.is_finite()));
    }

    /// Fog honesty: no plane may carry information on unexplored tiles, and
    /// enemy units outside current sight must be absent.
    #[test]
    fn fog_is_respected() {
        let mut g = Game::new(4, 28, 18, 5, 80, 2);
        let mut ais = AdvancedAi::fleet(&g);
        run_game(&mut g, &mut ais);
        for pid in 0..2 {
            if !g.players[pid].alive {
                continue;
            }
            let t = obs_tensor(&g, pid);
            let explored = ObsTensor::plane_index("explored");
            let visible = ObsTensor::plane_index("visible");
            for pos in g.map.tiles.keys() {
                if t.at(explored, *pos) == 0.0 {
                    for plane in 0..PLANES.len() {
                        assert_eq!(
                            t.at(plane, *pos),
                            0.0,
                            "unexplored {pos:?} leaks plane {}",
                            PLANES[plane]
                        );
                    }
                }
            }
            let enemy_mil = ObsTensor::plane_index("unit_enemy_mil");
            for unit in g.units.values() {
                if unit.owner != pid && t.at(visible, unit.pos) == 0.0 {
                    assert_eq!(
                        t.at(enemy_mil, unit.pos),
                        0.0,
                        "hidden enemy at {:?} leaked into tensor",
                        unit.pos
                    );
                }
            }
        }
    }

    /// The omniscient engine state and the fog view must actually differ for
    /// a fresh game — otherwise the fog gates are dead code.
    #[test]
    fn fog_hides_most_of_a_fresh_map() {
        let g = Game::new(4, 40, 24, 3, 60, 4);
        let t = obs_tensor(&g, 0);
        let explored = ObsTensor::plane_index("explored");
        let seen: f32 = g.map.tiles.keys().map(|p| t.at(explored, *p)).sum();
        assert!(
            (seen as usize) < g.map.tiles.len() / 4,
            "a turn-0 player has explored {seen} of {} tiles",
            g.map.tiles.len()
        );
    }
}
