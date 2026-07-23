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
    game.units.get_mut(&attacker).unwrap().xp = 0;
    game.award_initiated_combat_xp(attacker, 3.0);
    assert_eq!(
        game.units[&attacker].xp, 6,
        "Kabul also doubles fixed XP from initiated district combat"
    );

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

#[test]
fn zanzibar_luxuries_each_supply_six_cities() {
    let (mut game, _) = game_with_capitals(1, 89_012);
    while game.player_city_ids(0).len() < 7 {
        let existing: Vec<Pos> = game
            .player_city_ids(0)
            .into_iter()
            .map(|city| game.cities[&city].pos)
            .collect();
        let position = game
            .map
            .tiles
            .iter()
            .find_map(|(position, tile)| {
                (tile.owner_city.is_none()
                    && game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && existing
                        .iter()
                        .all(|city| game.wdist(*city, *position) >= 3))
                .then_some(*position)
            })
            .expect("map has room for the Zanzibar allocation test");
        game.found_city_for(0, position, None);
    }
    let before: i64 = game.luxury_amenity_allocations(0).values().sum();
    let zanzibar = add_city_state(&mut game, "Zanzibar");
    make_suzerain(&mut game, 0, zanzibar);
    let after: i64 = game.luxury_amenity_allocations(0).values().sum();
    assert_eq!(
        after - before,
        12,
        "Cinnamon and Cloves provide six Amenities each"
    );
}

#[test]
fn suzerains_improve_repair_and_accumulate_city_state_resources() {
    let (mut game, _) = game_with_capitals(1, 89_013);
    let minor = add_city_state(&mut game, "Geneva");
    let position = game
        .map
        .tiles
        .iter()
        .find_map(|(position, tile)| {
            (tile.owner_city.is_none()
                && game.rules.is_passable(tile)
                && !game.rules.is_water(tile))
            .then_some(*position)
        })
        .unwrap();
    let city = game.found_city_for(minor, position, None);
    let resource = game.cities[&city]
        .owned_tiles
        .iter()
        .copied()
        .find(|tile| *tile != position)
        .unwrap();
    {
        let tile = game.map.tiles.get_mut(&resource).unwrap();
        tile.terrain = "plains".to_string();
        tile.feature = None;
        tile.resource = Some("iron".to_string());
        tile.improvement = None;
        tile.pillaged = false;
    }
    game.players[0]
        .techs
        .extend(["mining", "bronze_working"].into_iter().map(str::to_string));
    let builder = game.spawn_test_unit("builder", 0, resource);

    assert!(!game
        .valid_improvements(0, resource)
        .contains(&"mine".to_string()));
    make_suzerain(&mut game, 0, minor);
    assert!(game
        .valid_improvements(0, resource)
        .contains(&"mine".to_string()));
    game.apply(
        0,
        &Action::Improve {
            unit: builder,
            improvement: "mine".to_string(),
        },
    )
    .unwrap();
    assert_close(game.strategic_resource_rate(0, "iron"), 2.0);

    game.map.tiles.get_mut(&resource).unwrap().pillaged = true;
    game.units.get_mut(&builder).unwrap().moves_left = 2.0;
    let repair = Action::RepairImprovement { unit: builder };
    assert!(game.legal_actions(0).contains(&repair));
    game.apply(0, &repair).unwrap();
    assert!(!game.map.tiles[&resource].pillaged);
}

#[test]
fn every_roster_city_state_has_the_expected_type() {
    let expected = [
        ("Kabul", "militaristic"),
        ("Geneva", "scientific"),
        ("Carthage", "militaristic"),
        ("Hattusa", "scientific"),
        ("Mohenjo-Daro", "cultural"),
        ("Yerevan", "religious"),
        ("Zanzibar", "trade"),
        ("Auckland", "industrial"),
        ("Valletta", "militaristic"),
        ("Vilnius", "cultural"),
        ("Stockholm", "scientific"),
        ("Kandy", "religious"),
    ];
    for (city_state, kind) in expected {
        assert_eq!(Game::cs_type(city_state), kind);
    }
}

