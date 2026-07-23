//! Martin Halvorson's Civilization VIS — Rust performance core.
//! Same ruleset JSON, action protocol, and mechanics as the Python
//! reference engine (`civvis/`); deterministic per seed within this engine.
#![recursion_limit = "512"]

pub mod action_space;
pub mod ai;
pub mod belief;
pub mod elo;
pub mod evolve;
pub mod game;
pub mod hex;
pub mod mapgen;
pub mod neural;
pub mod obs;
pub mod obs_tensor;
pub mod policy;
pub mod rng;
pub mod rules;
pub mod selfplay;
pub mod server;
pub mod setup;
pub mod strategic;
pub mod valuenet;
pub mod mods;
pub mod pedia;
pub mod validate;
pub mod world;

pub type Pos = (i32, i32);

#[cfg(test)]
mod tests {
    use crate::ai::{run_game, Ai, BasicAi};
    use crate::game::{Action, Game, GameOptions};
    use crate::hex;
    use crate::rules::Rules;
    use std::collections::BTreeSet;

    fn options(difficulty: &str, speed: &str, human_seats: &[usize]) -> GameOptions {
        GameOptions {
            difficulty: difficulty.to_string(),
            speed: speed.to_string(),
            human_seats: human_seats.iter().copied().collect::<BTreeSet<_>>(),
            barbarians: false,
            ..GameOptions::new(2, 20, 14, 7, 40, 0)
        }
    }

    /// The ladder is a contiguous run from Settler to Deity, Prince is the
    /// level that hands out nothing, and the handicaps only ever grow.
    #[test]
    fn the_difficulty_ladder_is_ordered_and_neutral_at_prince() {
        let rules = Rules::embedded();
        let mut levels: Vec<_> = rules.difficulties.values().collect();
        levels.sort_by_key(|spec| spec.order);
        assert_eq!(levels.len(), 8);
        assert!(levels.iter().enumerate().all(|(i, spec)| spec.order == i));
        let prince = &rules.difficulties["prince"];
        assert_eq!(prince.order, 3);
        assert_eq!(prince.ai_combat_strength, 0.0);
        assert_eq!(prince.human_combat_strength, 0.0);
        assert_eq!(prince.ai_yield_pct.science, 0.0);
        assert!(prince.ai_bonus_units.is_empty());
        for pair in levels.windows(2) {
            let (lower, higher) = (pair[0], pair[1]);
            assert!(higher.ai_yield_pct.science >= lower.ai_yield_pct.science);
            assert!(higher.ai_xp_pct >= lower.ai_xp_pct);
            assert!(higher.human_combat_strength <= lower.human_combat_strength);
        }
    }

    /// Above Prince the handicaps land on the AI seats: better yields, a
    /// stronger army, and extra units already on the map at turn one.
    #[test]
    fn deity_hands_its_bonuses_to_the_ai_seats() {
        let deity = Game::new_with(options("deity", "standard", &[0]));
        let prince = Game::new_with(options("prince", "standard", &[0]));
        // Seat 0 is the human and is untouched; seat 1 is an AI major.
        assert_eq!(deity.handicap_combat_strength(0), 0.0);
        assert_eq!(deity.handicap_combat_strength(1), 3.0);
        assert_eq!(deity.handicap_yield_pct(0).science, 0.0);
        assert_eq!(deity.handicap_yield_pct(1).production, 80.0);
        assert_eq!(deity.handicap_xp_pct(1), 40.0);
        let extra_units = |g: &Game, pid: usize| g.player_unit_ids(pid).len();
        assert_eq!(extra_units(&prince, 1), extra_units(&prince, 0));
        assert_eq!(
            extra_units(&deity, 1),
            extra_units(&deity, 0) + 7, // 4 warriors, 2 builders, a settler
        );
    }

    /// Below Prince the same machinery runs the other way, and the bonuses
    /// reach the person at the keyboard instead.
    #[test]
    fn settler_hands_its_bonuses_to_the_human_seat() {
        let g = Game::new_with(options("settler", "standard", &[0]));
        assert_eq!(g.handicap_combat_strength(0), 3.0);
        assert_eq!(g.handicap_xp_pct(0), 45.0);
        assert_eq!(g.handicap_combat_strength(1), 0.0);
        assert_eq!(g.handicap_yield_pct(1), Default::default());
        // With no seat declared human, a headless game stays neutral.
        let headless = Game::new_with(options("settler", "standard", &[]));
        assert_eq!(headless.handicap_combat_strength(0), 0.0);
    }

    /// A difficulty bonus reaches the yields a city actually reports, and the
    /// strength an opponent actually has to fight through.
    #[test]
    fn handicaps_reach_city_yields_and_unit_strength() {
        let mut deity = Game::new_with(options("deity", "standard", &[0]));
        let mut prince = Game::new_with(options("prince", "standard", &[0]));
        for game in [&mut deity, &mut prince] {
            for pid in 0..2 {
                let settler = game
                    .player_unit_ids(pid)
                    .into_iter()
                    .find(|uid| game.units[uid].kind == "settler")
                    .unwrap();
                let pos = game.units[&settler].pos;
                game.found_city_for(pid, pos, None);
            }
        }
        let ai_city = |g: &Game| {
            let cid = g.player_city_ids(1)[0];
            g.city_yields(cid)
        };
        let (boosted, stock) = (ai_city(&deity), ai_city(&prince));
        assert!((boosted.production - stock.production * 1.8).abs() < 1e-6);
        assert!((boosted.science - stock.science * 1.32).abs() < 1e-6);
        assert_eq!(boosted.food, stock.food, "growth stays honest");
        let warrior = |g: &Game, pid: usize| {
            let uid = g
                .player_unit_ids(pid)
                .into_iter()
                .find(|uid| g.units[uid].kind == "warrior")
                .unwrap();
            g.unit_strength(&g.units[&uid], false)
        };
        assert_eq!(warrior(&deity, 1) - warrior(&prince, 1), 3.0);
        assert_eq!(warrior(&deity, 0), warrior(&prince, 0));
    }

    /// Speed scales everything bought with a stockpiled yield, and brings its
    /// own turn budget from the shipped turn-length tables.
    #[test]
    fn game_speed_scales_every_cost() {
        let marathon = Game::new_with(options("prince", "marathon", &[]));
        let standard = Game::new_with(options("prince", "standard", &[]));
        let online = Game::new_with(options("prince", "online", &[]));
        assert_eq!(marathon.speed_cost_mult(), 3.0);
        assert_eq!(online.speed_cost_mult(), 0.5);
        let item = crate::game::Item::Unit {
            unit: "warrior".to_string(),
        };
        assert_eq!(
            marathon.item_cost(&item),
            standard.item_cost(&item) * 3.0
        );
        assert_eq!(marathon.tech_cost("mining"), standard.tech_cost("mining") * 3.0);
        assert_eq!(
            marathon.civic_cost("code_of_laws"),
            standard.civic_cost("code_of_laws") * 3.0
        );
        let rules = Rules::embedded();
        assert_eq!(rules.speeds["marathon"].turns, 1500);
        assert_eq!(rules.speeds["standard"].turns, 500);
    }

