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
    assert_eq!(recruit_current_engineer(&mut game), "gustave_eiffel");
    assert_eq!(game.cities[&city].production, 960.0);

    game.cities
        .get_mut(&city)
        .unwrap()
        .pillaged_buildings
        .insert("workshop".to_string());
    assert_eq!(game.city_yields(city).culture, culture_before);
}
