//! Civilization VI's stock map-size presets.
//!
//! Keep these values in one place: browser games, CLI games, map generation,
//! city-state defaults, religion limits, and observation metadata all consume
//! the same profile instead of maintaining subtly different tables.

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
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

    use crate::game::{Action, Game};

    use super::MapSize;

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
        let wonders = game
            .map
            .tiles
            .values()
            .filter(|tile| {
                tile.feature
                    .as_ref()
                    .map(|feature| game.rules.features[feature].natural_wonder)
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(wonders, size.natural_wonders);
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
