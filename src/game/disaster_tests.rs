//! Gathering Storm's random disasters: the intensity setting, the four storm
//! systems, river floods, droughts and volcanic eruptions, and the way a
//! warming world makes all of them more frequent and more severe.
use super::{Drought, Game, Pos, Storm, DEFAULT_DISASTER_INTENSITY};

fn quiet_game() -> Game {
    // Disasters roll against a per-turn probability derived from `max_turns`,
    // so every test states the turn budget it is reasoning about.
    Game::new_full(2, 30, 18, 4242, 500, 0, false)
}

/// Drive whole game turns so the world-turn phases (including disasters) run.
fn advance(game: &mut Game, turns: u32) {
    let target = game.turn + turns;
    while game.turn < target {
        let before = game.turn;
        while game.turn == before {
            let current = game.current;
            game.apply(current, &super::Action::EndTurn).unwrap();
        }
    }
}

fn land_tile(game: &Game) -> Pos {
    *game
        .map
        .tiles
        .iter()
        .find(|(_, tile)| !game.rules.is_water(tile) && tile.feature.is_none())
        .map(|(position, _)| position)
        .expect("the map has open land")
}

#[test]
fn the_default_intensity_is_the_middle_of_the_five_settings() {
    let game = quiet_game();
    assert_eq!(game.disaster_intensity(), DEFAULT_DISASTER_INTENSITY);
    assert_eq!(DEFAULT_DISASTER_INTENSITY, 2);
}

#[test]
fn intensity_zero_leaves_every_volcano_dormant_and_fires_nothing() {
    let mut game = quiet_game();
    game.disaster_intensity = 0;
    let volcano = land_tile(&game);
    game.map.tiles.get_mut(&volcano).unwrap().feature = Some("volcano".to_string());
    assert!(!game.volcano_active(volcano));

    advance(&mut game, 30);
    assert!(game.storms.is_empty(), "no storms form with disasters off");
    assert!(game.droughts.is_empty(), "no droughts form with disasters off");
}

#[test]
fn a_higher_intensity_activates_more_volcanoes_and_fires_more_often() {
    let mut game = quiet_game();
    let volcanoes: Vec<Pos> = game
        .map
        .tiles
        .iter()
        .filter(|(_, tile)| !game.rules.is_water(tile))
        .map(|(position, _)| *position)
        .take(400)
        .collect();
    for position in &volcanoes {
        game.map.tiles.get_mut(position).unwrap().feature = Some("volcano".to_string());
    }
    let active_at = |game: &mut Game, intensity: u8| {
        game.disaster_intensity = intensity;
        volcanoes
            .iter()
            .filter(|position| game.volcano_active(**position))
            .count()
    };
    // The shipped band runs from 45% of the map's cones to 95% of them.
    let minimal = active_at(&mut game, 1);
    let hyperreal = active_at(&mut game, 4);
    assert!(
        minimal < hyperreal,
        "intensity 1 activated {minimal} cones, intensity 4 only {hyperreal}"
    );
    assert!((0.30..0.60).contains(&(minimal as f64 / volcanoes.len() as f64)));
    assert!((0.85..1.0).contains(&(hyperreal as f64 / volcanoes.len() as f64)));

    game.disaster_intensity = 1;
    let quiet = game.disaster_rate("river_flood");
    game.disaster_intensity = 4;
    assert!(game.disaster_rate("river_flood") > quiet * 4.0);
}

#[test]
fn an_eruption_damages_the_ring_and_leaves_volcanic_soil() {
    let mut game = quiet_game();
    let volcano = land_tile(&game);
    game.map.tiles.get_mut(&volcano).unwrap().feature = Some("volcano".to_string());
    let ring: Vec<Pos> = game
        .wdisk(volcano, 1)
        .into_iter()
        .filter(|position| *position != volcano && !game.rules.is_water(&game.map.tiles[position]))
        .collect();
    assert!(!ring.is_empty(), "the volcano needs land around it");
    for position in &ring {
        let tile = game.map.tiles.get_mut(position).unwrap();
        tile.feature = None;
        tile.improvement = Some("farm".to_string());
        tile.pillaged = false;
    }

    game.resolve_eruption(volcano, 3);

    assert!(
        ring.iter()
            .any(|position| game.map.tiles[position].pillaged),
        "a severity-3 eruption pillages what is built around the cone"
    );
    assert!(
        ring.iter()
            .any(|position| game.map.tiles[position].feature.as_deref() == Some("volcanic_soil")),
        "ash leaves Volcanic Soil behind"
    );
    assert_eq!(
        game.map.tiles[&volcano].feature.as_deref(),
        Some("volcano"),
        "the cone itself is not buried"
    );
}

