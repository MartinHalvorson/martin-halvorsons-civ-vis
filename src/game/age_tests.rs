//! Rise & Fall Ages: the Normal-Age half of every Dedication, the era each
//! Dedication can be chosen in, and the Dark/Normal/Golden/Heroic ladder.
use super::{Action, Game};

fn two_player_game() -> Game {
    Game::new_full(2, 30, 18, 515, 300, 0, false)
}

#[test]
fn every_dedication_carries_both_halves_and_an_era_span() {
    let rules = crate::rules::Rules::embedded();
    assert_eq!(
        rules.dedications.len(),
        12,
        "Rise & Fall ships twelve Dedications"
    );
    for (name, spec) in rules.dedications.iter() {
        assert!(!spec.normal.is_empty(), "{name} has no Normal-Age text");
        assert!(!spec.golden.is_empty(), "{name} has no Golden-Age text");
        assert!(
            !spec.triggers.is_empty(),
            "{name} pays no Era Score in a Normal Age"
        );
        assert!(
            spec.eras.0 >= 1 && spec.eras.1 < crate::rules::ERA_NAMES.len(),
            "{name} spans {:?}, which is not a run of real eras",
            spec.eras
        );
        assert!(spec.eras.0 <= spec.eras.1, "{name} spans backwards");
    }
}

#[test]
fn dedications_are_offered_only_in_their_own_eras() {
    let mut game = two_player_game();
    game.players[0].dedication_choices = 1;

    // Classical: the early four are on offer and the late ones are not.
    game.world_era = 1;
    let classical = game.available_dedications(0);
    assert!(classical.contains(&"monumentality".to_string()));
    assert!(classical.contains(&"exodus_of_the_evangelists".to_string()));
    assert!(!classical.contains(&"automaton_warfare".to_string()));
    assert!(!classical.contains(&"wish_you_were_here".to_string()));

    // Information: the late ones are, and the Classical-only ones are gone.
    game.world_era = 7;
    let information = game.available_dedications(0);
    assert!(information.contains(&"automaton_warfare".to_string()));
    assert!(information.contains(&"wish_you_were_here".to_string()));
    assert!(!information.contains(&"exodus_of_the_evangelists".to_string()));
    assert!(!information.contains(&"monumentality".to_string()));
}

#[test]
fn the_two_gathering_storm_dedications_exist_and_can_be_chosen() {
    let mut game = two_player_game();
    game.world_era = 7;
    game.players[0].dedication_choices = 2;
    for dedication in ["wish_you_were_here", "bodyguard_of_lies"] {
        game.apply(
            0,
            &Action::ChooseDedication {
                dedication: dedication.to_string(),
            },
        )
        .unwrap_or_else(|error| panic!("{dedication} should be choosable: {error}"));
        assert!(game.players[0].dedications.contains(dedication));
    }
}

#[test]
fn a_normal_age_dedication_still_pays_era_score() {
    let mut game = two_player_game();
    game.players[0].age = "normal".to_string();
    game.players[0]
        .dedications
        .insert("free_inquiry".to_string());
    let before = game.players[0].era_score;

    game.dedication_trigger(0, "eureka", 1);

    assert_eq!(
        game.players[0].era_score,
        before + 1,
        "Free Inquiry pays +1 Era Score per Eureka in a Normal Age"
    );
}

#[test]
fn a_dark_age_dedication_pays_the_same_score_but_not_the_golden_bonus() {
    let mut game = two_player_game();
    game.players[0].age = "dark".to_string();
    game.players[0]
        .dedications
        .insert("monumentality".to_string());
    let before = game.players[0].era_score;

    game.dedication_trigger(0, "specialty_district", 1);

    assert_eq!(
        game.players[0].era_score,
        before + 1,
        "a Dark Age Dedication is how a civilization climbs out of it"
    );
    assert!(
        !game.dedication_active(0, "monumentality"),
        "but the Golden-Age half stays off"
    );

    game.players[0].age = "golden".to_string();
    assert!(game.dedication_active(0, "monumentality"));
}

#[test]
fn a_dedication_pays_only_for_its_own_trigger() {
    let mut game = two_player_game();
    game.players[0].age = "normal".to_string();
    game.players[0].dedications.insert("to_arms".to_string());
    let before = game.players[0].era_score;

    game.dedication_trigger(0, "eureka", 3);
    assert_eq!(game.players[0].era_score, before, "To Arms! is not a Eureka");

    game.dedication_trigger(0, "army_kill", 2);
    assert_eq!(
        game.players[0].era_score,
        before + 4,
        "two Army kills at +2 Era Score each"
    );
}

#[test]
fn a_heroic_age_still_grants_three_dedications() {
    let mut game = two_player_game();
    game.players[0].age = "dark".to_string();
    game.players[0].era_score = game.players[0].golden_age_threshold;
    game.players[1].era_score = 0;
    game.players[0].techs.insert("horseback_riding".to_string());
    game.process_eras();
    assert_eq!(game.players[0].age, "heroic");
    assert_eq!(game.players[0].dedication_choices, 3);
    assert_eq!(
        game.players[1].dedication_choices, 1,
        "and every other age grants exactly one"
    );
}

#[test]
fn an_age_transition_clears_last_age_dedications() {
    let mut game = two_player_game();
    game.players[0]
        .dedications
        .insert("monumentality".to_string());
    game.players[0].techs.insert("horseback_riding".to_string());
    game.process_eras();
    assert!(
        game.players[0].dedications.is_empty(),
        "a Dedication lasts one age"
    );
}

