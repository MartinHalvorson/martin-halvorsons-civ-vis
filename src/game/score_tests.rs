//! Gathering Storm score formula, per the Civilopedia Score/Time rules:
//! 3/civic, 5/city, 2/district (4 unique), 1/building, 1/Citizen,
//! 5/Great Person, 10 founding a religion + 2 per foreign follower city,
//! 2/technology, 15/wonder, plus Era Score.
use super::{Action, Game};

fn one_city_game() -> (Game, u32) {
    let mut game = Game::new_full(2, 30, 18, 909, 200, 0, false);
    for pid in 0..2 {
        let settler = game
            .player_unit_ids(pid)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.current = pid;
        game.apply(pid, &Action::FoundCity { unit: settler }).unwrap();
    }
    game.current = 0;
    let city = game.player_city_ids(0)[0];
    (game, city)
}

#[test]
fn score_uses_the_gathering_storm_category_values() {
    let (mut game, city) = one_city_game();
    game.players[0].techs.clear();
    game.players[0].civics.clear();
    game.players[0].great_people.clear();
    game.players[0].religion = None;
    game.players[0].era_score = 0;
    let c = game.cities.get_mut(&city).unwrap();
    c.pop = 4;
    c.buildings = vec!["palace".to_string(), "monument".to_string()];
    c.districts.clear();
    c.wonders.clear();
    // 5 city + 4 Citizens + 2 buildings = 11
    let base = game.score(0);
    assert_eq!(base, 11);

    game.players[0].techs.insert("mining".to_string());
    assert_eq!(game.score(0) - base, 2, "2 points per technology");
    game.players[0].civics.insert("code_of_laws".to_string());
    assert_eq!(game.score(0) - base, 5, "3 points per civic");
    game.players[0].great_people.push("hypatia".to_string());
    assert_eq!(game.score(0) - base, 10, "5 points per Great Person");
    game.players[0].era_score = 7;
    assert_eq!(game.score(0) - base, 17, "Era Score adds its points");
}

#[test]
fn wonders_score_fifteen_and_unique_districts_score_double() {
    let (mut game, city) = one_city_game();
    let before = game.score(0);
    let pos = game.cities[&city].pos;
    game.cities
        .get_mut(&city)
        .unwrap()
        .wonders
        .insert("pyramids".to_string(), pos);
    assert_eq!(game.score(0) - before, 15, "15 points per wonder");

    let ordinary = game
        .rules
        .districts
        .iter()
        .find(|(_, spec)| spec.unique_to.is_none())
        .map(|(name, _)| name.clone())
        .unwrap();
    let unique = game
        .rules
        .districts
        .iter()
        .find(|(_, spec)| spec.unique_to.is_some())
        .map(|(name, _)| name.clone())
        .unwrap();
    let after_wonder = game.score(0);
    game.cities
        .get_mut(&city)
        .unwrap()
        .districts
        .insert(ordinary, pos);
    assert_eq!(game.score(0) - after_wonder, 2, "2 points per district");
    game.cities
        .get_mut(&city)
        .unwrap()
        .districts
        .insert(unique, pos);
    assert_eq!(
        game.score(0) - after_wonder,
        6,
        "a unique district scores 4"
    );
}

#[test]
fn religion_scores_the_founding_bonus_and_foreign_followers() {
    let (mut game, _city) = one_city_game();
    let before = game.score(0);
    game.players[0].religion = Some("Home Faith".to_string());
    assert_eq!(game.score(0) - before, 10, "10 points for founding");

    let rival = game.player_city_ids(1)[0];
    game.cities
        .get_mut(&rival)
        .unwrap()
        .pressure
        .insert("Home Faith".to_string(), 500.0);
    assert_eq!(
        game.score(0) - before,
        12,
        "2 points per foreign city following our religion"
    );
}

/// Units score nothing in Civ 6 — the previous formula gave 1 point each,
/// which paid an AI for hoarding obsolete units at the turn limit.
#[test]
fn units_do_not_score() {
    let (mut game, city) = one_city_game();
    let before = game.score(0);
    let pos = game.cities[&city].pos;
    for _ in 0..5 {
        game.spawn_test_unit("warrior", 0, pos);
    }
    assert_eq!(game.score(0), before);
}
