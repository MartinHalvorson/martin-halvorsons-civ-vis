use super::*;

fn game_with_capitals(seed: u64) -> Game {
    let mut game = Game::new_full(2, 30, 18, seed, 200, 0, false);
    for pid in 0..2 {
        let settler = game
            .player_unit_ids(pid)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.found_city_for(pid, game.units[&settler].pos, None);
        game.remove_unit(settler);
    }
    game
}

fn found_remote_city(game: &mut Game, owner: usize, name: &str) -> u32 {
    let position = game
        .map
        .tiles
        .iter()
        .filter(|(position, tile)| {
            game.rules.is_passable(tile)
                && !game.rules.is_water(tile)
                && game.city_at(**position).is_none()
                && game
                    .cities
                    .values()
                    .all(|city| game.wdist(city.pos, **position) >= 4)
        })
        .map(|(position, _)| *position)
        .next()
        .unwrap();
    game.found_city_for(owner, position, Some(name.to_string()))
}

#[test]
fn multi_city_trade_is_permanent_mutually_valued_and_save_stable() {
    let mut game = game_with_capitals(89_101);
    let first = found_remote_city(&mut game, 0, "First Trade City");
    let second = found_remote_city(&mut game, 0, "Second Trade City");
    game.cities.get_mut(&first).unwrap().pop = 4;
    game.cities.get_mut(&second).unwrap().pop = 3;
    game.players[1].gold = 20_000.0;
    let warrior = game.spawn_unit("warrior", 0, game.cities[&first].pos);
    let builder = game.spawn_unit("builder", 0, game.cities[&first].pos);

    let cities = DealItems {
        cities: vec![first, second],
        ..DealItems::default()
    };
    let seller_floor = game.give_items_cost(0, 1, &cities);
    let buyer_ceiling = game.receive_items_value(1, 0, &cities);
    assert!(buyer_ceiling > seller_floor);
    let payment = DealItems {
        gold: (seller_floor + buyer_ceiling) / 2.0,
        ..DealItems::default()
    };
    let utilities = game.trade_utilities(0, 1, &cities, &payment);
    assert!(utilities.0 > 0.25 && utilities.1 > 0.25);

    game.do_trade(0, 1, &cities, &payment).unwrap();
    assert_eq!(game.cities[&first].owner, 1);
    assert_eq!(game.cities[&second].owner, 1);
    assert_eq!(game.cities[&first].captured_from, None);
    assert_eq!(game.cities[&first].occupied_from, None);
    assert_eq!(game.player_city_ids(0).len(), 1);
    assert!(game.active_trade_deals.is_empty());
    for unit_id in [warrior, builder] {
        let unit = &game.units[&unit_id];
        assert_eq!(unit.owner, 0);
        assert!(
            ![first, second].contains(&game.map.tiles[&unit.pos].owner_city.unwrap_or(u32::MAX))
        );
    }

    let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(restored.cities[&first].owner, 1);
    assert_eq!(restored.cities[&second].owner, 1);
}

#[test]
fn city_terms_reject_capitals_last_city_duplicates_and_explicit_great_works() {
    let mut game = game_with_capitals(89_102);
    let capital = game.player_city_ids(0)[0];
    let city = found_remote_city(&mut game, 0, "Validated Trade City");
    game.players[1].gold = 20_000.0;
    let payment = DealItems {
        gold: 5_000.0,
        ..DealItems::default()
    };

    let capital_term = DealItems {
        cities: vec![capital],
        ..DealItems::default()
    };
    assert!(game.do_trade(0, 1, &capital_term, &payment).is_err());

    let duplicate = DealItems {
        cities: vec![city, city],
        ..DealItems::default()
    };
    assert!(game.do_trade(0, 1, &duplicate, &payment).is_err());

    let mixed = DealItems {
        cities: vec![city],
        great_works: BTreeMap::from([("writing".to_string(), 1)]),
        ..DealItems::default()
    };
    assert!(game.do_trade(0, 1, &mixed, &payment).is_err());

    let lone_capital = game.player_city_ids(1)[0];
    let last_city = DealItems {
        cities: vec![lone_capital],
        ..DealItems::default()
    };
    assert!(game
        .do_trade(
            1,
            0,
            &last_city,
            &DealItems {
                gold: 5_000.0,
                ..DealItems::default()
            }
        )
        .is_err());
}