#[test]
fn the_top_two_intensities_widen_the_eruption_to_two_rings() {
    let mut game = quiet_game();
    let volcano = land_tile(&game);
    game.map.tiles.get_mut(&volcano).unwrap().feature = Some("volcano".to_string());
    let second_ring: Vec<Pos> = game
        .wdisk(volcano, 2)
        .into_iter()
        .filter(|position| {
            game.wdist(*position, volcano) == 2 && !game.rules.is_water(&game.map.tiles[position])
        })
        .collect();
    assert!(!second_ring.is_empty());
    let arm = |game: &mut Game| {
        for position in &second_ring {
            let tile = game.map.tiles.get_mut(position).unwrap();
            tile.feature = None;
            tile.improvement = Some("farm".to_string());
            tile.pillaged = false;
        }
    };

    game.disaster_intensity = 2;
    arm(&mut game);
    game.resolve_eruption(volcano, 3);
    assert!(
        !second_ring
            .iter()
            .any(|position| game.map.tiles[position].pillaged),
        "at Moderate an eruption reaches one ring"
    );

    game.disaster_intensity = 4;
    arm(&mut game);
    game.resolve_eruption(volcano, 3);
    assert!(
        second_ring
            .iter()
            .any(|position| game.map.tiles[position].pillaged),
        "at Hyperreal it reaches two"
    );
}

#[test]
fn a_drought_holds_its_tiles_then_lifts_on_schedule() {
    let mut game = quiet_game();
    let farm = land_tile(&game);
    game.map.tiles.get_mut(&farm).unwrap().improvement = Some("farm".to_string());
    game.resolve_drought(&[farm]);
    game.droughts.push(Drought {
        tiles: vec![farm],
        severity: 1,
        ends: game.turn + 3,
    });
    assert!(game.map.tiles[&farm].drought);

    advance(&mut game, 2);
    assert!(game.map.tiles[&farm].drought, "the drought has not run out");

    advance(&mut game, 2);
    assert!(!game.map.tiles[&farm].drought, "the rain came back");
    assert!(game.droughts.is_empty());
    assert!(
        game.map.tiles[&farm].pillaged,
        "the farm it killed stays killed until it is repaired"
    );
}

#[test]
fn a_storm_drifts_for_three_turns_and_then_dissipates() {
    let mut game = quiet_game();
    let origin = *game
        .map
        .tiles
        .iter()
        .find(|(_, tile)| tile.terrain == "plains")
        .map(|(position, _)| position)
        .expect("the map has plains");
    game.storms.push(Storm {
        kind: "tornado".to_string(),
        pos: origin,
        heading: 0,
        severity: 1,
        ends: game.turn + 3,
    });
    // Anything a storm forms on top of is not what the storm is; clear the
    // rest of the roll so only this system is under test.
    game.disaster_intensity = 0;

    advance(&mut game, 1);
    let moved = game.storms.first().map(|storm| storm.pos);
    assert!(moved.is_some(), "the system is still alive on turn one");
    assert_ne!(moved, Some(origin), "a storm does not sit still");
    assert!(
        game.map
            .tiles
            .values()
            .any(|tile| tile.storm.as_deref() == Some("tornado")),
        "the tiles under it are marked while it passes"
    );

    advance(&mut game, 3);
    assert!(game.storms.is_empty(), "three turns and it is gone");
    assert!(
        game.map
            .tiles
            .values()
            .all(|tile| tile.storm.is_none()),
        "and it leaves no marker behind"
    );
}

#[test]
fn a_warming_world_is_a_stormier_one() {
    let mut game = quiet_game();
    let cold = game.disaster_rate("hurricane");
    game.climate_phase = 6;
    let hot = game.disaster_rate("hurricane");
    assert!(
        hot > cold,
        "climate phase 6 should raise the hurricane rate above {cold}, got {hot}"
    );
}

