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
