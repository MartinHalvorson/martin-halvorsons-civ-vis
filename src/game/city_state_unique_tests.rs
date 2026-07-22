use super::*;

fn game_with_capitals(players: usize, seed: u64) -> (Game, Vec<u32>) {
    let mut game = Game::new_full(players, 28, 18, seed, 300, 0, false);
    let mut cities = Vec::new();
    for pid in 0..players {
        let settler = game
            .player_unit_ids(pid)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        let city = game.found_city_for(pid, game.units[&settler].pos, None);
        game.remove_unit(settler);
        cities.push(city);
    }
    (game, cities)
}

fn add_city_state(game: &mut Game, name: &str) -> usize {
    let id = game.players.len();
    game.players.push(Player::new(id, name, true));
    id
}

fn make_suzerain(game: &mut Game, leader: usize, minor: usize) {
    game.players[leader].envoys.push((minor, 3));
}

fn install_alliance(game: &mut Game, first: usize, second: usize, kind: &str, level: i32) {
    let alliance = AllianceState {
        kind: kind.to_string(),
        points: match level {
            3.. => 240.0,
            2 => 80.0,
            _ => 0.0,
        },
        level,
        ends: game.turn + 60,
    };
    game.players[first]
        .alliances
        .insert(second, alliance.clone());
    game.players[second].alliances.insert(first, alliance);
}

