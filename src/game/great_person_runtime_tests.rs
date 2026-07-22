use super::*;

fn scientist_game(seed: u64) -> (Game, u32, Pos) {
    let mut game = Game::new_full(1, 24, 16, seed, 300, 0, false);
    let settler = game
        .player_unit_ids(0)
        .into_iter()
        .find(|unit| game.units[unit].kind == "settler")
        .unwrap();
    let city = game.found_city_for(0, game.units[&settler].pos, None);
    let campus = install_test_district(&mut game, city, "campus");
    (game, city, campus)
}

fn recruit_current_scientist(game: &mut Game) -> String {
    let expected = game
        .current_great_person("scientist")
        .unwrap()
        .0
        .to_string();
    let cost = game.gp_cost(0, "scientist");
    game.players[0].gpp.insert("scientist".to_string(), cost);
    game.claim_great_person(0, "scientist", None).unwrap();
    assert_eq!(game.players[0].great_people.last(), Some(&expected));
    expected
}

fn recruit_current_engineer(game: &mut Game) -> String {
    let expected = game.current_great_person("engineer").unwrap().0.to_string();
    let cost = game.gp_cost(0, "engineer");
    game.players[0].gpp.insert("engineer".to_string(), cost);
    game.claim_great_person(0, "engineer", None).unwrap();
    assert_eq!(game.players[0].great_people.last(), Some(&expected));
    expected
}

fn recruit_current_merchant(game: &mut Game) -> String {
    let expected = game.current_great_person("merchant").unwrap().0.to_string();
    let cost = game.gp_cost(0, "merchant");
    game.players[0].gpp.insert("merchant".to_string(), cost);
    game.claim_great_person(0, "merchant", None).unwrap();
    assert_eq!(game.players[0].great_people.last(), Some(&expected));
    expected
}

fn recruit_current_military_person(game: &mut Game, kind: &str) -> String {
    let expected = game.current_great_person(kind).unwrap().0.to_string();
    let cost = game.gp_cost(0, kind);
    game.players[0].gpp.insert(kind.to_string(), cost);
    game.claim_great_person(0, kind, None).unwrap();
    assert_eq!(game.players[0].great_people.last(), Some(&expected));
    expected
}

#[test]
fn named_scientists_grant_exact_buildings_science_and_era_boosts() {
    let (mut game, city, _) = scientist_game(95_001);
    let initial_science = game.city_yields(city).science;
    let initial_boosts = game.players[0].boosted_techs.clone();

    assert_eq!(recruit_current_scientist(&mut game), "hypatia");
    assert!(game.cities[&city]
        .buildings
        .contains(&"library".to_string()));
    assert_eq!(game.players[0].boosted_techs, initial_boosts);
    assert_eq!(
        game.city_yields(city).science - initial_science,
        game.rules.buildings["library"].yields.science + 1.0
    );

    let before_newton = game.city_yields(city).science;
    assert_eq!(recruit_current_scientist(&mut game), "isaac_newton");
    assert!(game.cities[&city]
        .buildings
        .contains(&"university".to_string()));
    assert_eq!(game.players[0].boosted_techs, initial_boosts);
    assert_eq!(
        game.city_yields(city).science - before_newton,
        game.rules.buildings["university"].yields.science + 2.0
    );

    game.cities
        .get_mut(&city)
        .unwrap()
        .buildings
        .push("research_lab".to_string());
    game.cities
        .get_mut(&city)
        .unwrap()
        .building_eras
        .insert("research_lab".to_string(), game.world_era);
    let before_einstein = game.city_yields(city).science;
    let boosts_before_einstein = game.players[0].boosted_techs.clone();

    assert_eq!(recruit_current_scientist(&mut game), "albert_einstein");
    assert_eq!(game.city_yields(city).science - before_einstein, 4.0);
    let new_boosts: Vec<&String> = game.players[0]
        .boosted_techs
        .difference(&boosts_before_einstein)
        .collect();
    assert_eq!(new_boosts.len(), 1);
    assert!((5..=6).contains(&game.rules.techs[new_boosts[0].as_str()].era));

    let active_science = game.city_yields(city).science;
    game.cities
        .get_mut(&city)
        .unwrap()
        .pillaged_buildings
        .insert("research_lab".to_string());
    assert_eq!(active_science - game.city_yields(city).science, 7.0);
    game.cities
        .get_mut(&city)
        .unwrap()
        .pillaged_buildings
        .remove("research_lab");

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(restored.city_yields(city), game.city_yields(city));
    assert_eq!(
        restored.players[0]
            .counters
            .get("great_person:research_lab_science"),
        Some(&4)
    );
}