#[test]
fn valletta_purchases_city_center_and_encampment_buildings_with_discounted_walls() {
    let (mut game, cities) = game_with_capitals(2, 89_006);
    let city = cities[0];
    let valletta = add_city_state(&mut game, "Valletta");
    make_suzerain(&mut game, 1, valletta);
    install_alliance(&mut game, 0, 1, "economic", 3);
    game.players[0].techs.extend(
        ["pottery", "masonry", "bronze_working"]
            .into_iter()
            .map(str::to_string),
    );
    game.players[0].faith = 1_000.0;

    assert_eq!(
        game.building_faith_purchase_cost(0, city, "granary"),
        Some(130.0)
    );
    assert_eq!(
        game.building_faith_purchase_cost(0, city, "walls"),
        Some(80.0)
    );
    assert_eq!(game.building_faith_purchase_cost(0, city, "library"), None);
    let purchase = Action::BuyBuilding {
        city,
        building: "walls".to_string(),
        currency: "faith".to_string(),
    };
    assert!(game.legal_actions(0).contains(&purchase));
    game.apply(0, &purchase).unwrap();
    assert_close(game.players[0].faith, 920.0);
    assert!(game.cities[&city].buildings.contains(&"walls".to_string()));
    assert_eq!(game.cities[&city].wall_hp, 100);

    install_district(&mut game, city, "encampment");
    assert_eq!(
        game.building_faith_purchase_cost(0, city, "barracks"),
        Some(180.0)
    );
    game.players[0].alliances.clear();
    game.players[1].alliances.clear();
    assert_eq!(game.building_faith_purchase_cost(0, city, "granary"), None);
}

#[test]
fn final_patch_envoy_thresholds_follow_active_building_tiers() {
    let (mut game, cities) = game_with_capitals(1, 89_007);
    let city = cities[0];
    let scientific = add_city_state(&mut game, "Hattusa");
    install_district(&mut game, city, "campus");
    install_district(&mut game, city, "diplomatic_quarter");
    game.cities.get_mut(&city).unwrap().buildings.extend(
        [
            "library",
            "university",
            "research_lab",
            "consulate",
            "chancery",
        ]
        .into_iter()
        .map(str::to_string),
    );

    game.players[0].envoys = vec![(scientific, 1)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).science, 2.0);
    game.players[0].envoys = vec![(scientific, 3)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).science, 6.0);
    game.players[0].envoys = vec![(scientific, 6)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).science, 12.0);

    game.cities
        .get_mut(&city)
        .unwrap()
        .pillaged_buildings
        .insert("library".to_string());
    assert_close(game.envoy_yields(0, &game.cities[&city]).science, 11.0);
}

#[test]
fn trade_envoys_double_each_independent_commercial_and_harbor_tier() {
    let (mut game, cities) = game_with_capitals(1, 89_008);
    let city = cities[0];
    let trade = add_city_state(&mut game, "Zanzibar");
    install_district(&mut game, city, "commercial_hub");
    install_district(&mut game, city, "harbor");
    install_district(&mut game, city, "diplomatic_quarter");
    game.cities.get_mut(&city).unwrap().buildings.extend(
        [
            "market",
            "lighthouse",
            "bank",
            "shipyard",
            "stock_exchange",
            "seaport",
            "consulate",
            "chancery",
        ]
        .into_iter()
        .map(str::to_string),
    );

    game.players[0].envoys = vec![(trade, 1)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).gold, 6.0);
    game.players[0].envoys = vec![(trade, 3)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).gold, 18.0);
    game.players[0].envoys = vec![(trade, 6)];
    assert_close(game.envoy_yields(0, &game.cities[&city]).gold, 36.0);
}

