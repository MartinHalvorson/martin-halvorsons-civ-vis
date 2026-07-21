//! Martin Halvorson's Civilization VIS — Rust performance core.
//! Same ruleset JSON, action protocol, and mechanics as the Python
//! reference engine (`civvis/`); deterministic per seed within this engine.

pub mod ai;
pub mod elo;
pub mod game;
pub mod hex;
pub mod mapgen;
pub mod obs;
pub mod rng;
pub mod rules;
pub mod server;
pub mod world;

pub type Pos = (i32, i32);

#[cfg(test)]
mod tests {
    use crate::ai::{run_game, BasicAi};
    use crate::game::{Action, Game};
    use crate::hex;

    #[test]
    fn hex_math() {
        assert_eq!(hex::distance((0, 0), (3, -2)), 3);
        assert_eq!(hex::disk((0, 0), 1).len(), 7);
        assert_eq!(hex::disk((4, -1), 2).len(), 19);
        for n in hex::neighbors((2, 5)) {
            assert_eq!(hex::distance((2, 5), n), 1);
        }
    }

    #[test]
    fn determinism_same_seed() {
        let mut a = Game::new(2, 20, 14, 42, 30, 2);
        let mut b = Game::new(2, 20, 14, 42, 30, 2);
        let mut ais_a = BasicAi::fleet(&a);
        let mut ais_b = BasicAi::fleet(&b);
        run_game(&mut a, &mut ais_a);
        run_game(&mut b, &mut ais_b);
        let ja = serde_json::to_value(&a).unwrap();
        let jb = serde_json::to_value(&b).unwrap();
        assert_eq!(ja, jb);
    }

    #[test]
    fn selfplay_completes() {
        let mut g = Game::new(2, 20, 14, 11, 60, 2);
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        assert!(g.winner.is_some());
        assert!(g.cities.len() >= 2);
        for p in &g.players {
            if !p.is_barbarian {
                assert!(p.techs.len() > 1);
            }
        }
    }

    #[test]
    fn city_states_stay_single() {
        let mut g = Game::new(2, 28, 18, 2, 50, 3);
        let minors: Vec<usize> = g.players.iter()
            .filter(|p| p.is_minor && !p.is_barbarian).map(|p| p.id).collect();
        assert!(!minors.is_empty());
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        for pid in minors {
            let founded = g.cities.values().filter(|c| c.original_owner == pid).count();
            assert_eq!(founded, 1);
        }
    }

    #[test]
    fn movement_range_and_move_to() {
        let mut g = Game::new_full(2, 20, 14, 5, 60, 0, false);
        let uid = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let reach = g.reachable(uid);
        assert!(!reach.is_empty());
        let start = g.units[&uid].pos;
        let far = *reach.iter()
            .max_by_key(|p| crate::hex::distance(start, **p)).unwrap();
        g.apply(0, &Action::MoveTo { unit: uid, to: far }).unwrap();
        assert_eq!(g.units[&uid].pos, far);
    }

    #[test]
    fn rivers_freshwater_embark_wonders() {
        let g = Game::new_full(2, 24, 16, 3, 60, 0, false);
        // rivers generate
        assert!(g.map.tiles.values().any(|t| t.river));
        // embark gated on shipbuilding
        let mut g2 = Game::new_full(2, 24, 16, 3, 60, 0, false);
        let uid = g2.player_unit_ids(0)[0];
        let coast = g2.map.tiles.values()
            .find(|t| t.terrain == "coast"
                && crate::hex::distance(t.pos, g2.units[&uid].pos) == 1)
            .map(|t| t.pos);
        if let Some(c) = coast {
            assert!(!g2.can_move(uid, c));
            g2.players[0].techs.insert("shipbuilding".to_string());
            assert!(g2.can_move(uid, c));
        }
        // wonders are world-unique
        assert!(g2.rules.buildings["pyramids"].wonder);
        let cid = {
            let s = g2.player_unit_ids(0).into_iter()
                .find(|id| g2.units[id].kind == "settler").unwrap();
            g2.apply(0, &Action::FoundCity { unit: s }).unwrap();
            g2.player_city_ids(0)[0]
        };
        g2.cities.get_mut(&cid).unwrap().buildings.push("pyramids".to_string());
        assert!(g2.wonder_built("pyramids"));
        g2.players[0].techs.insert("masonry".to_string());
        assert!(!g2.can_produce(0, cid,
            &crate::game::Item::Building { building: "pyramids".to_string() }));
    }