#[test]
fn great_scientist_yield_bonuses_apply_to_unique_building_families() {
    let (mut game, city, _) = scientist_game(95_002);
    game.players[0].civ = "Arabia".to_string();
    game.cities
        .get_mut(&city)
        .unwrap()
        .buildings
        .extend(["library".to_string(), "madrasa".to_string()]);
    game.players[0]
        .counters
        .insert("great_person:library_science".to_string(), 1);
    game.players[0]
        .counters
        .insert("great_person:university_science".to_string(), 2);
    let with_bonuses = game.city_yields(city).science;

    game.players[0]
        .counters
        .remove("great_person:library_science");
    game.players[0]
        .counters
        .remove("great_person:university_science");
    assert_eq!(with_bonuses - game.city_yields(city).science, 3.0);
}

#[test]
fn named_engineers_apply_exact_charges_wonder_gates_and_workshop_culture() {
    let mut game = Game::new_full(1, 24, 16, 95_003, 300, 0, false);
    let settler = game
        .player_unit_ids(0)
        .into_iter()
        .find(|unit| game.units[unit].kind == "settler")
        .unwrap();
    let city = game.found_city_for(0, game.units[&settler].pos, None);
    install_test_district(&mut game, city, "industrial_zone");
    game.cities
        .get_mut(&city)
        .unwrap()
        .buildings
        .push("workshop".to_string());
    let wonder_site = game.cities[&city]
        .owned_tiles
        .iter()
        .copied()
        .find(|position| *position != game.cities[&city].pos)
        .unwrap();

    game.players[0].gpp.insert("engineer".to_string(), 60.0);
    assert_eq!(game.current_great_person("engineer").unwrap().1.era, 2);
    assert!(game.claim_great_person(0, "engineer", None).is_err());
    assert!(!game.retired_great_people.contains("imhotep"));

    game.cities.get_mut(&city).unwrap().queue = vec![Item::Wonder {
        wonder: "pyramids".to_string(),
        pos: wonder_site,
    }];
    assert_eq!(recruit_current_engineer(&mut game), "imhotep");
    assert_eq!(game.cities[&city].production, 700.0);

    game.cities.get_mut(&city).unwrap().queue.clear();
    let culture_before = game.city_yields(city).culture;
    let boosts_before = game.players[0].boosted_techs.clone();
    assert_eq!(recruit_current_engineer(&mut game), "leonardo_da_vinci");
    assert_eq!(game.city_yields(city).culture - culture_before, 3.0);
    let new_boosts: Vec<&String> = game.players[0]
        .boosted_techs
        .difference(&boosts_before)
        .collect();
    assert_eq!(new_boosts.len(), 1);
    assert_eq!(game.rules.techs[new_boosts[0].as_str()].era, 5);

    game.cities.get_mut(&city).unwrap().production = 0.0;
    game.cities.get_mut(&city).unwrap().queue = vec![Item::Wonder {
        wonder: "eiffel_tower".to_string(),
        pos: wonder_site,
    }];
    assert_eq!(game.current_great_person("engineer").unwrap().1.era, 4);
    assert_eq!(recruit_current_engineer(&mut game), "gustave_eiffel");
    assert_eq!(game.cities[&city].production, 960.0);

    game.cities
        .get_mut(&city)
        .unwrap()
        .pillaged_buildings
        .insert("workshop".to_string());
    assert_eq!(game.city_yields(city).culture, culture_before);
}