#[test]
fn production_envoys_obey_unit_and_infrastructure_queues() {
    let (mut game, cities) = game_with_capitals(1, 89_009);
    let city = cities[0];
    let state = add_city_state(&mut game, "Carthage");
    game.players[0].envoys = vec![(state, 6)];
    install_district(&mut game, city, "encampment");
    install_district(&mut game, city, "industrial_zone");
    install_district(&mut game, city, "diplomatic_quarter");
    game.cities.get_mut(&city).unwrap().buildings.extend(
        [
            "barracks",
            "armory",
            "military_academy",
            "consulate",
            "chancery",
        ]
        .into_iter()
        .map(str::to_string),
    );
    game.cities.get_mut(&city).unwrap().queue = vec![Item::Unit {
        unit: "warrior".to_string(),
    }];
    assert_close(game.envoy_yields(0, &game.cities[&city]).production, 12.0);
    game.cities.get_mut(&city).unwrap().queue = vec![Item::Building {
        building: "granary".to_string(),
    }];
    assert_close(game.envoy_yields(0, &game.cities[&city]).production, 0.0);

    game.players[state].civ = "Auckland".to_string();
    game.cities.get_mut(&city).unwrap().buildings = [
        "workshop",
        "factory",
        "coal_power_plant",
        "consulate",
        "chancery",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_close(game.envoy_yields(0, &game.cities[&city]).production, 12.0);
    game.cities.get_mut(&city).unwrap().queue = vec![Item::Unit {
        unit: "warrior".to_string(),
    }];
    assert_close(game.envoy_yields(0, &game.cities[&city]).production, 0.0);
}

#[test]
fn kilwa_scales_total_type_yields_and_matching_production_categories() {
    let (mut game, cities) = game_with_capitals(1, 89_010);
    let host = cities[0];
    let second_position = game
        .map
        .tiles
        .iter()
        .find_map(|(position, tile)| {
            (tile.owner_city.is_none()
                && game.rules.is_passable(tile)
                && !game.rules.is_water(tile)
                && game.wdist(game.cities[&host].pos, *position) >= 4)
                .then_some(*position)
        })
        .unwrap();
    let second = game.found_city_for(0, second_position, Some("Kilwa Reach".to_string()));
    let first_state = add_city_state(&mut game, "Hattusa");
    let second_state = add_city_state(&mut game, "Stockholm");
    game.players[0].envoys = vec![(first_state, 3), (second_state, 3)];
    let host_position = game.cities[&host].pos;
    game.cities
        .get_mut(&host)
        .unwrap()
        .wonders
        .insert("kilwa_kisiwani".to_string(), host_position);

    let mut without_kilwa = game.clone();
    without_kilwa
        .cities
        .get_mut(&host)
        .unwrap()
        .wonders
        .remove("kilwa_kisiwani");
    assert_close(
        game.city_yields(host).science,
        without_kilwa.city_yields(host).science * 1.30,
    );
    assert_close(
        game.city_yields(second).science,
        without_kilwa.city_yields(second).science * 1.15,
    );

    game.players[first_state].civ = "Kabul".to_string();
    game.players[second_state].civ = "Carthage".to_string();
    let unit = Item::Unit {
        unit: "warrior".to_string(),
    };
    let mut no_production_kilwa = game.clone();
    no_production_kilwa
        .cities
        .get_mut(&host)
        .unwrap()
        .wonders
        .remove("kilwa_kisiwani");
    assert_close(
        game.item_prod_mult(0, host, Some(&unit)),
        no_production_kilwa.item_prod_mult(0, host, Some(&unit)) + 0.30,
    );
    assert_close(
        game.item_prod_mult(0, second, Some(&unit)),
        no_production_kilwa.item_prod_mult(0, second, Some(&unit)) + 0.15,
    );
}

#[test]
fn leading_sent_envoys_expand_borders_and_strengthen_the_city_state() {
    let (mut game, major_cities) = game_with_capitals(2, 89_011);
    let minor = add_city_state(&mut game, "Geneva");
    let minor_position = game
        .map
        .tiles
        .iter()
        .filter(|(_, tile)| {
            tile.owner_city.is_none() && game.rules.is_passable(tile) && !game.rules.is_water(tile)
        })
        .map(|(position, _)| *position)
        .max_by_key(|position| {
            major_cities
                .iter()
                .map(|city| game.wdist(game.cities[city].pos, *position))
                .sum::<i32>()
        })
        .unwrap();
    let minor_city = game.found_city_for(minor, minor_position, None);
    let initial_tiles = game.cities[&minor_city].owned_tiles.len();

    game.players[0].envoys_free = 2;
    game.do_send_envoy(0, minor).unwrap();
    assert_eq!(game.cities[&minor_city].owned_tiles.len(), initial_tiles);
    game.do_send_envoy(0, minor).unwrap();
    assert_eq!(
        game.cities[&minor_city].owned_tiles.len(),
        initial_tiles + 1
    );

    game.players[1].envoys_free = 3;
    game.do_send_envoy(1, minor).unwrap();
    game.do_send_envoy(1, minor).unwrap();
    assert_eq!(
        game.cities[&minor_city].owned_tiles.len(),
        initial_tiles + 1,
        "a first Envoy and a later tie do not expand borders"
    );
    game.do_send_envoy(1, minor).unwrap();
    assert_eq!(
        game.cities[&minor_city].owned_tiles.len(),
        initial_tiles + 2
    );
    assert_eq!(game.suzerain_of(minor), Some(1));

    install_district(&mut game, minor_city, "encampment");
    {
        let city = game.cities.get_mut(&minor_city).unwrap();
        city.encampment_hp = 100;
        city.encampment_wall_hp = 100;
    }
    let warrior = game.spawn_unit("warrior", minor, minor_position);
    let mut without_envoys = game.clone();
    without_envoys.players[1].envoys.clear();
    assert_close(
        game.unit_strength(&game.units[&warrior], true)
            - without_envoys.unit_strength(&without_envoys.units[&warrior], true),
        3.0,
    );
    assert_close(
        game.city_strength(minor_city) - without_envoys.city_strength(minor_city),
        3.0,
    );
    assert_close(
        game.encampment_strength(minor_city) - without_envoys.encampment_strength(minor_city),
        3.0,
    );

    game.players[1].gold = 1_000.0;
    game.do_levy_military(1, minor).unwrap();
    assert_eq!(game.units[&warrior].owner, 1);
    assert_eq!(game.units[&warrior].levied_from, Some(minor));
    let mut levied_without_envoys = game.clone();
    levied_without_envoys.players[1].envoys.clear();
    assert_close(
        game.unit_strength(&game.units[&warrior], true)
            - levied_without_envoys.unit_strength(&levied_without_envoys.units[&warrior], true),
        3.0,
    );
}
