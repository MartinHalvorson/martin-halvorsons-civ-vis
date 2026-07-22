//! Martin Halvorson's Civilization VIS — Rust performance core.
//! Same ruleset JSON, action protocol, and mechanics as the Python
//! reference engine (`civvis/`); deterministic per seed within this engine.
#![recursion_limit = "256"]

pub mod ai;
pub mod elo;
pub mod evolve;
pub mod game;
pub mod hex;
pub mod mapgen;
pub mod neural;
pub mod obs;
pub mod rng;
pub mod rules;
pub mod server;
pub mod valuenet;
pub mod world;

pub type Pos = (i32, i32);

#[cfg(test)]
mod tests {
    use crate::ai::{run_game, Ai, BasicAi};
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

    /// The game is exactly f(seed+params, action_log): re-applying the log of
    /// a finished game to a fresh engine reproduces it bit-for-bit. This is
    /// the desync detector — it fails if any failed apply consumes RNG or any
    /// rules path is nondeterministic.
    #[test]
    fn replay_from_action_log() {
        let mut g = Game::new(3, 24, 16, 9, 80, 2);
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        assert!(!g.log.is_empty());
        let mut r = Game::new(3, 24, 16, 9, 80, 2);
        for (i, (pid, a)) in g.log.iter().enumerate() {
            r.apply(*pid, a).unwrap_or_else(|e| {
                panic!("logged action {i} failed on replay: {e} ({a:?})")
            });
        }
        assert_eq!(serde_json::to_value(&g).unwrap(),
                   serde_json::to_value(&r).unwrap());
    }