#[test]
fn named_merchants_annex_tiles_and_apply_exact_trade_and_oil_effects() {
    let mut game = Game::new_full(2, 28, 18, 95_004, 300, 0, false);
    let mut cities = Vec::new();
    for pid in 0..2 {
        let settler = game
            .player_unit_ids(pid)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        cities.push(game.found_city_for(pid, game.units[&settler].pos, None));
    }
    let merchant_city = cities[0];
    let foreign_city = cities[1];
    install_test_district(&mut game, merchant_city, "commercial_hub");
    game.players[0].civics.insert("foreign_trade".to_string());

    let gold_before_crassus = game.players[0].gold;
    let tiles_before_crassus = game.cities[&merchant_city].owned_tiles.len();
    assert_eq!(
        recruit_current_merchant(&mut game),
        "marcus_licinius_crassus"
    );
    assert_eq!(game.players[0].gold - gold_before_crassus, 180.0);
    assert_eq!(
        game.cities[&merchant_city].owned_tiles.len() - tiles_before_crassus,
        3
    );

    game.routes.push(TradeRoute {
        origin: foreign_city,
        dest: merchant_city,
        owner: 1,
        ends: game.turn + 30,
    });
    let foreign_origin_gold = game.city_yields(foreign_city).gold;
    let merchant_destination_gold = game.city_yields(merchant_city).gold;
    let traders_before = game
        .units
        .values()
        .filter(|unit| unit.owner == 0 && unit.kind == "trader")
        .count();
    let capacity_before = game.trade_capacity(0);
    assert_eq!(recruit_current_merchant(&mut game), "marco_polo");
    assert_eq!(game.trade_capacity(0) - capacity_before, 1);
    assert_eq!(
        game.units
            .values()
            .filter(|unit| unit.owner == 0 && unit.kind == "trader")
            .count()
            - traders_before,
        1
    );
    assert_eq!(
        game.city_yields(foreign_city).gold - foreign_origin_gold,
        2.0
    );
    assert_eq!(
        game.city_yields(merchant_city).gold - merchant_destination_gold,
        2.0
    );

    let resource_tiles: Vec<Pos> = game.cities[&foreign_city]
        .owned_tiles
        .iter()
        .copied()
        .filter(|position| *position != game.cities[&foreign_city].pos)
        .take(2)
        .collect();
    assert_eq!(resource_tiles.len(), 2);
    for (position, resource, improvement) in [
        (resource_tiles[0], "iron", "mine"),
        (resource_tiles[1], "horses", "pasture"),
    ] {
        let tile = game.map.tiles.get_mut(&position).unwrap();
        tile.resource = Some(resource.to_string());
        tile.improvement = Some(improvement.to_string());
        tile.pillaged = false;
    }
    game.routes.push(TradeRoute {
        origin: merchant_city,
        dest: foreign_city,
        owner: 0,
        ends: game.turn + 30,
    });
    game.players[0].techs.insert("refining".to_string());
    let rockefeller_route_gold = game.city_yields(merchant_city).gold;
    assert_eq!(recruit_current_merchant(&mut game), "john_rockefeller");
    assert_eq!(
        game.city_yields(merchant_city).gold - rockefeller_route_gold,
        4.0
    );
    assert_eq!(game.strategic_resource_rate(0, "oil"), 3.0);
    game.process_strategic_resources(0);
    assert_eq!(game.strategic_stockpile(0, "oil"), 3.0);

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(
        restored.city_yields(merchant_city),
        game.city_yields(merchant_city)
    );
    assert_eq!(restored.strategic_resource_rate(0, "oil"), 3.0);
}

#[test]
fn named_generals_promote_or_form_exactly_one_land_unit() {
    let mut game = Game::new_full(1, 24, 16, 95_005, 300, 0, false);
    let position = game.player_unit_ids(0).into_iter().next().unwrap();
    let position = game.units[&position].pos;
    let target = game.spawn_unit("swordsman", 0, position);
    let untouched = game.spawn_unit("warrior", 0, position);

    assert_eq!(
        recruit_current_military_person(&mut game, "general"),
        "hannibal_barca"
    );
    assert!(game.promotion_pending(target));
    assert_eq!(game.units[&untouched].xp, 0);

    assert_eq!(
        recruit_current_military_person(&mut game, "general"),
        "el_cid"
    );
    assert_eq!(game.units[&target].formation, 1);
    assert_eq!(game.units[&untouched].formation, 0);

    assert_eq!(
        recruit_current_military_person(&mut game, "general"),
        "napoleon_bonaparte"
    );
    assert_eq!(game.units[&target].formation, 2);
    assert_eq!(game.units[&untouched].formation, 0);
}