#[test]
fn dark_age_policy_cards_are_offered_only_inside_a_dark_age() {
    let mut game = two_player_game();
    game.world_era = 2;
    game.players[0].civics.insert("code_of_laws".to_string());

    game.players[0].age = "normal".to_string();
    let normal = game.available_policies(0);
    assert!(
        !normal.contains(&"twilight_valor".to_string()),
        "a Normal Age never sees a Dark Age card"
    );
    assert!(
        normal.contains(&"discipline".to_string()),
        "but the ordinary cards it has unlocked are still there"
    );

    game.players[0].age = "dark".to_string();
    let dark = game.available_policies(0);
    assert!(dark.contains(&"twilight_valor".to_string()));
    assert!(dark.contains(&"inquisition".to_string()));
    assert!(
        !dark.contains(&"robber_barons".to_string()),
        "Robber Barons is an Industrial-era card"
    );
    assert!(
        !dark.contains(&"automated_workforce".to_string()),
        "the Gathering Storm additions are not modelled yet"
    );
}

#[test]
fn every_dark_age_card_is_a_wildcard_with_a_cost() {
    let rules = crate::rules::Rules::embedded();
    let dark: Vec<_> = rules
        .policies
        .iter()
        .filter(|(_, spec)| spec.dark_age)
        .collect();
    assert_eq!(dark.len(), 7);
    for (name, spec) in dark {
        assert_eq!(spec.slot, "wildcard", "{name} must take a Wildcard slot");
        assert!(
            spec.civic.is_none(),
            "{name} is unlocked by an age, not a civic"
        );
        assert!(spec.eras.is_some(), "{name} needs an era span");
        assert!(
            spec.effects.values().any(|value| *value < 0.0)
                || spec
                    .effects
                    .keys()
                    .any(|key| key.starts_with("no_") || key.ends_with("_surcharge")),
            "{name} is a Dark Age card and must carry a drawback"
        );
    }
}

#[test]
fn leaving_a_dark_age_takes_the_card_back_out_of_its_slot() {
    let mut game = two_player_game();
    game.world_era = 1;
    game.players[0].age = "dark".to_string();
    game.players[0].policies.insert("twilight_valor".to_string());
    game.players[0].policies.insert("discipline".to_string());
    // Cross into the Classical era with enough Era Score for a Heroic Age.
    game.players[0].era_score = game.players[0].golden_age_threshold;
    game.players[0].techs.insert("horseback_riding".to_string());
    game.world_era = 0;

    game.process_eras();

    assert_eq!(game.players[0].age, "heroic");
    assert!(
        !game.players[0].policies.contains("twilight_valor"),
        "the Dark Age card goes back when the Dark Age does"
    );
    assert!(
        game.players[0].policies.contains("discipline"),
        "ordinary cards stay slotted"
    );
}

#[test]
fn twilight_valor_pays_on_the_attack_and_charges_for_it() {
    let mut game = two_player_game();
    game.players[0].age = "dark".to_string();
    let position = game
        .units
        .values()
        .find(|unit| unit.owner == 0)
        .map(|unit| unit.pos)
        .unwrap();
    let warrior = game.spawn_unit("warrior", 0, position);
    // A tile nobody owns: the unit is abroad.
    let away = game
        .wdisk(position, 3)
        .into_iter()
        .find(|pos| {
            game.map.tiles[pos].owner_city.is_none()
                && !game.rules.is_water(&game.map.tiles[pos])
                && *pos != position
        })
        .unwrap();
    game.units.get_mut(&warrior).unwrap().pos = away;
    game.units.get_mut(&warrior).unwrap().hp = 50;

    let heal_before = game.unit_heal_rate(warrior);
    assert!(heal_before > 0, "a wounded unit normally heals somewhere");

    game.players[0].policies.insert("twilight_valor".to_string());
    assert_eq!(
        game.unit_heal_rate(warrior),
        0,
        "Twilight Valor stops a unit healing outside your own territory"
    );
    assert_eq!(
        game.policy_effect(0, "melee_attack_combat"),
        5.0,
        "and pays +5 Combat Strength on a melee attack for it"
    );
}

#[test]
fn isolationism_closes_the_frontier_and_pays_at_home() {
    let mut game = two_player_game();
    game.players[0].age = "dark".to_string();
    let settler = game
        .player_unit_ids(0)
        .into_iter()
        .find(|unit| game.units[unit].kind == "settler")
        .unwrap();
    game.current = 0;
    game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
    let city = game.player_city_ids(0)[0];
    game.cities.get_mut(&city).unwrap().pop = 4;
    assert!(game.can_produce_unit(0, city, "settler", true, 0.0));

    game.players[0].policies.insert("isolationism".to_string());
    assert!(
        !game.can_produce_unit(0, city, "settler", true, 0.0),
        "Isolationism forbids training Settlers"
    );
    assert_eq!(game.policy_effect(0, "domestic_trade_food"), 2.0);
    assert_eq!(game.policy_effect(0, "policy_trade_route_capacity"), 1.0);
}

#[test]
fn robber_barons_costs_amenities_everywhere_it_pays() {
    let mut game = two_player_game();
    game.players[0].age = "dark".to_string();
    let settler = game
        .player_unit_ids(0)
        .into_iter()
        .find(|unit| game.units[unit].kind == "settler")
        .unwrap();
    game.current = 0;
    game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
    let city = game.player_city_ids(0)[0];
    let before = game.city_local_amenities(&game.cities[&city]);

    game.players[0].policies.insert("robber_barons".to_string());
    assert_eq!(
        game.city_local_amenities(&game.cities[&city]),
        before - 2,
        "-2 Amenities in every city is what the Gold and Production cost"
    );
}