    /// A civilization's event stream records what happened to it, is visible
    /// only to it, and reaches the observation an agent or the GUI reads.
    #[test]
    fn the_event_stream_records_what_happened_to_each_civilization() {
        let mut g = Game::new_with(GameOptions {
            barbarians: false,
            ..GameOptions::new(2, 20, 14, 11, 60, 0)
        });
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        let pos = g.units[&settler].pos;
        g.found_city_for(0, pos, Some("Testopolis".to_string()));
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();

        let mine = g.events_for(0);
        assert!(mine
            .iter()
            .any(|e| e.category == "Cities" && e.text.contains("Testopolis") && e.pos == Some(pos)));
        let (aggressor, defender) = (g.players[0].civ.clone(), g.players[1].civ.clone());
        let declaration = format!("{aggressor} declared war on {defender}");
        assert!(mine
            .iter()
            .any(|e| e.category == "War" && e.text == declaration));
        // The other side hears about the war, and about nothing else of ours.
        let theirs = g.events_for(1);
        assert!(theirs
            .iter()
            .any(|e| e.category == "War" && e.text == declaration));
        assert!(g.events.iter().all(|event| {
            !event
                .text
                .split(|character: char| !character.is_alphabetic())
                .any(|word| word.eq_ignore_ascii_case("you") || word.eq_ignore_ascii_case("your"))
        }));
        assert!(!theirs.iter().any(|e| e.text.contains("Testopolis")));

        let observed = crate::obs::observation(&g, 0);
        let events = observed["events"].as_array().unwrap();
        assert!(events
            .iter()
            .any(|e| e["text"].as_str().unwrap().contains("Testopolis")));
        // Events are part of the game state, so they survive a save.
        let restored: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(restored.events_for(0).len(), mine.len());
    }

    /// Every leader carries the agenda and preference traits the shipped
    /// leader data assigns them.
    #[test]
    fn every_leader_has_their_historical_agenda() {
        let rules = Rules::embedded();
        let expected = [
            ("Rome", "optimus_princeps", "expansionist"),
            ("Egypt", "queen_of_the_nile", "mediterranean"),
            ("Greece", "delian_league", "pursue_diplomatic_victory"),
            ("China", "wonder_obsessed", "cultural_major_civ"),
            ("Sumeria", "ally_of_enkidu", "science_major_civ"),
            ("Aztec", "tlatoani", "aggressive_military"),
            ("Nubia", "city_planner", "science_major_civ"),
            ("Scythia", "backstab_averse", "killer_of_cyrus"),
        ];
        for (civ, agenda, leader_trait) in expected {
            let spec = &rules.civs[civ];
            assert_eq!(spec.agenda.as_deref(), Some(agenda), "{civ}");
            assert!(spec.traits.iter().any(|t| t == leader_trait), "{civ}");
            assert!(rules.agendas.contains_key(agenda), "{agenda}");
        }
    }

    /// A comparative agenda weighs a rival against the rest of the world:
    /// Trajan thinks well of a sprawling empire and poorly of a small one.
    #[test]
    fn a_comparative_agenda_scores_rivals_against_the_world() {
        let mut g = Game::new_with(GameOptions {
            barbarians: false,
            ..GameOptions::new(3, 26, 18, 21, 60, 0)
        });
        assert_eq!(g.players[0].civ, "Rome");
        for pid in 0..3 {
            let settler = g
                .player_unit_ids(pid)
                .into_iter()
                .find(|uid| g.units[uid].kind == "settler")
                .unwrap();
            let pos = g.units[&settler].pos;
            g.found_city_for(pid, pos, None);
        }
        // Give seat 1 a second city; seat 2 keeps one. Trajan should now
        // prefer the larger realm to the smaller.
        let spare = g.map.tiles.keys().copied().find(|pos| {
            g.map.tiles[pos].owner_city.is_none()
                && !g.rules.is_water(&g.map.tiles[pos])
                && g.cities.values().all(|c| g.wdist(*pos, c.pos) > 4)
        });
        g.found_city_for(1, spare.expect("an empty tile"), None);
        let big = g.agenda_opinion(0, 1);
        let small = g.agenda_opinion(0, 2);
        assert!(big > small, "Trajan preferred {small} to {big}");
        assert!(big > 0.0 && small < 0.0);
        assert!((-30.0..=30.0).contains(&big));
        // A leader has no opinion of themselves, of minors, or of barbarians.
        assert_eq!(g.agenda_opinion(0, 0), 0.0);
    }

    /// A relational agenda ignores the rest of the world: Tomyris judges a
    /// rival by their conduct towards her and their reputation elsewhere.
    #[test]
    fn a_relational_agenda_scores_conduct_not_size() {
        let mut g = Game::new_with(GameOptions {
            barbarians: false,
            ..GameOptions::new(8, 44, 26, 31, 60, 0)
        });
        let tomyris = g
            .players
            .iter()
            .position(|player| player.civ == "Scythia")
            .expect("Scythia is one of the eight");
        // Seat 0 opens the game, and declaring war is only legal on your own
        // turn, so the aggressor has to be the seat holding it.
        let treacherous = g.current;
        assert_ne!(treacherous, tomyris);
        let honest = (0..8).find(|pid| *pid != tomyris && *pid != treacherous).unwrap();
        assert_eq!(g.agenda_opinion(tomyris, honest), 0.0, "no reason to judge");
        // Declaring war piles grievances on the aggressor, which is exactly
        // the reputation her agenda punishes.
        g.apply(treacherous, &Action::DeclareWar { player: honest })
            .unwrap();
        assert!(
            g.agenda_opinion(tomyris, treacherous) < -10.0,
            "Tomyris shrugged at a surprise war"
        );
        assert_eq!(g.agenda_opinion(tomyris, honest), 0.0);
    }

    /// Stances reach the players through the event stream, once each, when
    /// they change — and reach the observation an agent reads.
    #[test]
    fn agenda_stances_are_announced_when_they_change() {
        let mut g = Game::new_with(GameOptions {
            barbarians: false,
            ..GameOptions::new(8, 44, 26, 32, 60, 0)
        });
        let tomyris = g
            .players
            .iter()
            .position(|player| player.civ == "Scythia")
            .unwrap();
        let (a, b) = (
            (0..8).find(|pid| *pid != tomyris).unwrap(),
            (0..8)
                .filter(|pid| *pid != tomyris)
                .nth(1)
                .unwrap(),
        );
        g.apply(a, &Action::DeclareWar { player: b }).unwrap();
        // Run a full world turn so the upkeep pass sees the new stance.
        for _ in 0..(g.players.len() + 1) {
            let current = g.current;
            let _ = g.apply(current, &Action::EndTurn);
        }
        let disapproval: Vec<&str> = g
            .events_for(a)
            .iter()
            .filter(|event| event.category == "Diplomacy")
            .map(|event| event.text.as_str())
            .collect();
        assert!(
            disapproval.iter().any(|text| {
                text.contains(&format!("Scythia disapproves of {}", g.players[a].civ))
            }),
            "no disapproval reached the aggressor: {disapproval:?}"
        );
        // Repeating the pass says nothing new.
        let before = g.events_for(a).len();
        for _ in 0..(g.players.len() + 1) {
            let current = g.current;
            let _ = g.apply(current, &Action::EndTurn);
        }
        let repeats = g
            .events_for(a)
            .iter()
            .filter(|event| {
                event
                    .text
                    .contains(&format!("Scythia disapproves of {}", g.players[a].civ))
            })
            .count();
        assert_eq!(repeats, 1, "a settled stance was announced twice");
        assert!(g.events_for(a).len() >= before);

        let observed = crate::obs::observation(&g, a);
        let seats = observed["players"].as_array().unwrap();
        let scythia = seats
            .iter()
            .find(|seat| seat["civ"] == "Scythia")
            .expect("Scythia is visible in the ribbon");
        assert_eq!(scythia["agenda"]["name"], "Backstab Averse");
        assert!(scythia["opinion_of_me"].as_f64().unwrap() < 0.0);
    }