#[test]
fn named_admirals_apply_city_trade_building_loyalty_and_flanking_effects() {
    let mut game = Game::new_full(2, 28, 18, 95_006, 300, 0, false);
    let mut cities = Vec::new();
    for pid in 0..2 {
        let settler = game
            .player_unit_ids(pid)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        cities.push(game.found_city_for(pid, game.units[&settler].pos, None));
    }
    let admiral_city = cities[0];
    let foreign_city = cities[1];
    install_test_district(&mut game, admiral_city, "harbor");
    game.players[0].civics.extend([
        "foreign_trade".to_string(),
        "military_tradition".to_string(),
    ]);

    assert_eq!(
        recruit_current_military_person(&mut game, "admiral"),
        "themistocles"
    );
    assert_eq!(
        game.cities[&admiral_city].great_person_loyalty_per_turn,
        2.0
    );
    game.cities.get_mut(&admiral_city).unwrap().loyalty = 50.0;
    let mut loyalty_baseline: Game =
        serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    loyalty_baseline
        .cities
        .get_mut(&admiral_city)
        .unwrap()
        .great_person_loyalty_per_turn = 0.0;
    game.process_loyalty(0);
    loyalty_baseline.process_loyalty(0);
    assert_eq!(
        game.cities[&admiral_city].loyalty - loyalty_baseline.cities[&admiral_city].loyalty,
        2.0
    );
    game.cities.get_mut(&admiral_city).unwrap().loyalty = 100.0;

    game.routes.push(TradeRoute {
        origin: foreign_city,
        dest: admiral_city,
        owner: 1,
        ends: game.turn + 30,
    });
    let origin_gold = game.city_yields(foreign_city).gold;
    let destination_gold = game.city_yields(admiral_city).gold;
    let capacity = game.trade_capacity(0);
    let traders = game
        .units
        .values()
        .filter(|unit| unit.owner == 0 && unit.kind == "trader")
        .count();
    assert_eq!(
        recruit_current_military_person(&mut game, "admiral"),
        "zheng_he"
    );
    assert_eq!(game.trade_capacity(0) - capacity, 1);
    assert_eq!(
        game.units
            .values()
            .filter(|unit| unit.owner == 0 && unit.kind == "trader")
            .count()
            - traders,
        1
    );
    assert_eq!(game.city_yields(foreign_city).gold - origin_gold, 2.0);
    assert_eq!(game.city_yields(admiral_city).gold - destination_gold, 2.0);

    let target = game
        .map
        .tiles
        .keys()
        .copied()
        .find(|position| game.nbrs(*position).len() == 6)
        .unwrap();
    let ring = game.nbrs(target);
    for position in std::iter::once(target).chain(ring.iter().copied()) {
        let tile = game.map.tiles.get_mut(&position).unwrap();
        tile.terrain = "coast".to_string();
        tile.feature = None;
    }
    let attacker = game.spawn_unit("galley", 0, ring[0]);
    game.spawn_unit("galley", 0, ring[1]);
    game.spawn_unit("galley", 1, target);
    assert_eq!(game.flanking_bonus(attacker, target), 2.0);
    assert_eq!(
        recruit_current_military_person(&mut game, "admiral"),
        "horatio_nelson"
    );
    assert!(game.cities[&admiral_city]
        .buildings
        .contains(&"lighthouse".to_string()));
    assert!(game.cities[&admiral_city]
        .buildings
        .contains(&"shipyard".to_string()));
    assert_eq!(game.flanking_bonus(attacker, target), 3.0);

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(restored.flanking_bonus(attacker, target), 3.0);
    assert_eq!(
        restored.cities[&admiral_city].great_person_loyalty_per_turn,
        2.0
    );
}