#[test]
fn disasters_actually_fire_over_a_full_game() {
    // The rates are per-game expectations, so a real run has to produce
    // events; a system that never triggers is the failure this guards.
    let mut game = Game::new_full(2, 30, 18, 31337, 200, 0, false);
    game.disaster_intensity = 4;
    let mut storms = 0usize;
    let mut droughts = 0usize;
    for _ in 0..60 {
        advance(&mut game, 1);
        storms += game.storms.len();
        droughts += game.droughts.len();
    }
    assert!(
        storms > 0 || droughts > 0,
        "sixty turns at Hyperreal produced no disasters at all"
    );
}

#[test]
fn disaster_state_survives_a_save() {
    let mut game = quiet_game();
    let farm = land_tile(&game);
    game.map.tiles.get_mut(&farm).unwrap().disaster_food = 2.0;
    game.disaster_intensity = 3;
    game.storms.push(Storm {
        kind: "blizzard".to_string(),
        pos: farm,
        heading: 2,
        severity: 2,
        ends: game.turn + 3,
    });
    game.droughts.push(Drought {
        tiles: vec![farm],
        severity: 1,
        ends: game.turn + 5,
    });

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(restored.disaster_intensity, 3);
    assert_eq!(restored.storms, game.storms);
    assert_eq!(restored.droughts, game.droughts);
    assert_eq!(restored.map.tiles[&farm].disaster_food, 2.0);
}

#[test]
fn a_save_written_before_disasters_loads_at_the_default_intensity() {
    let game = quiet_game();
    let mut value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("disaster_intensity");
    object.remove("storms");
    object.remove("droughts");
    let restored: Game = serde_json::from_value(value).unwrap();
    assert_eq!(restored.disaster_intensity(), DEFAULT_DISASTER_INTENSITY);
    assert!(restored.storms.is_empty());
}

/// The rates in `disasters.json` are per-game expectations, so a full game
/// has to land near them. This is the guard on the whole scheduler: a class
/// that silently stops firing, or one that fires every turn, shows up here.
#[test]
fn a_full_game_lands_near_the_rates_the_ruleset_asks_for() {
    let mut spawned = 0usize;
    const GAMES: u64 = 3;
    for seed in 0..GAMES {
        let mut game = Game::new_full(2, 40, 24, 900 + seed, 500, 0, false);
        game.disaster_intensity = DEFAULT_DISASTER_INTENSITY;
        for _ in 0..500 {
            let turn = game.turn;
            while game.turn == turn {
                let current = game.current;
                if game.apply(current, &super::Action::EndTurn).is_err() {
                    break;
                }
            }
            // A system is counted on the turn it forms, when its expiry is
            // still a full duration away.
            spawned += game
                .storms
                .iter()
                .filter(|storm| storm.ends == game.turn + 3)
                .count();
        }
    }
    // The four storm classes budget 26 systems a game between them at
    // Moderate; a Poisson-ish spread over three games should stay well inside
    // half to double that.
    let per_game = spawned as f64 / GAMES as f64;
    assert!(
        (13.0..=52.0).contains(&per_game),
        "{per_game} storms a game is nowhere near the 26 the ruleset budgets"
    );
}

/// Intensity has to move the whole system, not just the volcano share.
#[test]
fn raising_the_intensity_raises_what_actually_happens() {
    let count = |intensity: u8| {
        let mut game = Game::new_full(2, 40, 24, 4711, 500, 0, false);
        game.disaster_intensity = intensity;
        let mut spawned = 0usize;
        for _ in 0..300 {
            let turn = game.turn;
            while game.turn == turn {
                let current = game.current;
                if game.apply(current, &super::Action::EndTurn).is_err() {
                    break;
                }
            }
            spawned += game
                .storms
                .iter()
                .filter(|storm| storm.ends == game.turn + 3)
                .count();
        }
        spawned
    };
    let light = count(1);
    let hyperreal = count(4);
    assert!(
        hyperreal > light * 2,
        "Hyperreal produced {hyperreal} storms against Light's {light}"
    );
}