fn install_district(game: &mut Game, city_id: u32, district: &str) -> Pos {
    let center = game.cities[&city_id].pos;
    let position = game.cities[&city_id]
        .owned_tiles
        .iter()
        .copied()
        .find(|position| {
            *position != center
                && game.map.tiles[position].district.is_none()
                && game.map.tiles[position].wonder.is_none()
        })
        .unwrap();
    let tile = game.map.tiles.get_mut(&position).unwrap();
    tile.feature = None;
    tile.resource = None;
    tile.improvement = None;
    tile.district = Some(district.to_string());
    tile.pillaged = false;
    game.cities
        .get_mut(&city_id)
        .unwrap()
        .districts
        .insert(district.to_string(), position);
    position
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn economic_level_three_shares_unique_suzerain_bonuses_without_relays() {
    let (mut game, _) = game_with_capitals(3, 89_001);
    let geneva = add_city_state(&mut game, "Geneva");
    make_suzerain(&mut game, 1, geneva);

    install_alliance(&mut game, 0, 1, "economic", 2);
    assert!(!game.grants_city_state_unique_bonus(0, "Geneva"));
    game.players[0].alliances.get_mut(&1).unwrap().level = 3;
    game.players[1].alliances.get_mut(&0).unwrap().level = 3;
    assert!(game.grants_city_state_unique_bonus(0, "Geneva"));
    assert!(game.grants_city_state_unique_bonus(1, "Geneva"));
    assert!(!game.grants_city_state_unique_bonus(2, "Geneva"));

    install_alliance(&mut game, 1, 2, "economic", 3);
    assert!(game.grants_city_state_unique_bonus(2, "Geneva"));
    game.players[0].alliances.clear();
    game.players[1].alliances.remove(&0);
    assert!(game.grants_city_state_unique_bonus(2, "Geneva"));
    assert!(!game.grants_city_state_unique_bonus(0, "Geneva"));

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert!(restored.grants_city_state_unique_bonus(2, "Geneva"));
}

#[test]
fn carthage_mohenjo_daro_and_auckland_modify_their_native_systems() {
    let (mut game, cities) = game_with_capitals(2, 89_002);
    let city = cities[0];
    game.players[0].civics.insert("foreign_trade".to_string());
    let encampment = install_district(&mut game, city, "encampment");
    let carthage = add_city_state(&mut game, "Carthage");
    let base_capacity = game.trade_capacity(0);
    make_suzerain(&mut game, 0, carthage);
    assert_eq!(game.trade_capacity(0), base_capacity + 1);
    assert_eq!(Game::cs_type("Carthage"), "militaristic");

    let center = game.cities[&city].pos;
    game.map.tiles.get_mut(&center).unwrap().river_edges = [false; 6];
    for neighbor in game.nbrs(center) {
        let tile = game.map.tiles.get_mut(&neighbor).unwrap();
        tile.terrain = "grassland".to_string();
        tile.feature = None;
        tile.river_edges = [false; 6];
    }
    let housing_without = game.city_housing(&game.cities[&city]);
    game.players[carthage].civ = "Mohenjo-Daro".to_string();
    assert_close(
        game.city_housing(&game.cities[&city]),
        housing_without + 3.0,
    );

    let water = game.cities[&city]
        .owned_tiles
        .iter()
        .copied()
        .find(|position| *position != center && *position != encampment)
        .unwrap();
    {
        let tile = game.map.tiles.get_mut(&water).unwrap();
        tile.terrain = "coast".to_string();
        tile.feature = None;
        tile.resource = None;
        tile.improvement = None;
        tile.district = None;
    }
    let baseline = game.player_tile_yields(0, water, &game.map.tiles[&water]);
    game.players[carthage].civ = "Auckland".to_string();
    let ancient = game.player_tile_yields(0, water, &game.map.tiles[&water]);
    assert_close(ancient.production, baseline.production + 1.0);
    game.world_era = 4;
    let industrial = game.player_tile_yields(0, water, &game.map.tiles[&water]);
    assert_close(industrial.production, baseline.production + 2.0);
}

#[test]
fn geneva_kabul_and_yerevan_apply_yields_experience_and_promotion_choice() {
    let (mut game, cities) = game_with_capitals(2, 89_003);
    let minor = add_city_state(&mut game, "Hattusa");
    make_suzerain(&mut game, 0, minor);
    let science_without = game.city_yields(cities[0]).science;
    game.players[minor].civ = "Geneva".to_string();
    assert_close(game.city_yields(cities[0]).science, science_without * 1.15);
    game.at_war.insert(pair(0, 1));
    assert_close(game.city_yields(cities[0]).science, science_without);
    game.at_war.clear();

    game.players[minor].civ = "Carthage".to_string();
    let attacker = game.spawn_test_unit("warrior", 0, game.cities[&cities[0]].pos);
    let defender = game.spawn_test_unit("warrior", 1, game.cities[&cities[1]].pos);
    let opponent = game.units[&defender].clone();
    game.award_unit_combat_xp(attacker, &opponent, false, true, false);
    assert_eq!(game.units[&attacker].xp, 4);
    game.units.get_mut(&attacker).unwrap().xp = 0;
    game.players[minor].civ = "Kabul".to_string();
    game.award_unit_combat_xp(attacker, &opponent, false, true, false);
    assert_eq!(game.units[&attacker].xp, 8);

    game.players[minor].civ = "Kandy".to_string();
    let apostle = game.spawn_test_unit("apostle", 0, game.cities[&cities[0]].pos);
    assert_eq!(game.available_promotions(apostle).len(), 3);
    game.players[minor].civ = "Yerevan".to_string();
    assert_eq!(game.available_promotions(apostle).len(), 9);
}

#[test]
fn hattusa_stockholm_and_vilnius_use_resources_gpp_and_real_adjacency() {
    let (mut game, cities) = game_with_capitals(2, 89_004);
    let city = cities[0];
    let minor = add_city_state(&mut game, "Geneva");
    make_suzerain(&mut game, 0, minor);
    game.players[0].techs.insert("bronze_working".to_string());
    for position in game.cities[&city].owned_tiles.clone() {
        game.map.tiles.get_mut(&position).unwrap().resource = None;
    }
    assert_close(game.strategic_resource_rate(0, "iron"), 0.0);
    game.players[minor].civ = "Hattusa".to_string();
    assert_close(game.strategic_resource_rate(0, "iron"), 2.0);

    let campus = install_district(&mut game, city, "campus");
    game.cities
        .get_mut(&city)
        .unwrap()
        .buildings
        .push("library".to_string());
    let mut without_stockholm = game.clone();
    without_stockholm.players[minor].civ = "Geneva".to_string();
    without_stockholm.process_great_people(0);
    game.players[minor].civ = "Stockholm".to_string();
    game.process_great_people(0);
    assert_close(
        game.players[0].gpp["scientist"],
        without_stockholm.players[0].gpp["scientist"] + 1.0,
    );

    let theater = game.cities[&city]
        .owned_tiles
        .iter()
        .copied()
        .find(|position| {
            *position != game.cities[&city].pos
                && *position != campus
                && game.map.tiles[position].district.is_none()
        })
        .unwrap();
    game.map.tiles.get_mut(&theater).unwrap().district = Some("theater_square".to_string());
    game.cities
        .get_mut(&city)
        .unwrap()
        .districts
        .insert("theater_square".to_string(), theater);
    let wonder = game
        .nbrs(theater)
        .into_iter()
        .find(|position| *position != game.cities[&city].pos && *position != campus)
        .unwrap();
    game.map.tiles.get_mut(&wonder).unwrap().wonder = Some("pyramids".to_string());
    install_alliance(&mut game, 0, 1, "research", 2);
    game.players[minor].civ = "Mohenjo-Daro".to_string();
    let ordinary = game.district_yields("theater_square", theater).culture;
    assert!(ordinary > 0.0);
    game.players[minor].civ = "Vilnius".to_string();
    assert_close(
        game.district_yields("theater_square", theater).culture,
        ordinary * 2.0,
    );
}

#[test]
fn zanzibar_and_kandy_supply_luxuries_relics_and_relic_faith() {
    let (mut game, cities) = game_with_capitals(1, 89_005);
    let city = cities[0];
    let minor = add_city_state(&mut game, "Zanzibar");
    let luxuries_without = game.empire_luxuries(0);
    let amenities_without = game.city_amenity_surplus(&game.cities[&city]);
    make_suzerain(&mut game, 0, minor);
    assert_eq!(game.empire_luxuries(0), luxuries_without + 2);
    assert_eq!(
        game.city_amenity_surplus(&game.cities[&city]),
        amenities_without + 2
    );

    game.players[minor].civ = "Yerevan".to_string();
    game.cities
        .get_mut(&city)
        .unwrap()
        .buildings
        .push("temple".to_string());
    game.players[0]
        .counters
        .insert("great_work:relic".to_string(), 1);
    let faith_without = game.city_yields(city).faith;
    game.players[minor].civ = "Kandy".to_string();
    let multiplier = game.amenity_yield_mult(&game.cities[&city]);
    assert_close(
        game.city_yields(city).faith,
        faith_without + 2.0 * multiplier,
    );

    let natural_wonder = game
        .map
        .tiles
        .iter()
        .find_map(|(position, tile)| {
            tile.feature.as_ref().and_then(|feature| {
                game.rules.features[feature.as_str()]
                    .natural_wonder
                    .then_some((*position, feature.clone()))
            })
        })
        .unwrap();
    game.players[0].explored.remove(&natural_wonder.0);
    game.players[0]
        .discovered_natural_wonders
        .remove(&natural_wonder.1);
    let relics_before = game.players[0].counters["great_work:relic"];
    game.reveal(0, natural_wonder.0, 0);
    assert_eq!(
        game.players[0].counters["great_work:relic"],
        relics_before + 1
    );
}