    /// The setup choices are part of the game, so they survive a save.
    #[test]
    fn difficulty_and_speed_survive_a_save_round_trip() {
        let g = Game::new_with(options("immortal", "epic", &[0]));
        let restored: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(restored.difficulty, "immortal");
        assert_eq!(restored.speed, "epic");
        assert_eq!(restored.human_seats, BTreeSet::from([0]));
        assert_eq!(restored.handicap_combat_strength(1), 2.0);
        // Saves written before difficulty existed still load, at the stock level.
        let mut raw: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        for key in ["difficulty", "speed", "human_seats"] {
            raw.as_object_mut().unwrap().remove(key);
        }
        let legacy: Game = serde_json::from_value(raw).unwrap();
        assert_eq!(legacy.difficulty, "prince");
        // The typed speed field predates the compatibility ruleset string and
        // remains authoritative when that string is absent.
        assert_eq!(legacy.speed, "epic");
        assert!(legacy.human_seats.is_empty());
    }

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
            r.apply(*pid, a)
                .unwrap_or_else(|e| panic!("logged action {i} failed on replay: {e} ({a:?})"));
        }
        assert_eq!(
            serde_json::to_value(&g).unwrap(),
            serde_json::to_value(&r).unwrap()
        );
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
                assert!(
                    c.apply(pid, &a).is_ok(),
                    "legal action failed to apply: {a:?} (turn {})",
                    g.turn
                );
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
                assert!(
                    p.techs.len() > 1
                        || (p.is_minor && p.research.is_some() && p.research_progress > 0.0),
                    "player {} ({}) alive={} cities={} ended with {:?}, research {:?} at {:.1}",
                    p.id,
                    p.civ,
                    p.alive,
                    g.player_city_ids(p.id).len(),
                    p.techs,
                    p.research,
                    p.research_progress
                );
            }
        }
    }

    /// A crowded spawn used to drop its city-state on the floor, so a request
    /// for twelve could seat as few as four and the game played nothing like
    /// the one that was asked for. Every seat also needs a distinct identity:
    /// the roster is indexed by seat, not by spawn.
    #[test]
    fn every_requested_city_state_is_seated_once() {
        for seed in 7_300..7_312 {
            let g = Game::new(8, 40, 24, seed, 5, 12);
            let minors: Vec<&str> = g
                .players
                .iter()
                .filter(|p| p.is_minor && !p.is_barbarian)
                .map(|p| p.civ.as_str())
                .collect();
            assert_eq!(minors.len(), 12, "seed {seed} seated {minors:?}");
            let distinct: std::collections::BTreeSet<&str> = minors.iter().copied().collect();
            assert_eq!(distinct.len(), minors.len(), "seed {seed} repeats a name");
            // Stock Civ VI keeps starts four tiles apart.
            let capitals: Vec<crate::Pos> = g.cities.values().map(|c| c.pos).collect();
            for (i, a) in capitals.iter().enumerate() {
                for b in &capitals[i + 1..] {
                    assert!(g.wdist(*a, *b) >= 4, "seed {seed} crowds {a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn city_states_stay_single() {
        let mut g = Game::new(2, 28, 18, 2, 50, 3);
        let minors: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.is_minor && !p.is_barbarian)
            .map(|p| p.id)
            .collect();
        assert!(!minors.is_empty());
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        for pid in minors {
            let founded = g
                .cities
                .values()
                .filter(|c| c.original_owner == pid)
                .count();
            assert_eq!(founded, 1);
        }
    }

    #[test]
    fn movement_range_and_move_to() {
        let mut g = Game::new_full(2, 20, 14, 5, 60, 0, false);
        let uid = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let reach = g.reachable(uid);
        assert!(!reach.is_empty());
        let start = g.units[&uid].pos;
        let far = *reach
            .iter()
            .max_by_key(|p| crate::hex::distance(start, **p))
            .unwrap();
        g.apply(0, &Action::MoveTo { unit: uid, to: far }).unwrap();
        assert_eq!(g.units[&uid].pos, far);
    }

    #[test]
    fn long_range_route_detours_around_obstacles() {
        let mut g = Game::new_full(1, 20, 14, 17, 30, 0, false);
        let uid = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let start = *g
            .map
            .tiles
            .keys()
            .find(|p| g.wdisk(**p, 3).len() == 37)
            .unwrap();
        let target = (start.0 + 3, start.1);

        // Make a controlled open field, then block every greedy move that
        // immediately reduces hex distance. A valid route must initially go
        // sideways or backward around this wedge.
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "plains".to_string();
            tile.feature = None;
        }
        let mut g = teleport(&g, uid, start);
        let direct: Vec<_> = g
            .nbrs(start)
            .into_iter()
            .filter(|p| g.wdist(*p, target) < g.wdist(start, target))
            .collect();
        assert!(!direct.is_empty());
        for p in direct {
            g.map.tiles.get_mut(&p).unwrap().terrain = "mountain".to_string();
        }

        let step = g.route_step(uid, target, 0).expect("detour should exist");
        assert!(g.wdist(step, target) >= g.wdist(start, target));
        assert!(g.can_move(uid, step));
        g.apply(
            0,
            &Action::Move {
                unit: uid,
                to: step,
            },
        )
        .unwrap();
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
        assert!(g.map.tiles.values().any(|t| t.has_river()));
        // embark gated on shipbuilding
        let mut g2 = Game::new_full(2, 24, 16, 3, 60, 0, false);
        let uid = g2.player_unit_ids(0)[0];
        let coast = g2
            .map
            .tiles
            .values()
            .find(|t| {
                t.terrain == "coast"
                    && crate::hex::distance(t.pos, g2.units[&uid].pos) == 1
                    && g2.units_at(t.pos).is_empty()
            })
            .map(|t| t.pos);
        if let Some(c) = coast {
            // This assertion isolates embarkation. A generated cliff on the
            // same coast is an independent, valid reason movement can fail.
            let from = g2.units[&uid].pos;
            g2.map.tiles.get_mut(&from).unwrap().cliff_edges = [false; 6];
            g2.map.tiles.get_mut(&c).unwrap().cliff_edges = [false; 6];
            assert!(!g2.can_move(uid, c));
            g2.players[0].techs.insert("shipbuilding".to_string());
            assert!(g2.can_move(uid, c));
        }
        // wonders are world-unique
        assert!(g2.rules.wonders.contains_key("pyramids"));
        let cid = {
            let s = g2
                .player_unit_ids(0)
                .into_iter()
                .find(|id| g2.units[id].kind == "settler")
                .unwrap();
            g2.apply(0, &Action::FoundCity { unit: s }).unwrap();
            g2.player_city_ids(0)[0]
        };
        let wonder_pos = g2.cities[&cid].owned_tiles[1];
        g2.cities
            .get_mut(&cid)
            .unwrap()
            .wonders
            .insert("pyramids".to_string(), wonder_pos);
        g2.map.tiles.get_mut(&wonder_pos).unwrap().wonder = Some("pyramids".to_string());
        assert!(g2.wonder_built("pyramids"));
        g2.players[0].techs.insert("masonry".to_string());
        assert!(!g2.can_produce(
            0,
            cid,
            &crate::game::Item::Wonder {
                wonder: "pyramids".to_string(),
                pos: wonder_pos,
            }
        ));
    }

    #[test]
    fn civ6_starting_research_and_farms_need_no_agriculture_tech() {
        let mut g = Game::new_full(1, 20, 14, 29, 40, 0, false);

        // Civ VI Ancient starts know no technologies. The five first-column
        // technologies are immediately researchable; Agriculture is not a
        // technology in Civ VI and must not inflate score or era progress.
        assert!(g.players[0].techs.is_empty());
        assert!(!g.rules.techs.contains_key("agriculture"));
        let available: std::collections::BTreeSet<_> = g.available_techs(0).into_iter().collect();
        assert_eq!(
            available,
            [
                "animal_husbandry",
                "astrology",
                "mining",
                "pottery",
                "sailing",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        );

        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let city_pos = g.cities[&cid].pos;
        let farm_pos = g.cities[&cid]
            .owned_tiles
            .iter()
            .copied()
            .find(|p| *p != city_pos)
            .expect("city owns a ring tile");
        {
            let tile = g.map.tiles.get_mut(&farm_pos).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.hills = false;
        }
        assert!(g
            .valid_improvements(0, farm_pos)
            .iter()
            .any(|i| i == "farm"));

        // Conjure a builder on the controlled owned tile, then exercise the
        // real action path rather than only checking rules metadata.
        let mut saved = serde_json::to_value(&g).unwrap();
        let builder = saved["next_id"].as_u64().unwrap() as u32;
        saved["next_id"] = serde_json::json!(builder + 1);
        saved["units"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": builder, "type": "builder", "owner": 0,
                "pos": [farm_pos.0, farm_pos.1], "hp": 100,
                "moves_left": 2.0, "charges": 3,
            }));
        let mut g: Game = serde_json::from_value(saved).unwrap();
        let housing_before = g.city_housing(&g.cities[&cid]);
        g.apply(
            0,
            &Action::Improve {
                unit: builder,
                improvement: "farm".to_string(),
            },
        )
        .unwrap();
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
        let mut g = Game::new_full(2, 24, 16, 3, 60, 0, false);
        g.map.clear_rivers();
        let (flat, river) = g
            .map
            .tiles
            .values()
            .find_map(|t| {
                if g.rules.is_water(t) || !g.rules.is_passable(t) {
                    return None;
                }
                g.nbrs(t.pos)
                    .into_iter()
                    .find(|n| {
                        g.map
                            .get(*n)
                            .is_some_and(|nt| !g.rules.is_water(nt) && g.rules.is_passable(nt))
                    })
                    .map(|n| (t.pos, n))
            })
            .expect("map has adjacent passable land tiles");
        assert!(g.map.set_river_edge(flat, river, true));
        g.map.tiles.get_mut(&river).unwrap().feature = Some("forest".to_string());

        // The +2 surcharge belongs to this exact shared boundary and applies
        // in either direction, not to every move involving a 'river tile'.
        let into_base = g.rules.move_cost(&g.map.tiles[&river]);
        assert_eq!(g.step_cost(flat, river), into_base + 2.0);
        let out_base = g.rules.move_cost(&g.map.tiles[&flat]);
        assert_eq!(g.step_cost(river, flat), out_base + 2.0);
        let side = g
            .nbrs(river)
            .into_iter()
            .find(|n| {
                *n != flat
                    && g.map
                        .get(*n)
                        .is_some_and(|t| !g.rules.is_water(t) && g.rules.is_passable(t))
            })
            .expect("river tile has another land neighbor");
        assert_eq!(
            g.step_cost(river, side),
            g.rules.move_cost(&g.map.tiles[&side])
        );

        // a unit with full MP may always take one step, even a 4-MP river+hill
        let mut g2 = teleport(&g, g.player_unit_ids(0)[0], flat);
        let uid = g2.player_unit_ids(0)[0];
        if g2.can_move(uid, river) {
            g2.apply(
                0,
                &Action::Move {
                    unit: uid,
                    to: river,
                },
            )
            .unwrap();
            assert_eq!(g2.units[&uid].moves_left, 0.0);
        }
    }

    #[test]
    fn zone_of_control_stops_movement() {
        let mut g = Game::new_full(2, 24, 16, 7, 60, 0, false);
        let me = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let foe = g
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        // Park the enemy warrior two tiles from ours. Normalize the two test
        // tiles so this combat fixture is independent of map-generator tuning.
        let mypos = g.units[&me].pos;
        let spot = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|pos| {
                g.wdist(*pos, mypos) == 2
                    && g.units_at(*pos).is_empty()
                    && g.city_at(*pos).is_none()
                    && g.nbrs(*pos).iter().any(|n| {
                        g.wdist(*n, mypos) == 1
                            && g.map.get(*n).is_some()
                            && g.units_at(*n).is_empty()
                            && g.city_at(*n).is_none()
                    })
            })
            .expect("open tile at distance 2");
        let mid = *g
            .nbrs(spot)
            .iter()
            .find(|n| g.wdist(**n, mypos) == 1 && g.map.get(**n).is_some())
            .unwrap();
        g.map.clear_rivers();
        for pos in [spot, mid] {
            let tile = g.map.tiles.get_mut(&pos).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        let mut g = teleport(&g, foe, spot);
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        assert!(g.in_enemy_zoc(0, mid));
        g.apply(0, &Action::Move { unit: me, to: mid }).unwrap();
        assert!(g.units[&me].zoc_stopped);
        let out = hex::neighbors(mid)
            .into_iter()
            .find(|n| *n != mypos && g.map.get(*n).is_some());
        if let Some(o) = out {
            assert!(g.apply(0, &Action::Move { unit: me, to: o }).is_err());
        }
    }

    #[test]
    fn wall_hp_pool() {
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // no walls: no strike, no wall pool
        assert_eq!(g.city_max_wall_hp(&g.cities[&cid]), 0);
        assert!(!g.city_can_strike(&g.cities[&cid]));
        let base_cs = g.city_strength(cid);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 100;
        assert_eq!(g.city_max_wall_hp(&g.cities[&cid]), 100);
        assert!(g.city_can_strike(&g.cities[&cid]));
        assert_eq!(g.city_strength(cid), base_cs + 3.0);
        // city ranged strength floors at 3 and tracks best ranged unit
        assert!(g.city_ranged_strength(cid) >= 3.0);
        // healthy walls absorb a melee attack: city keeps nearly all HP
        let cpos = g.cities[&cid].pos;
        let foe = g
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let adj = hex::neighbors(cpos)
            .into_iter()
            .find(|n| {
                g.map
                    .get(*n)
                    .map(|t| {
                        !g.rules.is_water(t) && g.rules.is_passable(t) && g.units_at(*n).is_empty()
                    })
                    .unwrap_or(false)
            })
            .expect("open tile next to city");
        // clear the garrison so the melee attack targets the city itself
        let mine = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let far = g
            .map
            .tiles
            .values()
            .find(|t| {
                hex::distance(t.pos, cpos) > 6
                    && !g.rules.is_water(t)
                    && g.rules.is_passable(t)
                    && g.units_at(t.pos).is_empty()
            })
            .map(|t| t.pos)
            .unwrap();
        let g = teleport(&g, mine, far);
        let mut g = teleport(&g, foe, adj);
        g.apply(0, &Action::DeclareWar { player: 1 }).unwrap();
        g.apply(0, &Action::EndTurn).unwrap();
        let city_hp = g.cities[&cid].hp;
        g.apply(
            1,
            &Action::Attack {
                unit: foe,
                target: cpos,
            },
        )
        .unwrap();
        assert!(g.cities[&cid].wall_hp < 100); // walls took the hit
        assert!(g.cities[&cid].hp >= city_hp - 1); // city behind walls: 1 dmg
    }

    #[test]
    fn policy_cards() {
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // chiefdom: 1 military + 1 economic slot
        g.players[0].civics.insert("code_of_laws".to_string());
        g.apply(
            0,
            &Action::Government {
                government: "chiefdom".to_string(),
            },
        )
        .unwrap();
        let base_prod = g.city_yields(cid).production;
        g.apply(
            0,
            &Action::SlotPolicy {
                policy: "urban_planning".to_string(),
            },
        )
        .unwrap();
        // Urban Planning's +1 Production, scaled by the Happy amenity band.
        assert!((g.city_yields(cid).production - base_prod - 1.0 * 1.1).abs() < 1e-9);
        // second economic card cannot fit (no wildcard slots in chiefdom)
        assert!(g
            .apply(
                0,
                &Action::SlotPolicy {
                    policy: "god_king".to_string()
                }
            )
            .is_err());
        // military slot still free
        g.apply(
            0,
            &Action::SlotPolicy {
                policy: "discipline".to_string(),
            },
        )
        .unwrap();
        // oligarchy has a wildcard slot: economic overflow fits there
        g.players[0]
            .civics
            .insert("political_philosophy".to_string());
        g.apply(
            0,
            &Action::Government {
                government: "oligarchy".to_string(),
            },
        )
        .unwrap();
        g.apply(
            0,
            &Action::SlotPolicy {
                policy: "god_king".to_string(),
            },
        )
        .unwrap();
        assert_eq!(g.players[0].policies.len(), 3);
        // downgrading drops cards until the layout fits again
        g.apply(
            0,
            &Action::Government {
                government: "chiefdom".to_string(),
            },
        )
        .unwrap();
        assert!(g.players[0].policies.len() <= 2);
        // feudalism obsoletes agoge via feudal_contract
        g.players[0].civics.insert("craftsmanship".to_string());
        assert!(g.available_policies(0).iter().any(|c| c == "agoge"));
        g.players[0].civics.insert("feudalism".to_string());
        assert!(!g.available_policies(0).iter().any(|c| c == "agoge"));
        assert!(g
            .available_policies(0)
            .iter()
            .any(|c| c == "feudal_contract"));
    }

    #[test]
    fn citizen_governor_meets_food_target_and_tracks_city_plan() {
        let mut g = Game::new_full(1, 20, 14, 41, 40, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let center = g.cities[&cid].pos;
        let ring: Vec<_> = g.cities[&cid]
            .owned_tiles
            .iter()
            .filter(|p| **p != center)
            .copied()
            .collect();
        assert!(ring.len() >= 4);

        // A controlled housing-capped city needs two food from its two worked
        // tiles. It also has a culture option and a production option.
        g.map.clear_rivers();
        for pos in g.cities[&cid].owned_tiles.clone() {
            let tile = g.map.tiles.get_mut(&pos).unwrap();
            tile.terrain = "desert".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.hills = false;
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
            assert_eq!(
                w.production > balanced.production,
                production,
                "{civ} production priority"
            );
            assert_eq!(w.gold > balanced.gold, gold, "{civ} gold priority");
            assert_eq!(
                w.science > balanced.science,
                science,
                "{civ} science priority"
            );
            assert_eq!(
                w.culture > balanced.culture,
                culture,
                "{civ} culture priority"
            );
        }

        g.players[0].civ = "Greece".to_string();
        let greek = g.city_citizen_plan(cid);
        assert_eq!(greek.worked_tiles.len(), 2);
        let collected_food = 2.0
            + greek
                .worked_tiles
                .iter()
                .map(|p| g.rules.tile_yields(&g.map.tiles[p]).food)
                .sum::<f64>();
        assert!(collected_food + 1e-9 >= greek.strategy.food_target);
        assert!(
            greek.worked_tiles.contains(&ring[0]),
            "food safety tile not worked"
        );
        assert!(
            greek.worked_tiles.contains(&ring[2]),
            "Greece should favor culture"
        );

        // The identical city under Nubia keeps the food tile but switches its
        // discretionary citizen from culture to production.
        g.players[0].civ = "Nubia".to_string();
        let nubian = g.city_citizen_plan(cid);
        assert!(nubian.worked_tiles.contains(&ring[0]));
        assert!(
            nubian.worked_tiles.contains(&ring[1]),
            "Nubia should favor production"
        );
        assert!(!nubian.worked_tiles.contains(&ring[2]));

        let before = g.citizen_strategy(cid);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .queue
            .push(crate::game::Item::Wonder {
                wonder: "pyramids".to_string(),
                pos: ring[3],
            });
        let wonder = g.citizen_strategy(cid);
        assert_eq!(wonder.focus, "wonder");
        assert!(wonder.weights.production > before.weights.production);

        // A Civ VI Granary supplies exactly +1 Food. That frees one citizen
        // for a strategic job, while the other still covers food consumption.
        g.rules.buildings.get_mut("granary").unwrap().housing = 0.0;
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("granary".to_string());
        let fed_by_infrastructure = g.city_citizen_plan(cid);
        assert!(fed_by_infrastructure.worked_tiles.contains(&ring[0]));
        assert!(fed_by_infrastructure.worked_tiles.contains(&ring[1]));

        let observed = crate::obs::observation(&g, 0);
        let city = observed["cities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == serde_json::json!(cid))
            .unwrap();
        assert_eq!(city["citizens"]["focus"], "wonder");
        assert_eq!(
            city["citizens"]["worked_tiles"].as_array().unwrap().len(),
            2
        );

        // Building slots turn specialty-district yields into real citizen
        // jobs. The governor compares them directly with tiles and exposes
        // the assignment in observations.
        g.map.tiles.get_mut(&ring[1]).unwrap().hills = false;
        g.map.tiles.get_mut(&ring[2]).unwrap().resource = None;
        g.map.tiles.get_mut(&ring[3]).unwrap().district = Some("campus".to_string());
        g.cities
            .get_mut(&cid)
            .unwrap()
            .districts
            .insert("campus".to_string(), ring[3]);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("library".to_string());
        g.players[0].civ = "China".to_string();
        g.cities.get_mut(&cid).unwrap().queue.clear();
        let specialist_plan = g.city_citizen_plan(cid);
        assert_eq!(specialist_plan.specialists, vec!["campus"]);
        assert_eq!(specialist_plan.worked_tiles, vec![ring[0]]);

        let observed = crate::obs::observation(&g, 0);
        let city = observed["cities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|city| city["id"] == serde_json::json!(cid))
            .unwrap();
        assert_eq!(
            city["citizens"]["specialists"],
            serde_json::json!(["campus"])
        );
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
        assert_eq!(Game::trade_route_duration(4, 0), 24);
        assert_eq!(Game::trade_route_duration(5, 1), 30);
        assert_eq!(Game::trade_route_duration(10, 0), 40);
        assert_eq!(Game::trade_route_duration(11, 1), 22);
        assert_eq!(Game::trade_route_duration(8, 2), 32);
        assert_eq!(Game::trade_route_duration(21, 6), 42);
        assert_eq!(Game::trade_route_duration(26, 8), 52);

        let mut g = Game::new_full(2, 26, 16, 3, 200, 2, false);
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cap = g.player_city_ids(0)[0];
        let cpos = g.cities[&cap].pos;
        // second own city 4+ tiles out for a domestic route
        let spot = g
            .map
            .tiles
            .values()
            .find(|t| {
                let d = g.wdist(t.pos, cpos);
                (4..=8).contains(&d)
                    && !g.rules.is_water(t)
                    && g.rules.is_passable(t)
                    && g.units_at(t.pos).is_empty()
                    && g.cities.values().all(|c| g.wdist(t.pos, c.pos) >= 4)
            })
            .map(|t| t.pos)
            .expect("settle spot");
        let (g2, s2) = conjure(&g, "settler", spot);
        let mut g = g2;
        g.apply(0, &Action::FoundCity { unit: s2 }).unwrap();
        let second = *g.player_city_ids(0).iter().find(|c| **c != cap).unwrap();
        // trader + foreign trade civic → capacity 1
        g.players[0].civics.insert("foreign_trade".to_string());
        assert_eq!(g.trade_capacity(0), 1);
        let (mut g, trader) = conjure(&g, "trader", cpos);
        let before = g.city_yields(cap);
        g.apply(
            0,
            &Action::TradeRoute {
                unit: trader,
                city: second,
            },
        )
        .unwrap();
        let one_way = g.wdist(g.cities[&cap].pos, g.cities[&second].pos) as u32;
        assert_eq!(
            g.routes[0].ends - g.turn,
            Game::trade_route_duration(one_way, g.world_era)
        );
        assert_eq!(g.active_routes(0), 1);
        assert!(!g.units.contains_key(&trader)); // trader is on the road
        let after = g.city_yields(cap);
        // domestic city-center route: +1 food +1 production at the origin
        let route = g.route_yields(second, true);
        assert!((route.food - 1.0).abs() < 1e-9);
        assert!((route.production - 1.0).abs() < 1e-9);
        assert!(
            after.total() > before.total(),
            "the citizen governor may reassign tiles, but the route must increase total output"
        );
        // capacity is enforced
        let (mut g3, t2) = conjure(&g, "trader", cpos);
        assert!(g3
            .apply(
                0,
                &Action::TradeRoute {
                    unit: t2,
                    city: second
                }
            )
            .is_err());
        // Increasing capacity must not permit a second route to the same
        // destination, even when it starts in a different owned city.
        let third_spot = g3
            .map
            .tiles
            .values()
            .find(|tile| {
                !g3.rules.is_water(tile)
                    && g3.rules.is_passable(tile)
                    && g3.units_at(tile.pos).is_empty()
                    && g3
                        .cities
                        .values()
                        .all(|city| g3.wdist(tile.pos, city.pos) >= 4)
                    && g3.wdist(tile.pos, g3.cities[&second].pos) <= 15
            })
            .map(|tile| tile.pos)
            .expect("third settle spot");
        let (g4, third_settler) = conjure(&g3, "settler", third_spot);
        let mut g4 = g4;
        g4.apply(
            0,
            &Action::FoundCity {
                unit: third_settler,
            },
        )
        .unwrap();
        let third = *g4
            .player_city_ids(0)
            .iter()
            .find(|city| **city != cap && **city != second)
            .unwrap();
        let before_government = g4.trade_capacity(0);
        g4.players[0].government = Some("merchant_republic".to_string());
        // Merchant Republic carries two Trade Routes of its own.
        assert_eq!(g4.trade_capacity(0), before_government + 2);
        g4.players[0]
            .counters
            .insert("great_person_trade_capacity".to_string(), 1);
        assert!(g4.trade_capacity(0) > g4.active_routes(0));
        let third_pos = g4.cities[&third].pos;
        let (mut g4, third_trader) = conjure(&g4, "trader", third_pos);
        assert!(g4
            .apply(
                0,
                &Action::TradeRoute {
                    unit: third_trader,
                    city: second,
                },
            )
            .is_err());
        // a road was laid toward the destination
        assert!(g.map.tiles.values().any(|t| t.road));
        // Final-patch envoys: +1 of a non-trade type yield in the Capital at
        // 1 envoy (trade city-states grant +2 Gold instead).
        let minor = g
            .players
            .iter()
            .find(|p| p.is_minor && !p.is_barbarian)
            .expect("city-state")
            .id;
        match g.players[minor].civ.as_str() {
            "Kabul" | "Carthage" | "Valletta" => {
                g.cities.get_mut(&cap).unwrap().queue = vec![crate::game::Item::Unit {
                    unit: "warrior".to_string(),
                }];
            }
            "Auckland" => {
                g.cities.get_mut(&cap).unwrap().queue = vec![crate::game::Item::Building {
                    building: "granary".to_string(),
                }];
            }
            _ => {}
        }
        g.players[0].envoys_free = 1;
        let before = g.city_yields(cap);
        g.apply(0, &Action::SendEnvoy { player: minor }).unwrap();
        assert_eq!(g.envoys_at(0, minor), 1);
        let after = g.city_yields(cap);
        // Non-food envoy yields carry the Happy amenity band.
        let expected = if g.players[minor].civ == "Zanzibar" {
            2.0 * 1.1
        } else {
            1.1
        };
        let delta = after.total() - before.total();
        assert!(
            (delta - expected).abs() < 1e-6,
            "{} first Envoy: expected {expected}, got {delta}",
            g.players[minor].civ
        );
        // suzerain needs 3+ envoys and a strict lead
        assert_eq!(g.suzerain_of(minor), None);
        g.players[0].envoys[0].1 = 3;
        assert_eq!(g.suzerain_of(minor), Some(0));
        g.players[1].envoys = vec![(minor, 3)];
        assert_eq!(g.suzerain_of(minor), None, "a tie has no Suzerain");
        g.players[0].envoys[0].1 = 4;
        assert_eq!(g.suzerain_of(minor), Some(0));
        g.at_war.insert((0, 1));
        assert!(
            g.is_at_war(minor, 1),
            "a city-state follows its Suzerain into war"
        );
        g.current = 1;
        let peace = g.legal_actions(1);
        assert!(peace.contains(&Action::MakePeace { player: 0 }));
        assert!(!peace.contains(&Action::MakePeace { player: minor }));
        g.current = 0;
        g.at_war.remove(&(0, 1));
        assert!(
            !g.is_at_war(minor, 1),
            "a city-state follows its Suzerain back to peace"
        );
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
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        // pantheon at 25 faith; beliefs are exclusive
        assert!(g
            .apply(
                0,
                &Action::ChoosePantheon {
                    belief: "fertility_rites".to_string()
                }
            )
            .is_err());
        g.players[0].faith = 30.0;
        g.apply(
            0,
            &Action::ChoosePantheon {
                belief: "fertility_rites".to_string(),
            },
        )
        .unwrap();
        assert!(g
            .apply(
                0,
                &Action::ChoosePantheon {
                    belief: "divine_spark".to_string()
                }
            )
            .is_err());
        // prophet + holy site founds a religion with exclusive beliefs
        let dpos = g.cities[&cid]
            .owned_tiles
            .iter()
            .find(|p| **p != g.cities[&cid].pos)
            .cloned()
            .unwrap();
        g.cities
            .get_mut(&cid)
            .unwrap()
            .districts
            .insert("holy_site".to_string(), dpos);
        g.players[0].prophet_pending = true;
        g.apply(
            0,
            &Action::FoundReligion {
                follower: "choral_music".to_string(),
                founder: "tithe".to_string(),
            },
        )
        .unwrap();
        assert!(g.players[0].religion.is_some());
        // the holy city converts instantly
        assert_eq!(
            g.city_religion(&g.cities[&cid]),
            g.players[0].religion.as_deref()
        );
        // follower belief: +2 culture with a shrine in a following city
        let before = g.city_yields(cid).culture;
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("shrine".to_string());
        let after = g.city_yields(cid).culture;
        assert!((after - before - 2.0 * 1.1).abs() < 1e-9);
        // missionary spread converts a foreign city
        let religion = g.players[0].religion.clone().unwrap();
        let s1 = g
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        let mut g = {
            // second civ founds far away
            let far = g
                .map
                .tiles
                .values()
                .find(|t| {
                    g.wdist(t.pos, g.cities[&cid].pos) >= 6
                        && !g.rules.is_water(t)
                        && g.rules.is_passable(t)
                        && g.units_at(t.pos).is_empty()
                        && g.cities.values().all(|c| g.wdist(t.pos, c.pos) >= 4)
                })
                .map(|t| t.pos)
                .unwrap();
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
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let dpos = g.cities[&cid]
            .owned_tiles
            .iter()
            .find(|p| **p != g.cities[&cid].pos)
            .cloned()
            .unwrap();
        g.cities
            .get_mut(&cid)
            .unwrap()
            .districts
            .insert("campus".to_string(), dpos);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("library".to_string());
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
        assert!(
            (pts - 2.0).abs() < 1e-9,
            "expected 2 scientist gpp, got {pts}"
        );
        let hypatia_cost = g.gp_cost(0, "scientist");
        assert_eq!(hypatia_cost, 78.0);
        // Reaching the threshold auto-claims Hypatia. Because this Campus
        // already has a Library, her instant building is a no-op while her
        // permanent +1 Science to Libraries still applies.
        g.players[0]
            .gpp
            .insert("scientist".to_string(), hypatia_cost - 1.0);
        let boosts_before = g.players[0].boosted_techs.clone();
        let science_before = g.city_yields(cid).science;
        round(&mut g);
        assert_eq!(g.players[0].gp_claimed.get("scientist"), Some(&1));
        assert_eq!(g.players[0].boosted_techs, boosts_before);
        assert!((g.city_yields(cid).science - science_before - 1.0 * 1.1).abs() < 1e-9);
        // The global market advances to the next named Scientist rather than
        // fabricating a generic doubled threshold.
        assert_eq!(
            g.current_great_person("scientist").unwrap().0,
            "isaac_newton"
        );
        assert_eq!(g.gp_cost(0, "scientist"), 1_646.0);
    }

    #[test]
    fn eras_and_culture_victory() {
        let mut g = Game::new_full(2, 20, 14, 5, 300, 0, false);
        assert_eq!(g.world_era, 0);
        // push the leader past the classical threshold with a big era score
        for t in [
            "pottery",
            "mining",
            "sailing",
            "astrology",
            "irrigation",
            "archery",
            "writing",
            "masonry",
            "bronze_working",
            "animal_husbandry",
            "horseback_riding",
            "currency",
        ] {
            g.players[0].techs.insert(t.to_string());
        }
        g.players[0].era_score = g.players[0].golden_age_threshold;
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
        g.players[0].tourism_pressure.insert(1, 100000.0);
        round(&mut g);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("culture"));
    }

    #[test]
    fn natural_wonders_and_support_units() {
        let g = Game::new_full(2, 26, 16, 3, 60, 0, false);
        // Wonders generate, including Crater Lake's workable/passable tile.
        let nw: Vec<_> = g
            .map
            .tiles
            .values()
            .filter(|t| {
                t.feature
                    .as_deref()
                    .map(|f| g.rules.features[f].natural_wonder)
                    .unwrap_or(false)
            })
            .collect();
        assert!(!nw.is_empty(), "no natural wonders generated");
        // Crater Lake is a one-tile passable wonder that acts as a Lake; the
        // impassable ones are Uluru, Yosemite, Everest and Pamukkale.
        if let Some(t) = g
            .map
            .tiles
            .values()
            .find(|t| t.feature.as_deref() == Some("crater_lake"))
        {
            assert!(g.rules.is_passable(t));
            assert_eq!(g.rules.tile_yields(t).faith, 5.0);
        }
        if let Some(t) = g
            .map
            .tiles
            .values()
            .find(|t| t.feature.as_deref() == Some("uluru"))
        {
            assert!(!g.rules.is_passable(t));
        }
        // battering ram lets melee hit ancient walls at full strength
        let mut g = Game::new_full(2, 24, 16, 9, 60, 0, false);
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let cpos = g.cities[&cid].pos;
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 100;
        let mine = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let far = g
            .map
            .tiles
            .values()
            .find(|t| {
                g.wdist(t.pos, cpos) > 6
                    && !g.rules.is_water(t)
                    && g.rules.is_passable(t)
                    && g.units_at(t.pos).is_empty()
            })
            .map(|t| t.pos)
            .unwrap();
        let g2 = teleport(&g, mine, far);
        let adj = crate::hex::neighbors(cpos)
            .into_iter()
            .find(|n| {
                g2.map
                    .get(*n)
                    .map(|t| {
                        !g2.rules.is_water(t)
                            && g2.rules.is_passable(t)
                            && g2.units_at(*n).is_empty()
                    })
                    .unwrap_or(false)
            })
            .unwrap();
        let foe = g2
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g2.units[id].kind == "warrior")
            .unwrap();
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
        g4.apply(
            1,
            &Action::Attack {
                unit: foe,
                target: cpos,
            },
        )
        .unwrap();
        // full-strength wall damage (>= 8) instead of the 15% trickle (<= 6)
        assert!(
            100 - g4.cities[&cid].wall_hp >= 8,
            "ram should breach: wall_hp {}",
            g4.cities[&cid].wall_hp
        );
    }

    #[test]
    fn loyalty_governors_congress() {
        let mut g = Game::new_full(2, 26, 16, 9, 300, 1, false);
        let s = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(0)[0];
        assert_eq!(g.cities[&cid].loyalty, 100.0);
        // governor titles come from civic milestones
        assert_eq!(g.governor_titles(0), 0);
        g.players[0]
            .civics
            .insert("political_philosophy".to_string());
        assert_eq!(g.governor_titles(0), 1);
        g.apply(0, &Action::AssignGovernor { city: cid }).unwrap();
        assert!(g.apply(0, &Action::AssignGovernor { city: cid }).is_err());
        // amenity bonus from the governor
        assert!(g.players[0].governors.contains(&cid));
        let round = |g: &mut Game| {
            let first = g.current;
            g.apply(first, &Action::EndTurn).unwrap();
            while g.current != first && g.winner.is_none() {
                let current = g.current;
                g.apply(current, &Action::EndTurn).unwrap();
            }
        };
        // World Congress: a Medieval session opens on turn 30, accepts
        // outcome-and-target ballots for five rounds, then enacts the result.
        g.world_era = 2;
        g.turn = 29; // wraps to 30 after a full round
        let minor = g
            .players
            .iter()
            .find(|p| p.is_minor && !p.is_barbarian)
            .map(|p| p.id)
            .unwrap();
        g.players[0].envoys = vec![(minor, 3)];
        round(&mut g);
        assert!(g.congress.is_some());
        let (resolution, choice) = {
            let resolution = &g.congress.as_ref().unwrap().resolutions[0];
            (resolution.id.clone(), resolution.choices[0].clone())
        };
        g.apply(
            0,
            &Action::CongressVote {
                resolution,
                choice,
                votes: 1,
            },
        )
        .unwrap();
        for _ in 0..5 {
            round(&mut g);
        }
        assert_eq!(g.players[0].dvp, 1);
        // At the Modern-era World Leader resolution, 20 points wins.
        g.players[0].dvp = 18;
        g.world_era = 5;
        g.turn = 59;
        round(&mut g);
        g.apply(
            0,
            &Action::CongressVote {
                resolution: "world_leader".to_string(),
                choice: "A:0".to_string(),
                votes: 1,
            },
        )
        .unwrap();
        for _ in 0..5 {
            round(&mut g);
        }
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("diplomatic"));
    }

    #[test]
    fn leaders_present_and_uniques_gated() {
        let g = Game::new_full(8, 40, 24, 3, 60, 0, false);
        // every playable civ has a leader and ability defined
        for name in crate::game::CIV_NAMES {
            let spec = g
                .rules
                .civs
                .get(name)
                .unwrap_or_else(|| panic!("no leader data for {name}"));
            assert!(!spec.leader.is_empty());
            assert!(!spec.ability.is_empty());
            if let Some(uu) = &spec.unique_unit {
                let us = &g.rules.units[uu.as_str()];
                assert_eq!(
                    us.unique_to.as_deref(),
                    Some(name),
                    "{uu} unique_to mismatch"
                );
            }
        }
        // seats map to civs in order: 0 Rome .. 7 Scythia
        for (i, name) in crate::game::CIV_NAMES.iter().enumerate() {
            assert_eq!(&g.players[i].civ, name);
        }
        // unique units: only their civ builds them; the base is blocked
        let mut g = g;
        let greece = 2;
        let s = g
            .player_unit_ids(greece)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        // clear the current player gate for direct checks
        g.players[greece].techs.insert("bronze_working".to_string());
        while g.current != greece {
            let cur = g.current;
            g.apply(cur, &Action::EndTurn).unwrap();
        }
        g.apply(greece, &Action::FoundCity { unit: s }).unwrap();
        let cid = g.player_city_ids(greece)[0];
        use crate::game::Item;
        assert!(g.can_produce(
            greece,
            cid,
            &Item::Unit {
                unit: "hoplite".to_string()
            }
        ));
        assert!(!g.can_produce(
            greece,
            cid,
            &Item::Unit {
                unit: "spearman".to_string()
            }
        ));
        assert!(!g.can_produce(
            greece,
            cid,
            &Item::Unit {
                unit: "legion".to_string()
            }
        ));
        // Greece: Plato's Republic grants an extra wildcard slot
        g.players[greece].civics.insert("code_of_laws".to_string());
        g.apply(
            greece,
            &Action::Government {
                government: "chiefdom".to_string(),
            },
        )
        .unwrap();
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
        g.apply(
            china,
            &Action::Research {
                tech: "pottery".to_string(),
            },
        )
        .unwrap();
        let cost = g.rules.techs["pottery"].cost;
        assert!((g.players[china].research_progress - 0.5 * cost).abs() < 1e-9);
        // Rome: founded cities start with a free monument
        let rome = 0;
        let s = g
            .player_unit_ids(rome)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
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
        let settler = g
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        let spos = g.units[&settler].pos;
        // park their escort far away so the capture is uncontested
        let foe_w = g
            .player_unit_ids(1)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        let far = g
            .map
            .tiles
            .values()
            .find(|t| {
                g.wdist(t.pos, spos) > 8
                    && !g.rules.is_water(t)
                    && g.rules.is_passable(t)
                    && g.units_at(t.pos).is_empty()
            })
            .map(|t| t.pos)
            .unwrap();
        let g2 = teleport(&g, foe_w, far);
        let mine = g2
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g2.units[id].kind == "warrior")
            .unwrap();
        let adj = crate::hex::neighbors(spos)
            .into_iter()
            .find(|n| {
                g2.map
                    .get(*n)
                    .map(|t| {
                        !g2.rules.is_water(t)
                            && g2.rules.is_passable(t)
                            && g2.units_at(*n).is_empty()
                    })
                    .unwrap_or(false)
            })
            .unwrap();
        let mut g3 = teleport(&g2, mine, adj);
        g3.apply(
            0,
            &Action::Move {
                unit: mine,
                to: spos,
            },
        )
        .unwrap();
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
    fn spectator_city_detail_is_seat_stable() {
        let mut g = Game::new(2, 18, 12, 4, 25, 1);
        let mut ais = BasicAi::fleet(&g);
        run_game(&mut g, &mut ais);
        // spectator frames rotate the observed seat every step; city detail
        // (queue, production) must be present from every seat or the GUI's
        // bars blink as the seat changes
        for pid in 0..2 {
            let o = crate::obs::observation_spectator(&g, pid);
            let cities = o["cities"].as_array().unwrap();
            assert!(!cities.is_empty());
            for c in cities {
                assert!(
                    c.get("queue").is_some(),
                    "city {} lacks detail from seat {pid}",
                    c["name"]
                );
            }
            // the fog-of-war view keeps detail private to the observer
            let o = crate::obs::observation(&g, pid);
            for c in o["cities"].as_array().unwrap() {
                if c["owner"].as_u64() != Some(pid as u64) {
                    assert!(c.get("queue").is_none());
                }
            }
        }
    }

    #[test]
    fn spectator_frames_skip_human_only_reachable_pathfinding() {
        let g = Game::new(1, 18, 12, 41, 25, 0);
        let uid = g.player_unit_ids(0)[0];

        let human = crate::obs::observation(&g, 0);
        let human_unit = human["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["id"].as_u64() == Some(uid as u64))
            .unwrap();
        assert!(human_unit["reachable"].is_array());

        let spectator = crate::obs::observation_spectator(&g, 0);
        let spectator_unit = spectator["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["id"].as_u64() == Some(uid as u64))
            .unwrap();
        assert!(spectator_unit.get("reachable").is_none());
    }

    #[test]
    fn action_protocol_json() {
        let a: Action =
            serde_json::from_str(r#"{"type": "move", "unit": 3, "to": [1, -2]}"#).unwrap();
        match a {
            Action::Move { unit, to } => {
                assert_eq!(unit, 3);
                assert_eq!(to, (1, -2));
            }
            _ => panic!("wrong variant"),
        }
        let e = serde_json::to_string(&Action::EndTurn).unwrap();
        assert_eq!(e, r#"{"type":"end_turn"}"#);

        let promotion: Action =
            serde_json::from_str(r#"{"type":"promote","unit":7,"promotion":"battlecry"}"#).unwrap();
        assert!(matches!(
            promotion,
            Action::Promote { unit: 7, ref promotion } if promotion == "battlecry"
        ));
        assert_eq!(
            serde_json::to_string(&Action::EncampmentStrike {
                city: 4,
                target: (2, -1),
            })
            .unwrap(),
            r#"{"type":"encampment_strike","city":4,"target":[2,-1]}"#
        );
    }
}