    /// legal_actions must be exactly consistent with apply: everything it
    /// generates applies cleanly (spot-checked over the early game).
    #[test]
    fn legal_actions_all_apply() {
        let mut g = Game::new(2, 20, 14, 13, 40, 1);
        let mut ais = BasicAi::fleet(&g);
        while g.winner.is_none() && g.turn < 12 {
            let pid = g.current;
            for a in g.legal_actions(pid) {
                let snap = serde_json::to_value(&g).unwrap();
                let mut c: Game = serde_json::from_value(snap).unwrap();
                assert!(c.apply(pid, &a).is_ok(),
                        "legal action failed to apply: {a:?} (turn {})", g.turn);
            }
            ais[pid].take_turn(&mut g, pid);
            if g.winner.is_none() && g.current == pid {
                let _ = g.apply(pid, &Action::EndTurn);
            }
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
    fn long_range_route_detours_around_obstacles() {
        let mut g = Game::new_full(1, 20, 14, 17, 30, 0, false);
        let uid = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let start = *g.map.tiles.keys()
            .find(|p| g.wdisk(**p, 3).len() == 37).unwrap();
        let target = (start.0 + 3, start.1);

        // Make a controlled open field, then block every greedy move that
        // immediately reduces hex distance. A valid route must initially go
        // sideways or backward around this wedge.
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "plains".to_string();
            tile.feature = None;
        }
        let mut g = teleport(&g, uid, start);
        let direct: Vec<_> = g.nbrs(start).into_iter()
            .filter(|p| g.wdist(*p, target) < g.wdist(start, target))
            .collect();
        assert!(!direct.is_empty());
        for p in direct {
            g.map.tiles.get_mut(&p).unwrap().terrain = "mountain".to_string();
        }

        let step = g.route_step(uid, target, 0).expect("detour should exist");
        assert!(g.wdist(step, target) >= g.wdist(start, target));
        assert!(g.can_move(uid, step));
        g.apply(0, &Action::Move { unit: uid, to: step }).unwrap();
        assert_eq!(g.units[&uid].pos, step);
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

    #[test]
    fn civ6_starting_research_and_farms_need_no_agriculture_tech() {
        let mut g = Game::new_full(1, 20, 14, 29, 40, 0, false);

        // Civ VI Ancient starts know no technologies. The five first-column
        // technologies are immediately researchable; Agriculture is not a
        // technology in Civ VI and must not inflate score or era progress.
        assert!(g.players[0].techs.is_empty());
        assert!(!g.rules.techs.contains_key("agriculture"));
        let available: std::collections::BTreeSet<_> =
            g.available_techs(0).into_iter().collect();
        assert_eq!(available, [
            "animal_husbandry", "astrology", "mining", "pottery", "sailing",
        ].into_iter().map(str::to_string).collect());

        let settler = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let city_pos = g.cities[&cid].pos;
        let farm_pos = g.cities[&cid].owned_tiles.iter()
            .copied().find(|p| *p != city_pos).expect("city owns a ring tile");
        {
            let tile = g.map.tiles.get_mut(&farm_pos).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.hills = false;
        }
        assert!(g.valid_improvements(0, farm_pos).iter().any(|i| i == "farm"));

        // Conjure a builder on the controlled owned tile, then exercise the
        // real action path rather than only checking rules metadata.
        let mut saved = serde_json::to_value(&g).unwrap();
        let builder = saved["next_id"].as_u64().unwrap() as u32;
        saved["next_id"] = serde_json::json!(builder + 1);
        saved["units"].as_array_mut().unwrap().push(serde_json::json!({
            "id": builder, "type": "builder", "owner": 0,
            "pos": [farm_pos.0, farm_pos.1], "hp": 100,
            "moves_left": 2.0, "charges": 3,
        }));
        let mut g: Game = serde_json::from_value(saved).unwrap();
        let housing_before = g.city_housing(&g.cities[&cid]);
        g.apply(0, &Action::Improve {
            unit: builder,
            improvement: "farm".to_string(),
        }).unwrap();
        assert_eq!(g.map.tiles[&farm_pos].improvement.as_deref(), Some("farm"));
        assert_eq!(g.city_housing(&g.cities[&cid]), housing_before + 0.5);
        assert_eq!(g.rules.improvements["pasture"].housing, 0.5);
        assert_eq!(g.rules.improvements["plantation"].housing, 0.5);
        assert_eq!(g.rules.improvements["camp"].housing, 0.5);
        assert_eq!(g.rules.improvements["fishing_boats"].housing, 0.5);
        assert_eq!(g.rules.improvements["mine"].housing, 0.0);
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

    #[test]
    fn citizen_governor_meets_food_target_and_tracks_city_plan() {
        let mut g = Game::new_full(1, 20, 14, 41, 40, 0, false);
        let settler = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let center = g.cities[&cid].pos;
        let ring: Vec<_> = g.cities[&cid].owned_tiles.iter()
            .filter(|p| **p != center).copied().collect();
        assert!(ring.len() >= 4);

        // A controlled housing-capped city needs two food from its two worked
        // tiles. It also has a culture option and a production option.
        for pos in g.cities[&cid].owned_tiles.clone() {
            let tile = g.map.tiles.get_mut(&pos).unwrap();
            tile.terrain = "desert".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.hills = false;
            tile.river = false;
        }
        g.map.tiles.get_mut(&ring[0]).unwrap().terrain = "grassland".to_string();
        g.map.tiles.get_mut(&ring[1]).unwrap().hills = true;
        g.map.tiles.get_mut(&ring[2]).unwrap().resource = Some("silk".to_string());
        g.cities.get_mut(&cid).unwrap().pop = 2;

        // Every playable civilization contributes priorities through its
        // ruleset ability (city-states/custom civs retain the balanced base).
        g.players[0].civ = "Kabul".to_string();
        let balanced = g.citizen_strategy(cid).weights;
        for (civ, food, production, gold, science, culture) in [
            ("Rome", false, true, false, false, true),
            ("Egypt", false, true, true, false, false),
            ("Greece", false, false, false, false, true),
            ("China", false, true, false, true, true),
            ("Sumeria", false, true, true, false, false),
            ("Aztec", true, true, true, false, false),
            ("Nubia", true, true, false, false, false),
            ("Scythia", true, true, false, false, false),
        ] {
            g.players[0].civ = civ.to_string();
            let w = g.citizen_strategy(cid).weights;
            assert_eq!(w.food > balanced.food, food, "{civ} food priority");
            assert_eq!(w.production > balanced.production, production,
                       "{civ} production priority");
            assert_eq!(w.gold > balanced.gold, gold, "{civ} gold priority");
            assert_eq!(w.science > balanced.science, science,
                       "{civ} science priority");
            assert_eq!(w.culture > balanced.culture, culture,
                       "{civ} culture priority");
        }

        g.players[0].civ = "Greece".to_string();
        let greek = g.city_citizen_plan(cid);
        assert_eq!(greek.worked_tiles.len(), 2);
        let collected_food = 2.0 + greek.worked_tiles.iter()
            .map(|p| g.rules.tile_yields(&g.map.tiles[p]).food).sum::<f64>();
        assert!(collected_food + 1e-9 >= greek.strategy.food_target);
        assert!(greek.worked_tiles.contains(&ring[0]), "food safety tile not worked");
        assert!(greek.worked_tiles.contains(&ring[2]), "Greece should favor culture");

        // The identical city under Nubia keeps the food tile but switches its
        // discretionary citizen from culture to production.
        g.players[0].civ = "Nubia".to_string();
        let nubian = g.city_citizen_plan(cid);
        assert!(nubian.worked_tiles.contains(&ring[0]));
        assert!(nubian.worked_tiles.contains(&ring[1]), "Nubia should favor production");
        assert!(!nubian.worked_tiles.contains(&ring[2]));

        let before = g.citizen_strategy(cid);
        g.cities.get_mut(&cid).unwrap().queue.push(crate::game::Item::Building {
            building: "pyramids".to_string(),
        });
        let wonder = g.citizen_strategy(cid);
        assert_eq!(wonder.focus, "wonder");
        assert!(wonder.weights.production > before.weights.production);

        // Fixed food from infrastructure frees both citizens for strategic
        // jobs; the governor does not redundantly force the grassland tile.
        g.rules.buildings.get_mut("granary").unwrap().housing = 0.0;
        g.cities.get_mut(&cid).unwrap().buildings.push("granary".to_string());
        let fed_by_infrastructure = g.city_citizen_plan(cid);
        assert!(!fed_by_infrastructure.worked_tiles.contains(&ring[0]));
        assert!(fed_by_infrastructure.worked_tiles.contains(&ring[1]));

        let observed = crate::obs::observation(&g, 0);
        let city = observed["cities"].as_array().unwrap().iter()
            .find(|c| c["id"] == serde_json::json!(cid)).unwrap();
        assert_eq!(city["citizens"]["focus"], "wonder");
        assert_eq!(city["citizens"]["worked_tiles"].as_array().unwrap().len(), 2);
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
    fn religion() {
        let mut g = Game::new_full(2, 24, 16, 9, 200, 0, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // pantheon at 25 faith; beliefs are exclusive
        assert!(g.apply(0, &Action::ChoosePantheon {
            belief: "fertility_rites".to_string() }).is_err());
        g.players[0].faith = 30.0;
        g.apply(0, &Action::ChoosePantheon {
            belief: "fertility_rites".to_string() }).unwrap();
        assert!(g.apply(0, &Action::ChoosePantheon {
            belief: "divine_spark".to_string() }).is_err());
        // prophet + holy site founds a religion with exclusive beliefs
        let dpos = g.cities[&cid].owned_tiles.iter()
            .find(|p| **p != g.cities[&cid].pos).cloned().unwrap();
        g.cities.get_mut(&cid).unwrap().districts
            .insert("holy_site".to_string(), dpos);
        g.players[0].prophet_pending = true;
        g.apply(0, &Action::FoundReligion {
            follower: "choral_music".to_string(),
            founder: "tithe".to_string() }).unwrap();
        assert!(g.players[0].religion.is_some());
        // the holy city converts instantly
        assert_eq!(g.city_religion(&g.cities[&cid]),
                   g.players[0].religion.as_deref());
        // follower belief: +2 culture with a shrine in a following city
        let before = g.city_yields(cid).culture;
        g.cities.get_mut(&cid).unwrap().buildings.push("shrine".to_string());
        let after = g.city_yields(cid).culture;
        assert!((after - before - 2.0).abs() < 1e-9);
        // missionary spread converts a foreign city
        let religion = g.players[0].religion.clone().unwrap();
        let s1 = g.player_unit_ids(1).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        let mut g = {
            // second civ founds far away
            let far = g.map.tiles.values()
                .find(|t| g.wdist(t.pos, g.cities[&cid].pos) >= 6
                    && !g.rules.is_water(t) && g.rules.is_passable(t)
                    && g.units_at(t.pos).is_empty()
                    && g.cities.values().all(|c| g.wdist(t.pos, c.pos) >= 4))
                .map(|t| t.pos).unwrap();
            let mut v = serde_json::to_value(&g).unwrap();
            for u in v["units"].as_array_mut().unwrap() {
                if u["id"] == serde_json::json!(s1) {
                    u["pos"] = serde_json::json!([far.0, far.1]);
                }
            }
            let g2: Game = serde_json::from_value(v).unwrap();
            g2
        };
        g.apply(0, &Action::EndTurn).unwrap();
        g.apply(1, &Action::FoundCity { unit: s1 }).unwrap();
        let their = g.player_city_ids(1)[0];
        assert_eq!(g.city_religion(&g.cities[&their]), None);
        let (mut g, m) = conjure(&g, "missionary", g.cities[&their].pos);
        g.units.get_mut(&m).unwrap().charges = 3;
        g.apply(1, &Action::EndTurn).unwrap();
        g.apply(0, &Action::Spread { unit: m }).unwrap();
        assert_eq!(g.city_religion(&g.cities[&their]), Some(religion.as_str()));
        // every major majority-following → religious victory
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("religious"));
    }

    #[test]
    fn great_people() {
        let mut g = Game::new_full(2, 24, 16, 9, 200, 0, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let dpos = g.cities[&cid].owned_tiles.iter()
            .find(|p| **p != g.cities[&cid].pos).cloned().unwrap();
        g.cities.get_mut(&cid).unwrap().districts
            .insert("campus".to_string(), dpos);
        g.cities.get_mut(&cid).unwrap().buildings.push("library".to_string());
        // campus (+1) + library (+1) scientist points per turn
        let round = |g: &mut Game| {
            g.apply(0, &Action::EndTurn).unwrap();
            while g.current != 0 {
                let cur = g.current;
                g.apply(cur, &Action::EndTurn).unwrap();
            }
        };
        round(&mut g);
        let pts = *g.players[0].gpp.get("scientist").unwrap();
        assert!((pts - 2.0).abs() < 1e-9, "expected 2 scientist gpp, got {pts}");
        assert_eq!(g.gp_cost(0, "scientist"), 60.0);
        // reaching the threshold auto-claims and grants two eurekas
        g.players[0].gpp.insert("scientist".to_string(), 59.0);
        let boosts_before = g.players[0].boosted_techs.len();
        round(&mut g);
        assert_eq!(g.players[0].gp_claimed.get("scientist"), Some(&1));
        assert!(g.players[0].boosted_techs.len() >= boosts_before + 1);
        // next scientist costs double
        assert_eq!(g.gp_cost(0, "scientist"), 120.0);
    }

    #[test]
    fn eras_and_culture_victory() {
        let mut g = Game::new_full(2, 20, 14, 5, 300, 0, false);
        assert_eq!(g.world_era, 0);
        // push the leader past the classical threshold with a big era score
        for t in ["pottery", "mining", "sailing", "astrology", "irrigation",
                  "archery", "writing", "masonry", "bronze_working",
                  "animal_husbandry", "horseback_riding", "currency"] {
            g.players[0].techs.insert(t.to_string());
        }
        g.players[0].era_score = 20;
        g.players[1].era_score = 0;
        let round = |g: &mut Game| {
            g.apply(0, &Action::EndTurn).unwrap();
            while g.current != 0 {
                let cur = g.current;
                g.apply(cur, &Action::EndTurn).unwrap();
            }
        };
        round(&mut g);
        assert_eq!(g.world_era, 1);
        assert_eq!(g.players[0].age, "golden");
        assert_eq!(g.players[1].age, "dark");
        assert_eq!(g.players[0].era_score, 0);
        // culture victory: overwhelming accumulated tourism wins at wrap
        g.players[0].tourism_lifetime = 100000.0;
        round(&mut g);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("culture"));
    }

    #[test]
    fn natural_wonders_and_support_units() {
        let g = Game::new_full(2, 26, 16, 3, 60, 0, false);
        // wonders generate and impassable ones block movement
        let nw: Vec<_> = g.map.tiles.values()
            .filter(|t| t.feature.as_deref()
                .map(|f| g.rules.features[f].natural_wonder).unwrap_or(false))
            .collect();
        assert!(!nw.is_empty(), "no natural wonders generated");
        if let Some(t) = g.map.tiles.values()
            .find(|t| t.feature.as_deref() == Some("crater_lake")) {
            assert!(!g.rules.is_passable(t));
        }
        // battering ram lets melee hit ancient walls at full strength
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let cpos = g.cities[&cid].pos;
        g.cities.get_mut(&cid).unwrap().buildings.push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 50;
        let mine = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let far = g.map.tiles.values()
            .find(|t| g.wdist(t.pos, cpos) > 6 && !g.rules.is_water(t)
                && g.rules.is_passable(t) && g.units_at(t.pos).is_empty())
            .map(|t| t.pos).unwrap();
        let g2 = teleport(&g, mine, far);
        let adj = crate::hex::neighbors(cpos).into_iter()
            .find(|n| g2.map.get(*n).map(|t| !g2.rules.is_water(t)
                && g2.rules.is_passable(t)
                && g2.units_at(*n).is_empty()).unwrap_or(false)).unwrap();
        let foe = g2.player_unit_ids(1).into_iter()
            .find(|id| g2.units[id].kind == "warrior").unwrap();
        let g3 = teleport(&g2, foe, adj);
        let (mut g4, _ram) = {
            // conjure an enemy battering ram sharing the attacker's tile
            let mut v = serde_json::to_value(&g3).unwrap();
            let id = v["next_id"].as_u64().unwrap() as u32;
            v["next_id"] = serde_json::json!(id + 1);
            v["units"].as_array_mut().unwrap().push(serde_json::json!({
                "id": id, "type": "battering_ram", "owner": 1,
                "pos": [adj.0, adj.1], "hp": 100, "moves_left": 2.0, "charges": 0,
            }));
            (serde_json::from_value::<Game>(v).unwrap(), id)
        };
        g4.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        g4.apply(0, &Action::EndTurn).unwrap();
        g4.apply(1, &Action::Attack { unit: foe, target: cpos }).unwrap();
        // full-strength wall damage (>= 8) instead of the 15% trickle (<= 6)
        assert!(50 - g4.cities[&cid].wall_hp >= 8,
                "ram should breach: wall_hp {}", g4.cities[&cid].wall_hp);
    }

    #[test]
    fn loyalty_governors_congress() {
        let mut g = Game::new_full(2, 26, 16, 9, 300, 1, false);
        let s = g.player_unit_ids(0).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        assert_eq!(g.cities[&cid].loyalty, 100.0);
        // governor titles come from civic milestones
        assert_eq!(g.governor_titles(0), 0);
        g.players[0].civics.insert("political_philosophy".to_string());
        assert_eq!(g.governor_titles(0), 1);
        g.apply(0, &Action::AssignGovernor { city: cid }).unwrap();
        assert!(g.apply(0, &Action::AssignGovernor { city: cid }).is_err());
        // amenity bonus from the governor
        assert!(g.players[0].governors.contains(&cid));
        // world congress: medieval era, turn multiple of 30, most envoys wins
        g.world_era = 2;
        g.turn = 29; // wraps to 30 after a full round
        let minor = g.players.iter().find(|p| p.is_minor && !p.is_barbarian)
            .map(|p| p.id).unwrap();
        g.players[0].envoys = vec![(minor, 3)];
        g.apply(0, &Action::EndTurn).unwrap();
        while g.current != 0 {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        assert_eq!(g.players[0].dvp, 2);
        // 20 points = diplomatic victory at the next congress
        g.players[0].dvp = 18;
        g.turn = 59;
        g.apply(0, &Action::EndTurn).unwrap();
        while g.winner.is_none() && g.current != 0 {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("diplomatic"));
    }

    #[test]
    fn leaders_present_and_uniques_gated() {
        let g = Game::new_full(8, 40, 24, 3, 60, 0, false);
        // every playable civ has a leader and ability defined
        for name in crate::game::CIV_NAMES {
            let spec = g.rules.civs.get(name)
                .unwrap_or_else(|| panic!("no leader data for {name}"));
            assert!(!spec.leader.is_empty());
            assert!(!spec.ability.is_empty());
            if let Some(uu) = &spec.unique_unit {
                let us = &g.rules.units[uu.as_str()];
                assert_eq!(us.unique_to.as_deref(), Some(name),
                           "{uu} unique_to mismatch");
            }
        }
        // seats map to civs in order: 0 Rome .. 7 Scythia
        for (i, name) in crate::game::CIV_NAMES.iter().enumerate() {
            assert_eq!(&g.players[i].civ, name);
        }
        // unique units: only their civ builds them; the base is blocked
        let mut g = g;
        let greece = 2;
        let s = g.player_unit_ids(greece).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        // clear the current player gate for direct checks
        g.players[greece].techs.insert("bronze_working".to_string());
        while g.current != greece {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        g.apply(greece, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(greece)[0];
        use crate::game::Item;
        assert!(g.can_produce(greece, cid,
            &Item::Unit { unit: "hoplite".to_string() }));
        assert!(!g.can_produce(greece, cid,
            &Item::Unit { unit: "spearman".to_string() }));
        assert!(!g.can_produce(greece, cid,
            &Item::Unit { unit: "legion".to_string() }));
        // Greece: Plato's Republic grants an extra wildcard slot
        g.players[greece].civics.insert("code_of_laws".to_string());
        g.apply(greece, &Action::Government {
            government: "chiefdom".to_string() }).unwrap();
        let slots = g.gov_slots(greece);
        assert_eq!(slots.wildcard, 1); // chiefdom normally has none
        // China: Dynastic Cycle boosts give 50%, builders +1 charge
        let china = 3;
        assert!(g.has_ability(china, "dynastic_cycle"));
        g.players[china].boosted_techs.insert("pottery".to_string());
        while g.current != china {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        g.apply(china, &Action::Research { tech: "pottery".to_string() }).unwrap();
        let cost = g.rules.techs["pottery"].cost;
        assert!((g.players[china].research_progress - 0.5 * cost).abs() < 1e-9);
        // Rome: founded cities start with a free monument
        let rome = 0;
        let s = g.player_unit_ids(rome).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        while g.current != rome {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        g.apply(rome, &Action::FoundCity { unit: s }).unwrap();
        let rc = g.player_city_ids(rome)[0];
        assert!(g.cities[&rc].buildings.iter().any(|b| b == "monument"));
    }

    #[test]
    fn domination_victory() {
        let mut g = Game::new_full(2, 20, 14, 5, 300, 0, false);
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        // eliminate player 1 in open combat: seize their last settler
        let settler = g.player_unit_ids(1).into_iter()
            .find(|id| g.units[id].kind == "settler").unwrap();
        let spos = g.units[&settler].pos;
        // park their escort far away so the capture is uncontested
        let foe_w = g.player_unit_ids(1).into_iter()
            .find(|id| g.units[id].kind == "warrior").unwrap();
        let far = g.map.tiles.values()
            .find(|t| g.wdist(t.pos, spos) > 8 && !g.rules.is_water(t)
                && g.rules.is_passable(t) && g.units_at(t.pos).is_empty())
            .map(|t| t.pos).unwrap();
        let g2 = teleport(&g, foe_w, far);
        let mine = g2.player_unit_ids(0).into_iter()
            .find(|id| g2.units[id].kind == "warrior").unwrap();
        let adj = crate::hex::neighbors(spos).into_iter()
            .find(|n| g2.map.get(*n).map(|t| !g2.rules.is_water(t)
                && g2.rules.is_passable(t)
                && g2.units_at(*n).is_empty()).unwrap_or(false)).unwrap();
        let mut g3 = teleport(&g2, mine, adj);
        g3.apply(0, &Action::Move { unit: mine, to: spos }).unwrap();
        assert!(!g3.players[1].alive);
        assert_eq!(g3.winner, Some(0));
        assert_eq!(g3.victory_type.as_deref(), Some("domination"));
    }

    #[test]
    fn all_leaders_full_game() {
        // all 8 leaders in one headless game, played to a decision
        let mut g = Game::new(8, 40, 24, 21, 150, 2);
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        assert!(g.winner.is_some());
        assert!(g.victory_type.is_some());
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