    /// Move a unit by id via save-edit (occ indexes rebuild on load).
    fn teleport(g: &Game, uid: u32, to: crate::Pos) -> Game {
        let mut v = serde_json::to_value(g).unwrap();
        for u in v["units"].as_array_mut().unwrap() {
            if u["id"] == serde_json::json!(uid) {
                u["pos"] = serde_json::json!([to.0, to.1]);
            }
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn river_cost_and_min_one_move() {
        let g = Game::new_full(2, 24, 16, 3, 60, 0, false);
        let (river, flat) = {
            let mut found = None;
            'outer: for t in g.map.tiles.values() {
                if !t.river || g.rules.is_water(t) {
                    continue;
                }
                for n in hex::neighbors(t.pos) {
                    if let Some(nt) = g.map.get(n) {
                        if !nt.river && !g.rules.is_water(nt) {
                            found = Some((t.pos, n));
                            break 'outer;
                        }
                    }
                }
            }
            found.expect("map has a river tile with a dry neighbor")
        };
        // crossing surcharge: entering the river tile costs +2 over base
        let base = g.rules.move_cost(&g.map.tiles[&river]);
        assert_eq!(g.step_cost(flat, river), base + 2.0);
        // no surcharge moving off the river
        assert_eq!(g.step_cost(river, flat), g.rules.move_cost(&g.map.tiles[&flat]));
        // a unit with full MP may always take one step, even a 4-MP river+hill
        let mut g2 = teleport(&g, g.player_unit_ids(0)[0], flat);
        let uid = g2.player_unit_ids(0)[0];
        if g2.can_move(uid, river) {
            g2.apply(0, &Action::Move { unit: uid, to: river }).unwrap();
            assert_eq!(g2.units[&uid].moves_left, 0.0);
        }
    }

    #[test]
    fn zone_of_control_stops_movement() {
        let g = Game::new_full(2, 24, 16, 7, 60, 0, false);
        let me = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let foe = g.player_unit_ids(1).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        // park the enemy warrior two flat land tiles from ours
        let mypos = g.units[&me].pos;
        let spot = g.map.tiles.values()
            .find(|t| hex::distance(t.pos, mypos) == 2
                && !g.rules.is_water(t) && g.rules.is_passable(t) && !t.river
                && g.units_at(t.pos).is_empty() && g.city_at(t.pos).is_none()
                && hex::neighbors(t.pos).iter().any(|n| {
                    hex::distance(*n, mypos) == 1
                        && g.map.get(*n).map(|nt| !g.rules.is_water(nt)
                            && g.rules.is_passable(nt) && !nt.river
                            && g.units_at(*n).is_empty()).unwrap_or(false)
                }))
            .map(|t| t.pos).expect("open tile at distance 2");
        let mut g = teleport(&g, foe, spot);
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        let mid = *hex::neighbors(spot).iter()
            .find(|n| hex::distance(**n, mypos) == 1
                && g.map.get(**n).map(|nt| !g.rules.is_water(nt)
                    && g.rules.is_passable(nt) && !nt.river
                    && g.units_at(**n).is_empty()).unwrap_or(false))
            .unwrap();
        assert!(g.in_enemy_zoc(0, mid));
        g.apply(0, &Action::Move { unit: me, to: mid }).unwrap();
        assert!(g.units[&me].zoc_stopped);
        let out = hex::neighbors(mid).into_iter()
            .find(|n| *n != mypos && g.map.get(*n).is_some());
        if let Some(o) = out {
            assert!(g.apply(0, &Action::Move { unit: me, to: o }).is_err());
        }
    }

    #[test]
    fn wall_hp_pool() {
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // no walls: no strike, no wall pool
        assert_eq!(g.city_max_wall_hp(&g.cities[&cid]), 0);
        assert!(!g.city_can_strike(&g.cities[&cid]));
        let base_cs = g.city_strength(cid);
        g.cities.get_mut(&cid).unwrap().buildings.push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 50;
        assert_eq!(g.city_max_wall_hp(&g.cities[&cid]), 50);
        assert!(g.city_can_strike(&g.cities[&cid]));
        assert_eq!(g.city_strength(cid), base_cs + 3.0);
        // city ranged strength floors at 3 and tracks best ranged unit
        assert!(g.city_ranged_strength(cid) >= 3.0);
        // healthy walls absorb a melee attack: city keeps nearly all HP
        let cpos = g.cities[&cid].pos;
        let foe = g.player_unit_ids(1).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let adj = hex::neighbors(cpos).into_iter()
            .find(|n| g.map.get(*n).map(|t| !g.rules.is_water(t)
                && g.rules.is_passable(t)
                && g.units_at(*n).is_empty()).unwrap_or(false))
            .expect("open tile next to city");
        // clear the garrison so the melee attack targets the city itself
        let mine = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let far = g.map.tiles.values()
            .find(|t| hex::distance(t.pos, cpos) > 6 && !g.rules.is_water(t)
                && g.rules.is_passable(t) && g.units_at(t.pos).is_empty())
            .map(|t| t.pos).unwrap();
        let g = teleport(&g, mine, far);
        let mut g = teleport(&g, foe, adj);
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        g.apply(0, &Action::EndTurn).unwrap();
        let city_hp = g.cities[&cid].hp;
        g.apply(1, &Action::Attack { unit: foe, target: cpos }).unwrap();
        assert!(g.cities[&cid].wall_hp < 50); // walls took the hit
        assert!(g.cities[&cid].hp >= city_hp - 1); // city behind walls: 1 dmg
    }

    #[test]
    fn serialization_roundtrip() {
        let mut g = Game::new(2, 18, 12, 4, 25, 1);
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        let j = serde_json::to_value(&g).unwrap();
        let g2: Game = serde_json::from_value(j.clone()).unwrap();
        assert_eq!(serde_json::to_value(&g2).unwrap(), j);
    }

    #[test]
    fn action_protocol_json() {
        let a: Action = serde_json::from_str(
            r#"{"type": "move", "unit": 3, "to": [1, -2]}"#).unwrap();
        match a {
            Action::Move { unit, to } => {
                assert_eq!(unit, 3);
                assert_eq!(to, (1, -2));
            }
            _ => panic!("wrong variant"),
        }
        let e = serde_json::to_string(&Action::EndTurn).unwrap();
        assert_eq!(e, r#"{"type":"end_turn"}"#);
    }
}
