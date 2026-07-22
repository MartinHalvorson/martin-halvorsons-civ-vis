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
    fn world_wraps_east_west() {
        let g = Game::new_full(2, 20, 14, 5, 30, 0, false);
        let a = crate::hex::offset_to_axial(0, 4);
        let b = crate::hex::offset_to_axial(19, 4);
        assert_eq!(g.wdist(a, b), 1);
        assert!(g.nbrs(a).contains(&b));
        assert_eq!(crate::hex::canon((b.0 + 1, b.1), 20), a);
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
    fn policy_cards() {
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // chiefdom: 1 military + 1 economic slot
        g.players[0].civics.insert("code_of_laws".to_string());
        g.apply(0, &Action::Government { government: "chiefdom".to_string() }).unwrap();
        let base_prod = g.city_yields(cid).production;
        g.apply(0, &Action::SlotPolicy { policy: "urban_planning".to_string() }).unwrap();
        assert_eq!(g.city_yields(cid).production, base_prod + 1.0);
        // second economic card cannot fit (no wildcard slots in chiefdom)
        assert!(g.apply(0, &Action::SlotPolicy {
            policy: "god_king".to_string() }).is_err());
        // military slot still free
        g.apply(0, &Action::SlotPolicy { policy: "discipline".to_string() }).unwrap();
        // oligarchy has a wildcard slot: economic overflow fits there
        g.players[0].civics.insert("political_philosophy".to_string());
        g.apply(0, &Action::Government { government: "oligarchy".to_string() }).unwrap();
        g.apply(0, &Action::SlotPolicy { policy: "god_king".to_string() }).unwrap();
        assert_eq!(g.players[0].policies.len(), 3);
        // downgrading drops cards until the layout fits again
        g.apply(0, &Action::Government { government: "chiefdom".to_string() }).unwrap();
        assert!(g.players[0].policies.len() <= 2);
        // feudalism obsoletes agoge via feudal_contract
        g.players[0].civics.insert("craftsmanship".to_string());
        assert!(g.available_policies(0).iter().any(|c| c == "agoge"));
        g.players[0].civics.insert("feudalism".to_string());
        assert!(!g.available_policies(0).iter().any(|c| c == "agoge"));
        assert!(g.available_policies(0).iter().any(|c| c == "feudal_contract"));
    }

    /// Add a fresh unit of `kind` at `pos` for player 0 via save-edit.
    fn conjure(g: &Game, kind: &str, pos: crate::Pos) -> (Game, u32) {
        let mut v = serde_json::to_value(g).unwrap();
        let id = v["next_id"].as_u64().unwrap() as u32;
        v["next_id"] = serde_json::json!(id + 1);
        v["units"].as_array_mut().unwrap().push(serde_json::json!({
            "id": id, "type": kind, "owner": 0, "pos": [pos.0, pos.1],
            "hp": 100, "moves_left": 2.0, "charges": 0,
        }));
        (serde_json::from_value(v).unwrap(), id)
    }

    #[test]
    fn trade_routes_and_envoys() {
        let mut g = Game::new_full(2, 26, 16, 3, 200, 2, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cap = g.player_city_ids(0)[0];
        let cpos = g.cities[&cap].pos;
        // second own city 4+ tiles out for a domestic route
        let spot = g.map.tiles.values()
            .find(|t| {
                let d = g.wdist(t.pos, cpos);
                (4..=8).contains(&d) && !g.rules.is_water(t)
                    && g.rules.is_passable(t) && g.units_at(t.pos).is_empty()
                    && g.cities.values().all(|c| g.wdist(t.pos, c.pos) >= 4)
            })
            .map(|t| t.pos).expect("settle spot");
        let (g2, s2) = conjure(&g, "settler", spot);
        let mut g = g2;
        g.apply(0, &Action::FoundCity { unit: s2 }).unwrap();
        let second = *g.player_city_ids(0).iter().find(|c| **c != cap).unwrap();
        // trader + foreign trade civic → capacity 1
        g.players[0].civics.insert("foreign_trade".to_string());
        assert_eq!(g.trade_capacity(0), 1);
        let (mut g, trader) = conjure(&g, "trader", cpos);
        let before = g.city_yields(cap);
        g.apply(0, &Action::TradeRoute { unit: trader, city: second }).unwrap();
        assert_eq!(g.active_routes(0), 1);
        assert!(!g.units.contains_key(&trader)); // trader is on the road
        let after = g.city_yields(cap);
        // domestic city-center route: +1 food +1 production at the origin
        assert!((after.food - before.food - 1.0).abs() < 1e-9);
        assert!(after.production > before.production);
        // capacity is enforced
        let (mut g3, t2) = conjure(&g, "trader", cpos);
        assert!(g3.apply(0, &Action::TradeRoute { unit: t2, city: second }).is_err());
        // a road was laid toward the destination
        assert!(g.map.tiles.values().any(|t| t.road));
        // envoys: +2 of the type yield in the capital at 1 envoy
        let minor = g.players.iter()
            .find(|p| p.is_minor && !p.is_barbarian).expect("city-state").id;
        g.players[0].envoys_free = 1;
        let before = g.city_yields(cap);
        g.apply(0, &Action::SendEnvoy { player: minor }).unwrap();
        assert_eq!(g.envoys_at(0, minor), 1);
        let after = g.city_yields(cap);
        assert!((after.total() - before.total() - 2.0).abs() < 1e-6);
        // suzerain needs 6+ envoys and a strict lead
        assert_eq!(g.suzerain_of(minor), None);
        g.players[0].envoys[0].1 = 6;
        assert_eq!(g.suzerain_of(minor), Some(0));
        // routes expire and hand the trader back
        g.routes[0].ends = g.turn + 1;
        g.apply(0, &Action::EndTurn).unwrap();
        while g.current != 0 {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        assert_eq!(g.active_routes(0), 0);
        assert!(g.units.values().any(|u| u.owner == 0 && u.kind == "trader"));
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
