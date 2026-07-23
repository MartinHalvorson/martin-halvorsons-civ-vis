//! Civilization VI's stock game-setup presets.
//!
//! Keep these values in one place: browser games, CLI games, map generation,
//! city-state defaults, religion limits, and observation metadata all consume
//! the same profile instead of maintaining subtly different tables.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapScript {
    #[default]
    Pangaea,
    Continents,
    SmallContinents,
    InlandSea,
}

impl MapScript {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Pangaea => "pangaea",
            Self::Continents => "continents",
            Self::SmallContinents => "small_continents",
            Self::InlandSea => "inland_sea",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if id == "pangea" {
            return Some(Self::Pangaea);
        }
        CIV6_MAP_SCRIPTS
            .iter()
            .find(|script| script.id == id)
            .map(|script| script.script)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MapScriptSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    #[serde(skip)]
    pub script: MapScript,
}

pub const CIV6_MAP_SCRIPTS: [MapScriptSpec; 4] = [
    MapScriptSpec {
        id: "pangaea",
        name: "Pangaea",
        description: "One connected supercontinent surrounded by ocean.",
        script: MapScript::Pangaea,
    },
    MapScriptSpec {
        id: "continents",
        name: "Continents",
        description: "A few large landmasses separated by open water.",
        script: MapScript::Continents,
    },
    MapScriptSpec {
        id: "small_continents",
        name: "Small Continents",
        description: "Several smaller landmasses with more coastline and sea lanes.",
        script: MapScript::SmallContinents,
    },
    MapScriptSpec {
        id: "inland_sea",
        name: "Inland Sea",
        description: "A broad connected landmass surrounding a central sea.",
        script: MapScript::InlandSea,
    },
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameSpeed {
    Online,
    Quick,
    #[default]
    Standard,
    Epic,
    Marathon,
}

impl GameSpeed {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Quick => "quick",
            Self::Standard => "standard",
            Self::Epic => "epic",
            Self::Marathon => "marathon",
        }
    }

    /// Percentage of Standard costs and turn durations.
    pub const fn cost_percent(self) -> u32 {
        match self {
            Self::Online => 50,
            Self::Quick => 67,
            Self::Standard => 100,
            Self::Epic => 150,
            Self::Marathon => 300,
        }
    }

    pub const fn turn_limit(self) -> u32 {
        match self {
            Self::Online => 250,
            Self::Quick => 330,
            Self::Standard => 500,
            Self::Epic => 750,
            Self::Marathon => 1500,
        }
    }

    pub fn scale(self, standard: f64) -> f64 {
        standard * self.cost_percent() as f64 / 100.0
    }

    pub fn scale_turns(self, standard: u32) -> u32 {
        (standard as u64 * self.cost_percent() as u64).div_ceil(100).max(1) as u32
    }

    pub fn from_id(id: &str) -> Option<Self> {
        CIV6_GAME_SPEEDS
            .iter()
            .find(|speed| speed.id == id)
            .map(|speed| speed.speed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct GameSpeedSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub cost_percent: u32,
    pub turn_limit: u32,
    pub description: &'static str,
    #[serde(skip)]
    pub speed: GameSpeed,
}

pub const CIV6_GAME_SPEEDS: [GameSpeedSpec; 5] = [
    GameSpeedSpec {
        id: "online",
        name: "Online",
        cost_percent: 50,
        turn_limit: 250,
        description: "Double-speed game for online play.",
        speed: GameSpeed::Online,
    },
    GameSpeedSpec {
        id: "quick",
        name: "Quick",
        cost_percent: 67,
        turn_limit: 330,
        description: "Quick game (33% faster).",
        speed: GameSpeed::Quick,
    },
    GameSpeedSpec {
        id: "standard",
        name: "Standard",
        cost_percent: 100,
        turn_limit: 500,
        description: "Normal game speed.",
        speed: GameSpeed::Standard,
    },
    GameSpeedSpec {
        id: "epic",
        name: "Epic",
        cost_percent: 150,
        turn_limit: 750,
        description: "Prolonged game (50% slower).",
        speed: GameSpeed::Epic,
    },
    GameSpeedSpec {
        id: "marathon",
        name: "Marathon",
        cost_percent: 300,
        turn_limit: 1500,
        description: "Very prolonged game (200% slower).",
        speed: GameSpeed::Marathon,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MapSize {
    pub id: &'static str,
    pub name: &'static str,
    pub width: i32,
    pub height: i32,
    pub default_players: usize,
    pub max_players: usize,
    pub default_city_states: usize,
    pub max_city_states: usize,
    pub max_religions: usize,
    pub natural_wonders: usize,
    pub continents: usize,
}

/// The unmodified Civilization VI map-size rows (Base/Gameplay/Data/Maps.xml
/// plus the stock setup limits exposed by Advanced Setup).
pub const CIV6_MAP_SIZES: [MapSize; 6] = [
    MapSize {
        id: "duel",
        name: "Duel",
        width: 44,
        height: 26,
        default_players: 2,
        max_players: 4,
        default_city_states: 3,
        max_city_states: 6,
        max_religions: 2,
        natural_wonders: 2,
        continents: 1,
    },
    MapSize {
        id: "tiny",
        name: "Tiny",
        width: 60,
        height: 38,
        default_players: 4,
        max_players: 6,
        default_city_states: 6,
        max_city_states: 10,
        max_religions: 3,
        natural_wonders: 3,
        continents: 2,
    },
    MapSize {
        id: "small",
        name: "Small",
        width: 74,
        height: 46,
        default_players: 6,
        max_players: 10,
        default_city_states: 9,
        max_city_states: 14,
        max_religions: 4,
        natural_wonders: 4,
        continents: 3,
    },
    MapSize {
        id: "standard",
        name: "Standard",
        width: 84,
        height: 54,
        default_players: 8,
        max_players: 14,
        default_city_states: 12,
        max_city_states: 18,
        max_religions: 5,
        natural_wonders: 5,
        continents: 4,
    },
    MapSize {
        id: "large",
        name: "Large",
        width: 96,
        height: 60,
        default_players: 10,
        max_players: 16,
        default_city_states: 15,
        max_city_states: 22,
        max_religions: 6,
        natural_wonders: 6,
        continents: 5,
    },
    MapSize {
        id: "huge",
        name: "Huge",
        width: 106,
        height: 66,
        default_players: 12,
        max_players: 20,
        default_city_states: 18,
        max_city_states: 24,
        max_religions: 7,
        natural_wonders: 7,
        continents: 6,
    },
];

impl MapSize {
    /// Pick the stock size whose default major-civilization count fits the
    /// requested game. Counts above Huge retain Huge's world parameters.
    pub fn for_players(players: usize) -> &'static MapSize {
        CIV6_MAP_SIZES
            .iter()
            .find(|size| players <= size.default_players)
            .unwrap_or(&CIV6_MAP_SIZES[CIV6_MAP_SIZES.len() - 1])
    }

    pub fn from_dimensions(width: i32, height: i32) -> Option<&'static MapSize> {
        CIV6_MAP_SIZES
            .iter()
            .find(|size| size.width == width && size.height == height)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::game::{Action, Game, Item};

    use super::{GameSpeed, MapScript, MapSize, CIV6_GAME_SPEEDS};

    #[test]
    fn stock_game_speeds_scale_costs_durations_and_turn_limits() {
        let expected = [
            (GameSpeed::Online, 50, 250),
            (GameSpeed::Quick, 67, 330),
            (GameSpeed::Standard, 100, 500),
            (GameSpeed::Epic, 150, 750),
            (GameSpeed::Marathon, 300, 1500),
        ];
        assert_eq!(CIV6_GAME_SPEEDS.len(), expected.len());
        for (speed, percent, turns) in expected {
            assert_eq!(speed.cost_percent(), percent);
            assert_eq!(speed.turn_limit(), turns);
            assert_eq!(speed.scale(100.0), percent as f64);
            assert_eq!(speed.scale_turns(30), (30 * percent).div_ceil(100));
            assert_eq!(GameSpeed::from_id(speed.id()), Some(speed));
        }
    }

    #[test]
    fn every_speed_is_applied_to_live_research_growth_and_production_costs() {
        let size = MapSize::for_players(2);
        let mut game = Game::new_with_setup(
            2,
            size.width,
            size.height,
            701,
            GameSpeed::Online.turn_limit(),
            0,
            MapScript::Pangaea,
            GameSpeed::Online,
            false,
        );
        let settler = game
            .units
            .values()
            .find(|unit| unit.owner == 0 && unit.kind == "settler")
            .unwrap()
            .id;
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let monument = Item::Building {
            building: "monument".to_string(),
        };
        for speed in [
            GameSpeed::Online,
            GameSpeed::Quick,
            GameSpeed::Standard,
            GameSpeed::Epic,
            GameSpeed::Marathon,
        ] {
            game.game_speed = speed;
            let multiplier = speed.cost_percent() as f64 / 100.0;
            assert_eq!(
                game.tech_cost("pottery"),
                game.rules.techs["pottery"].cost * multiplier
            );
            assert_eq!(game.growth_cost(1), 15.0 * multiplier);
            assert_eq!(game.standard_duration(30), speed.scale_turns(30));
            assert_eq!(
                game.item_cost_for_city(0, city, &monument),
                game.rules.buildings["monument"].cost * multiplier
            );
        }

        game.map_script = MapScript::SmallContinents;
        let restored: Game = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
        assert_eq!(restored.game_speed, GameSpeed::Marathon);
        assert_eq!(restored.map_script, MapScript::SmallContinents);
    }

    #[test]
    fn requested_player_counts_use_civ6_dimensions_and_defaults() {
        let tiny = MapSize::for_players(4);
        assert_eq!((tiny.name, tiny.width, tiny.height), ("Tiny", 60, 38));
        assert_eq!(
            (
                tiny.default_city_states,
                tiny.natural_wonders,
                tiny.max_religions,
                tiny.continents
            ),
            (6, 3, 3, 2)
        );

        let small = MapSize::for_players(6);
        assert_eq!((small.name, small.width, small.height), ("Small", 74, 46));
        assert_eq!(
            (
                small.default_city_states,
                small.natural_wonders,
                small.max_religions,
                small.continents
            ),
            (9, 4, 4, 3)
        );
    }

    #[test]
    fn dimensions_round_trip_for_every_stock_size() {
        for players in [2, 4, 6, 8, 10, 12] {
            let size = MapSize::for_players(players);
            assert_eq!(
                MapSize::from_dimensions(size.width, size.height),
                Some(size)
            );
        }
    }

    fn assert_generated_profile(players: usize, seed: u64) {
        let size = MapSize::for_players(players);
        let mut game = Game::new_full(
            players,
            size.width,
            size.height,
            seed,
            50,
            size.default_city_states,
            false,
        );
        assert_eq!((game.map.width, game.map.height), (size.width, size.height));
        assert_eq!(game.map.tiles.len(), (size.width * size.height) as usize);
        assert_eq!(game.players.iter().filter(|p| !p.is_minor).count(), players);
        assert_eq!(
            game.players.iter().filter(|p| p.is_minor).count(),
            size.default_city_states
        );
        let wonders: BTreeSet<&str> = game
            .map
            .tiles
            .values()
            .filter_map(|tile| {
                let feature = tile.feature.as_deref()?;
                game.rules.features[feature]
                    .natural_wonder
                    .then_some(feature)
            })
            .collect();
        assert_eq!(
            wonders.len(),
            size.natural_wonders,
            "{} generated unexpected natural wonders: {wonders:?}",
            size.name
        );
        let continents: BTreeSet<usize> = game
            .map
            .tiles
            .values()
            .filter_map(|tile| tile.continent)
            .collect();
        assert_eq!(continents.len(), size.continents);
        assert_eq!(game.max_religions(), size.max_religions);

        for pid in 0..size.max_religions {
            game.players[pid].religion = Some(format!("Religion {pid}"));
        }
        if size.max_religions < players {
            let blocked = size.max_religions;
            game.players[blocked].prophet_pending = true;
            assert!(!game
                .legal_actions(blocked)
                .iter()
                .any(|action| matches!(action, Action::FoundReligion { .. })));
        }
    }

    #[test]
    fn every_selectable_world_generates_its_complete_profile() {
        for (players, seed) in [(2, 21), (4, 41), (6, 61), (8, 81), (10, 101), (12, 121)] {
            assert_generated_profile(players, seed);
        }
    }
}
