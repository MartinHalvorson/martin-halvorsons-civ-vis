//! Map generation (mirrors civvis/mapgen.py).
use std::collections::{BTreeMap, BTreeSet};

use crate::fractal::Fractal;
use crate::rng::Rng;
use crate::rules::Rules;
use crate::setup::MapScript;
use crate::world::WorldMap;
use crate::{hex, Pos};

fn offset_region(
    wm: &WorldMap,
    col_start: i32,
    col_end: i32,
    row_start: i32,
    row_end: i32,
) -> BTreeSet<Pos> {
    (row_start.max(0)..row_end.min(wm.height))
        .flat_map(|row| {
            (col_start.max(0)..col_end.min(wm.width)).map(move |col| hex::offset_to_axial(col, row))
        })
        .filter(|pos| wm.tiles.contains_key(pos))
        .collect()
}

/// Grow a single guaranteed-connected landmass inside an allowed region.
fn grow_blob(
    wm: &WorldMap,
    allowed: &BTreeSet<Pos>,
    seed: Pos,
    target: usize,
    rng: &mut Rng,
) -> BTreeSet<Pos> {
    if !allowed.contains(&seed) || target == 0 {
        return BTreeSet::new();
    }
    let mut land = BTreeSet::from([seed]);
    let mut frontier = vec![seed];
    for _ in 0..(50 * wm.width * wm.height) {
        if land.len() >= target.min(allowed.len()) || frontier.is_empty() {
            break;
        }
        let index = rng.below(frontier.len());
        let current = frontier[index];
        let candidates: Vec<Pos> = hex::neighbors(current)
            .into_iter()
            .map(|neighbor| hex::canon(neighbor, wm.width))
            .filter(|neighbor| allowed.contains(neighbor) && !land.contains(neighbor))
            .collect();
        if candidates.is_empty() {
            frontier.swap_remove(index);
            continue;
        }
        let next = candidates[rng.below(candidates.len())];
        land.insert(next);
        frontier.push(next);
        if rng.chance(0.18) {
            frontier.swap_remove(index);
        }
    }
    land
}

fn generate_land(
    wm: &WorldMap,
    script: MapScript,
    num_major_spawns: usize,
    rng: &mut Rng,
) -> BTreeSet<Pos> {
    let width = wm.width;
    let height = wm.height;
    let area = (width * height) as usize;
    match script {
        MapScript::Pangaea => {
            // A compact oval gives every seat comparable hinterland while
            // retaining a single coast-to-coast supercontinent. The stock
            // scripts cut their coastline out of a fractal rather than a
            // curve, which is what produces bays, peninsulas and the odd
            // offshore island; the oval only decides where the sea level sits.
            let center_col = (width - 1) as f64 / 2.0;
            let center_row = (height - 1) as f64 / 2.0;
            let radius_col = width as f64 * 0.39;
            let radius_row = height as f64 * 0.343;
            let shore = Fractal::new(rng, width, height, 4);
            let mut land = BTreeSet::new();
            for row in 1..height - 1 {
                for col in 0..width {
                    let x = (col as f64 - center_col) / radius_col;
                    let y = (row as f64 - center_row) / radius_row;
                    let ragged = 1.0 + 0.30 * (shore.at(col, row) as f64 / 255.0 - 0.5) * 2.0;
                    if (x * x + y * y).sqrt() <= ragged {
                        land.insert(hex::offset_to_axial(col, row));
                    }
                }
            }
            land
        }
        MapScript::Continents => {
            let gap = (width / 18).max(2);
            let midpoint = width / 2;
            let regions = [
                (gap, midpoint - gap, 2, height - 2),
                (midpoint + gap, width - gap, 2, height - 2),
            ];
            let mut land = BTreeSet::new();
            let per_continent = (area as f64 * 0.21) as usize;
            for (left, right, top, bottom) in regions {
                let allowed = offset_region(wm, left, right, top, bottom);
                let seed = hex::offset_to_axial((left + right) / 2, (top + bottom) / 2);
                land.extend(grow_blob(wm, &allowed, seed, per_continent, rng));
            }
            land
        }
        MapScript::SmallContinents => {
            let count = num_major_spawns.div_ceil(2).clamp(4, 8);
            let columns = if count <= 4 { 2 } else { 3 };
            let rows = count.div_ceil(columns);
            let per_island = ((area as f64 * 0.36) as usize / count).max(12);
            let mut land = BTreeSet::new();
            for index in 0..count {
                let column = index % columns;
                let row = index / columns;
                let left = (column * width as usize / columns) as i32 + 2;
                let right = ((column + 1) * width as usize / columns) as i32 - 2;
                let top = (row * height as usize / rows) as i32 + 2;
                let bottom = ((row + 1) * height as usize / rows) as i32 - 2;
                let allowed = offset_region(wm, left, right, top, bottom);
                let seed = hex::offset_to_axial((left + right) / 2, (top + bottom) / 2);
                land.extend(grow_blob(wm, &allowed, seed, per_island, rng));
            }
            land
        }
        MapScript::InlandSea => {
            let center_col = (width - 1) as f64 / 2.0;
            let center_row = (height - 1) as f64 / 2.0;
            let radius_col = width as f64 * 0.34;
            let radius_row = height as f64 * 0.30;
            let shore = Fractal::new(rng, width, height, 4);
            let mut land = BTreeSet::new();
            for row in 0..height {
                for col in 0..width {
                    let edge = col < 2 || col >= width - 2 || row < 2 || row >= height - 2;
                    let x = (col as f64 - center_col) / radius_col;
                    let y = (row as f64 - center_row) / radius_row;
                    // The same fractal shore, applied to the sea's edge, gives
                    // the basin gulfs and headlands instead of a drawn ellipse.
                    let ragged = 1.0 + 0.26 * (shore.at(col, row) as f64 / 255.0 - 0.5) * 2.0;
                    if edge || (x * x + y * y).sqrt() >= ragged {
                        land.insert(hex::offset_to_axial(col, row));
                    }
                }
            }
            land
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn generate(
    rules: &Rules,
    width: i32,
    height: i32,
    num_major_spawns: usize,
    num_minor_spawns: usize,
    num_natural_wonders: usize,
    num_continents: usize,
    rng: &mut Rng,
) -> (WorldMap, Vec<Pos>) {
    generate_with_script(
        rules,
        width,
        height,
        num_major_spawns,
        num_minor_spawns,
        num_natural_wonders,
        num_continents,
        MapScript::Pangaea,
        rng,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn generate_with_script(
    rules: &Rules,
    width: i32,
    height: i32,
    num_major_spawns: usize,
    num_minor_spawns: usize,
    num_natural_wonders: usize,
    num_continents: usize,
    script: MapScript,
    rng: &mut Rng,
) -> (WorldMap, Vec<Pos>) {
    let mut wm = WorldMap::new(width, height);

    // --- landmass topology selected by the stock-style map script
    let land = generate_land(&wm, script, num_major_spawns, rng);

    let land_list: Vec<Pos> = land.iter().cloned().collect();
    let latitude = |pos: Pos| -> f64 {
        let (_, row) = hex::axial_to_offset(pos.0, pos.1);
        (2.0 * row as f64 / (height - 1).max(1) as f64 - 1.0).abs()
    };

    // --- relief, then climate. The stock generator settles elevation first
    // (MountainsCliffs.lua) and only then paints biomes over it, because the
    // mountain fractal has to be free of the latitude bands to run across them.
    apply_tectonics(&mut wm, &land, rng);
    assign_biomes(&mut wm, &land_list, rng);

    // --- coast. A shelf is one tile of shallow water plus the stock's three
    // expansion passes, each giving a quarter of the Ocean tiles that already
    // touch shallow water their own turn to become Coast. Shelves therefore
    // vary from one tile in the open sea to five or more in a broad bay,
    // instead of a uniform outline traced around every landmass.
    let coastal: Vec<Pos> = wm
        .tiles
        .iter()
        .filter(|(pos, t)| {
            t.terrain == "ocean"
                && hex::neighbors(**pos)
                    .iter()
                    .any(|n| land.contains(&hex::canon(*n, width)))
        })
        .map(|(pos, _)| *pos)
        .collect();
    for pos in coastal {
        wm.tiles.get_mut(&pos).unwrap().terrain = "coast".into();
    }
    for _ in 0..3 {
        let expansion: Vec<Pos> = wm
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                tile.terrain == "ocean"
                    && hex::neighbors(**pos)
                        .into_iter()
                        .map(|neighbor| hex::canon(neighbor, width))
                        .any(|neighbor| {
                            wm.tiles
                                .get(&neighbor)
                                .is_some_and(|tile| tile.terrain == "coast")
                        })
            })
            .map(|(pos, _)| *pos)
            .collect();
        for pos in expansion {
            if rng.below(4) == 0 {
                wm.tiles.get_mut(&pos).unwrap().terrain = "coast".into();
            }
        }
    }

    // Coastal cliffs are shared edge features rather than tile terrain.
    // Generate from the land side and mirror the edge onto the water tile;
    // this makes embark/disembark legality exact at bays and narrow points.
    let mut cliff_edges = Vec::new();
    for &pos in &land_list {
        if wm.tiles[&pos].terrain == "mountain" {
            continue;
        }
        for neighbor in hex::neighbors(pos)
            .into_iter()
            .map(|neighbor| hex::canon(neighbor, width))
        {
            if wm
                .tiles
                .get(&neighbor)
                .is_some_and(|tile| matches!(tile.terrain.as_str(), "coast" | "ocean"))
                && rng.chance(0.35)
            {
                cliff_edges.push((pos, neighbor));
            }
        }
    }
    for (land, water) in cliff_edges {
        wm.set_cliff_edge(land, water, true);
    }

    // --- rivers: connected chains along shared hex edges, as in Civ VI.
    // Build each river upstream from a guaranteed coastal outlet. Walking the
    // edge graph (rather than the tile-center graph) keeps every consecutive
    // segment joined at a hex corner and never sends a channel through a tile.
    generate_rivers(&mut wm, &land_list, rng);

    // --- tribal villages (goody huts), roughly 1 per 40 land tiles
    for pos in &land_list {
        let t = &wm.tiles[pos];
        if t.terrain == "mountain" || t.has_river() {
            continue;
        }
        if rng.f64() < 0.025 {
            wm.tiles.get_mut(pos).unwrap().improvement = Some("goody_hut".into());
        }
    }

    // --- tectonic and polar features
    // Volcanoes replace a small, well-spaced subset of mountain tiles. Every
    // candidate needs exposed land at its foot so the volcano reads as part of
    // the landscape and can seed the volcanic soil produced by old eruptions.
    let mut volcano_candidates: Vec<Pos> = land_list
        .iter()
        .copied()
        .filter(|position| wm.tiles[position].terrain == "mountain")
        .filter(|position| {
            hex::neighbors(*position)
                .into_iter()
                .map(|neighbor| hex::canon(neighbor, width))
                .any(|neighbor| {
                    wm.tiles.get(&neighbor).is_some_and(|tile| {
                        !matches!(tile.terrain.as_str(), "mountain" | "coast" | "ocean")
                    })
                })
        })
        .collect();
    for index in (1..volcano_candidates.len()).rev() {
        let other = rng.below(index + 1);
        volcano_candidates.swap(index, other);
    }
    let volcano_target = (land_list.len() / 180).max(1);
    let mut volcanoes = Vec::new();
    for position in volcano_candidates {
        if volcanoes.len() >= volcano_target {
            break;
        }
        if volcanoes
            .iter()
            .all(|other| hex::wdistance(position, *other, width) >= 4)
        {
            wm.tiles.get_mut(&position).unwrap().feature = Some("volcano".into());
            volcanoes.push(position);
        }
    }

    // Ancient eruption deposits make volcanoes legible even while dormant.
    // Guarantee one deposit where geography allows, then scatter a few more
    // without consuming the RNG differently for later per-tile feature rolls.
    for volcano in &volcanoes {
        let mut foothills: Vec<Pos> = hex::neighbors(*volcano)
            .into_iter()
            .map(|neighbor| hex::canon(neighbor, width))
            .filter(|neighbor| {
                wm.tiles.get(neighbor).is_some_and(|tile| {
                    !matches!(tile.terrain.as_str(), "mountain" | "coast" | "ocean")
                        && tile.feature.is_none()
                })
            })
            .collect();
        for index in (1..foothills.len()).rev() {
            let other = rng.below(index + 1);
            foothills.swap(index, other);
        }
        for (index, position) in foothills.into_iter().enumerate() {
            if index == 0 || rng.chance(0.28) {
                wm.tiles.get_mut(&position).unwrap().feature = Some("volcanic_soil".into());
            }
        }
    }

    // Fissures follow tectonic relief rather than appearing on arbitrary flat
    // tiles. Spacing them preserves their value as recognizable landmarks.
    let mut fissure_candidates: Vec<Pos> = land_list
        .iter()
        .copied()
        .filter(|position| {
            let tile = &wm.tiles[position];
            tile.terrain != "mountain"
                && tile.feature.is_none()
                && hex::neighbors(*position)
                    .into_iter()
                    .map(|neighbor| hex::canon(neighbor, width))
                    .any(|neighbor| {
                        wm.tiles.get(&neighbor).is_some_and(|neighbor_tile| {
                            neighbor_tile.terrain == "mountain"
                                || neighbor_tile.feature.as_deref() == Some("volcano")
                        })
                    })
        })
        .collect();
    for index in (1..fissure_candidates.len()).rev() {
        let other = rng.below(index + 1);
        fissure_candidates.swap(index, other);
    }
    let fissure_target = (land_list.len() / 140).max(1);
    let mut fissures = Vec::new();
    for position in fissure_candidates {
        if fissures.len() >= fissure_target {
            break;
        }
        if fissures
            .iter()
            .all(|other| hex::wdistance(position, *other, width) >= 3)
        {
            wm.tiles.get_mut(&position).unwrap().feature = Some("geothermal_fissure".into());
            fissures.push(position);
        }
    }

    // Polar sea ice occupies both Ocean and Coast. Latitude controls density,
    // leaving navigable gaps instead of drawing an artificial solid wall.
    let polar_water: Vec<Pos> = wm
        .tiles
        .iter()
        .filter(|(position, tile)| {
            matches!(tile.terrain.as_str(), "coast" | "ocean") && latitude(**position) > 0.82
        })
        .map(|(position, _)| *position)
        .collect();
    for position in polar_water {
        let chance = ((latitude(position) - 0.82) / 0.18 * 0.72).clamp(0.0, 0.72);
        if rng.chance(chance) {
            wm.tiles.get_mut(&position).unwrap().feature = Some("ice".into());
        }
    }

    // --- vegetative, wetland and river-basin features, and the reefs that
    // supply the Campus's major Gathering Storm adjacency source.
    add_features(&mut wm, &land, rng);

    // --- natural wonders: use the stock per-map-size count and the actual
    // footprint of each modeled wonder. Multi-tile wonders are grown as a
    // connected cluster so discovery, adjacency and yields operate on every
    // constituent tile rather than on a single representative hex.
    //
    // The stock generator also spreads them out: `NaturalWonderGenerator`
    // rejects a candidate plot that sits too near a wonder it has already
    // drawn, so no two of them ever share a border and a single region never
    // collects the map's whole allowance. Two wonders that prefer the same
    // biome — Yosemite and Mount Everest both want mountains — otherwise
    // settle onto the same range and read as one oversized feature. The
    // separation is a preference, not a quota: it is relaxed one ring at a
    // time down to `MIN_WONDER_SEPARATION` before a wonder is allowed to
    // place unconstrained, so a cramped map still receives its full count.
    let mut placed_wonder_tiles: Vec<Pos> = Vec::new();
    let mut wonder_names = [
        "great_barrier_reef",
        "crater_lake",
        "pantanal",
        "uluru",
        "yosemite",
        "dead_sea",
        "mount_everest",
        "pamukkale",
    ];
    for index in (1..wonder_names.len()).rev() {
        let other = rng.below(index + 1);
        wonder_names.swap(index, other);
    }
    for wonder in wonder_names.iter().take(num_natural_wonders) {
        let footprint = match *wonder {
            "great_barrier_reef" | "yosemite" | "dead_sea" | "pamukkale" => 2,
            "mount_everest" => 3,
            "pantanal" => 4,
            _ => 1,
        };
        let preferred = |t: &crate::world::Tile| {
            if t.feature.is_some() || t.resource.is_some() {
                return false;
            }
            match *wonder {
                "great_barrier_reef" => t.terrain == "coast",
                "crater_lake" => {
                    matches!(t.terrain.as_str(), "grassland" | "plains" | "tundra")
                        && !t.hills
                        && !t.has_river()
                }
                "pantanal" => matches!(t.terrain.as_str(), "grassland" | "plains") && !t.hills,
                "uluru" => t.terrain == "desert" && !t.hills,
                "yosemite" | "mount_everest" => t.terrain == "mountain",
                "dead_sea" => {
                    matches!(t.terrain.as_str(), "desert" | "plains") && !t.hills && !t.has_river()
                }
                "pamukkale" => {
                    matches!(t.terrain.as_str(), "desert" | "grassland" | "plains") && !t.hills
                }
                _ => false,
            }
        };
        // A tile is far enough from the wonders already drawn when every one
        // of their tiles is at least `separation` hexes away. `separation`
        // of 1 is no constraint at all, which is what the final unconstrained
        // attempt uses.
        let far_enough = |position: Pos, separation: i32| {
            placed_wonder_tiles
                .iter()
                .all(|placed| hex::wdistance(position, *placed, width) >= separation)
        };
        let cluster_from = |anchor: Pos, preferred_only: bool, separation: i32| {
            let mut cluster = vec![anchor];
            while cluster.len() < footprint {
                let mut frontier: Vec<Pos> = cluster
                    .iter()
                    .flat_map(|position| hex::neighbors(*position))
                    .map(|position| hex::canon(position, width))
                    .filter(|position| wm.tiles.contains_key(position))
                    .filter(|position| !cluster.contains(position))
                    .filter(|position| far_enough(*position, separation))
                    .filter(|position| {
                        let tile = &wm.tiles[position];
                        if preferred_only {
                            preferred(tile)
                        } else if *wonder == "great_barrier_reef" {
                            tile.terrain == "coast"
                                && tile.feature.is_none()
                                && tile.resource.is_none()
                        } else {
                            !matches!(tile.terrain.as_str(), "ocean" | "coast")
                                && tile.feature.is_none()
                                && tile.resource.is_none()
                        }
                    })
                    .collect();
                frontier.sort();
                frontier.dedup();
                if frontier.is_empty() {
                    return None;
                }
                cluster.push(frontier[0]);
            }
            Some(cluster)
        };
        let preferred_sites: Vec<Pos> = wm
            .tiles
            .iter()
            .filter(|(_, t)| preferred(t))
            .map(|(p, _)| *p)
            .collect();
        // Very unusual seeds can lack a large enough preferred biome. Keep
        // the correct footprint and map-size count by shaping an otherwise
        // empty connected region into the wonder's terrain family.
        let shaped_sites: Vec<Pos> = wm
            .tiles
            .iter()
            .filter(|(_, t)| {
                ((*wonder == "great_barrier_reef" && t.terrain == "coast")
                    || (*wonder != "great_barrier_reef"
                        && !matches!(t.terrain.as_str(), "ocean" | "coast")))
                    && t.feature.is_none()
                    && t.resource.is_none()
            })
            .map(|(p, _)| *p)
            .collect();
        // Sites are tried in order of how far each one departs from the ideal:
        // the wonder's own biome at the widest spacing, then narrower rings,
        // then the shaped fallback down the same ladder. Rewriting a region
        // into the wonder's terrain is the larger departure of the two, so the
        // whole preferred ladder is exhausted first. Dropping the separation
        // altogether is worse than either and comes last, once no pool can
        // seat this wonder `MIN_WONDER_SEPARATION` hexes from its neighbours.
        let pools = [(&preferred_sites, true), (&shaped_sites, false)];
        let mut attempts: Vec<(&Vec<Pos>, bool, i32)> = Vec::new();
        for (sites, preferred_only) in pools {
            for separation in (MIN_WONDER_SEPARATION..=PREFERRED_WONDER_SEPARATION).rev() {
                attempts.push((sites, preferred_only, separation));
            }
        }
        for (sites, preferred_only) in pools {
            attempts.push((sites, preferred_only, 1));
        }
        let mut footprint_tiles = None;
        for (sites, preferred_only, separation) in attempts {
            let mut cands: Vec<Pos> = sites
                .iter()
                .copied()
                .filter(|position| far_enough(*position, separation))
                .collect();
            while !cands.is_empty() && footprint_tiles.is_none() {
                let index = rng.below(cands.len());
                let anchor = cands.swap_remove(index);
                footprint_tiles = cluster_from(anchor, preferred_only, separation);
            }
            if footprint_tiles.is_some() {
                break;
            }
        }
        if let Some(cluster) = footprint_tiles {
            for position in cluster {
                let tile = wm.tiles.get_mut(&position).unwrap();
                if matches!(*wonder, "yosemite" | "mount_everest") {
                    tile.terrain = "mountain".into();
                    tile.hills = false;
                }
                tile.feature = Some((*wonder).into());
                tile.resource = None;
                tile.improvement = None;
                placed_wonder_tiles.push(position);
            }
        }
    }

    // --- resources
    let all_pos: Vec<Pos> = wm.tiles.keys().cloned().collect();
    for pos in all_pos {
        let (terrain, feature) = {
            let t = &wm.tiles[&pos];
            (t.terrain.clone(), t.feature.clone())
        };
        let natural_wonder = feature
            .as_ref()
            .and_then(|f| rules.features.get(f))
            .map(|f| f.natural_wonder)
            .unwrap_or(false);
        if !rules.is_passable(&wm.tiles[&pos])
            || natural_wonder
            || feature.as_deref() == Some("oasis")
            || feature.as_deref() == Some("marsh")
            || feature.as_deref() == Some("volcanic_soil")
        {
            continue;
        }
        if rng.chance(0.13) {
            let hills = wm.tiles[&pos].hills;
            let valid: Vec<String> = rules
                .resources
                .iter()
                .filter(|(_, s)| {
                    // The shipped placement is a union: a listed feature on
                    // the tile, or a listed terrain on a featureless tile —
                    // and hills-only spawns (Sheep) respect the tile's form.
                    let by_feature = feature
                        .as_ref()
                        .map(|f| s.feature.contains(f))
                        .unwrap_or(false);
                    let by_terrain = feature.is_none() && s.terrain.contains(&terrain);
                    (by_feature || by_terrain) && s.hills.is_none_or(|want| want == hills)
                })
                .map(|(name, _)| name.clone())
                .collect();
            if !valid.is_empty() {
                let pick = valid[rng.below(valid.len())].clone();
                wm.tiles.get_mut(&pos).unwrap().resource = Some(pick);
            }
        }
    }

    place_strategic_quotas(rules, &mut wm, &land, num_major_spawns, rng);

    assign_continents(&mut wm, &land, width, num_continents, rng);

    // Gathering Storm marks only a subset of flat coastal land as vulnerable
    // 1 m, 2 m, or 3 m Coastal Lowland. The stock generator derives these
    // bands from its elevation field; this deterministic coordinate hash is
    // the equivalent for CIVVIS's biome generator and does not perturb the
    // seeded gameplay RNG stream.
    let coastal_candidates: Vec<Pos> = wm
        .tiles
        .iter()
        .filter(|(_, tile)| {
            !tile.hills
                && rules.is_passable(tile)
                && !rules.is_water(tile)
                && tile
                    .feature
                    .as_ref()
                    .and_then(|feature| rules.features.get(feature))
                    .is_none_or(|feature| !feature.natural_wonder)
        })
        .filter(|(position, _)| {
            hex::neighbors(**position)
                .into_iter()
                .map(|neighbor| hex::canon(neighbor, width))
                .any(|neighbor| {
                    wm.tiles
                        .get(&neighbor)
                        .is_some_and(|tile| rules.is_water(tile))
                })
        })
        .map(|(position, _)| *position)
        .collect();
    for position in coastal_candidates {
        let hash = (position.0 as i64)
            .wrapping_mul(73_856_093)
            .wrapping_add((position.1 as i64).wrapping_mul(19_349_663))
            .unsigned_abs();
        if !hash.is_multiple_of(5) {
            wm.tiles.get_mut(&position).unwrap().coastal_lowland = (hash % 3 + 1) as u8;
        }
    }

    // --- spawns. Pangaea and Inland Sea share one primary landmass; the
    // ocean-separated scripts deliberately seed majors across their viable
    // components so their geography affects play from turn one.
    let passable: BTreeSet<Pos> = land
        .iter()
        .filter(|pos| rules.is_passable(&wm.tiles[pos]))
        .cloned()
        .collect();
    let total_spawns = num_major_spawns + num_minor_spawns;
    let candidates_for = |component: &BTreeSet<Pos>, needed: usize| {
        let mut candidates: Vec<Pos> = component
            .iter()
            .filter(|position| {
                let tile = &wm.tiles[position];
                matches!(tile.terrain.as_str(), "grassland" | "plains")
                    && tile.feature.is_none()
                    && tile.improvement.is_none()
            })
            .cloned()
            .collect();
        if candidates.len() < needed {
            candidates = component
                .iter()
                .filter(|position| {
                    let tile = &wm.tiles[position];
                    tile.improvement.is_none()
                        && !tile
                            .feature
                            .as_ref()
                            .and_then(|feature| rules.features.get(feature))
                            .is_some_and(|feature| feature.natural_wonder)
                })
                .cloned()
                .collect();
        }
        candidates.sort();
        candidates
    };
    let components = connected_components(&passable, width);
    let primary = components.first().cloned().unwrap_or_default();
    let mut all_candidates = candidates_for(&passable, total_spawns);
    let mut spawns = if matches!(script, MapScript::Continents | MapScript::SmallContinents) {
        let viable: Vec<(BTreeSet<Pos>, Vec<Pos>)> = components
            .into_iter()
            .map(|component| {
                let candidates = candidates_for(&component, 1);
                (component, candidates)
            })
            .filter(|(_, candidates)| !candidates.is_empty())
            .collect();
        let mut allocations = vec![0usize; viable.len()];
        for _ in 0..num_major_spawns {
            let Some(index) = (0..viable.len())
                .filter(|index| allocations[*index] < viable[*index].1.len())
                .min_by_key(|index| {
                    // Fill every landmass once, then distribute proportionally
                    // to its available capital sites.
                    (
                        allocations[*index] > 0,
                        allocations[*index] * 1_000_000 / viable[*index].1.len().max(1),
                        *index,
                    )
                })
            else {
                break;
            };
            allocations[index] += 1;
        }
        let mut starts = Vec::new();
        for ((component, candidates), count) in viable.iter().zip(allocations) {
            starts.extend(balanced_major_spawns(
                rules, &wm, component, candidates, count, rng,
            ));
        }
        for index in (1..starts.len()).rev() {
            let other = rng.below(index + 1);
            starts.swap(index, other);
        }
        starts
    } else {
        let primary_candidates = candidates_for(&primary, total_spawns);
        all_candidates = primary_candidates.clone();
        balanced_major_spawns(
            rules,
            &wm,
            &primary,
            &primary_candidates,
            num_major_spawns,
            rng,
        )
    };
    // Defensive completion for unusually mountain-heavy seeds, followed by
    // city-state placement in the largest remaining gaps on eligible land.
    if spawns.len() < num_major_spawns {
        let missing = num_major_spawns - spawns.len();
        add_minor_spawns(rules, &wm, &all_candidates, &mut spawns, missing);
    }
    add_minor_spawns(rules, &wm, &all_candidates, &mut spawns, num_minor_spawns);
    for s in &spawns {
        let t = wm.tiles.get_mut(s).unwrap();
        t.feature = None;
        t.resource = None;
    }
    (wm, spawns)
}

/// Hexes the generator tries to keep between any two natural wonders, and the
/// floor it will not go below while a spacing-respecting site still exists.
/// `NaturalWonderGenerator` spreads the stock roster over the whole map rather
/// than letting two of them share a mountain range or a reef; the floor of 3
/// is the part that matters most, because it is what stops a pair from reading
/// as one oversized feature.
const PREFERRED_WONDER_SEPARATION: i32 = 6;
const MIN_WONDER_SEPARATION: i32 = 3;

/// World Age, which the stock scripts pass to every elevation percentile.
/// Continents.lua's "normal" is 3; a younger world raises more mountains.
const WORLD_AGE: i32 = 3;

/// Terrain band shares from `TerrainGenerator.lua` at Temperate: the driest
/// quarter of the desert field becomes Desert where the latitude allows it,
/// and the wetter half of the plains field becomes Plains.
const DESERT_PERCENT: u32 = 25;
const PLAINS_PERCENT: u32 = 50;
const SNOW_LATITUDE: f64 = 0.8;
const TUNDRA_LATITUDE: f64 = 0.65;
const GRASS_LATITUDE: f64 = 0.1;
const DESERT_BOTTOM_LATITUDE: f64 = 0.2;
const DESERT_TOP_LATITUDE: f64 = 0.5;

/// Elevation, the way `MountainsCliffs.lua` builds it: two fractal fields,
/// the mountain one with tectonic plate boundaries woven through it, cut at
/// percentiles. Mountains therefore arrive as ranges following a collision
/// line, ringed by their own foothills, rather than as short random walks;
/// hills additionally come in clumps wherever the hills field sits inside one
/// of its two bands.
fn apply_tectonics(wm: &mut WorldMap, land: &BTreeSet<Pos>, rng: &mut Rng) {
    let (width, height) = (wm.width, wm.height);
    // `MountainsCliffs.lua` weaves nine tectonic plates through the field
    // whatever the map size; the ridges they collide along are the ranges.
    const PLATES: usize = 9;
    let mut mountains = Fractal::new(rng, width, height, 3);
    mountains.build_ridges(rng, PLATES, 5.0, 5.0);
    let hills = Fractal::new(rng, width, height, 3);

    let cells: Vec<(i32, i32)> = land
        .iter()
        .map(|pos| hex::axial_to_offset(pos.0, pos.1))
        .collect();
    let mountain_threshold =
        mountains.percentile_within(cells.iter().copied(), (97 - WORLD_AGE) as u32);
    let foothills_threshold =
        mountains.percentile_within(cells.iter().copied(), (91 - 2 * WORLD_AGE) as u32);
    let pass_threshold =
        hills.percentile_within(cells.iter().copied(), (91 - 2 * WORLD_AGE) as u32);
    let low_band = (
        hills.percentile_within(cells.iter().copied(), (28 - WORLD_AGE) as u32),
        hills.percentile_within(cells.iter().copied(), (28 + WORLD_AGE) as u32),
    );
    let high_band = (
        hills.percentile_within(cells.iter().copied(), (72 - WORLD_AGE) as u32),
        hills.percentile_within(cells.iter().copied(), (72 + WORLD_AGE) as u32),
    );

    for pos in land {
        let (col, row) = hex::axial_to_offset(pos.0, pos.1);
        let mountain_value = mountains.at(col, row);
        let hill_value = hills.at(col, row);
        let tile = wm.tiles.get_mut(pos).unwrap();
        if mountain_value >= mountain_threshold {
            if hill_value >= pass_threshold {
                // A pass through the ridgeline, so a range is crossable.
                tile.hills = true;
            } else {
                tile.terrain = "mountain".into();
            }
        } else if mountain_value >= foothills_threshold {
            tile.hills = true;
        } else if (hill_value >= low_band.0 && hill_value <= low_band.1)
            || (hill_value >= high_band.0 && hill_value <= high_band.1)
        {
            tile.hills = true;
        }
    }

    // The stock generator demotes nine in ten mountains that reach the water,
    // which is what keeps coastlines workable and leaves the ranges inland.
    let coastal_peaks: Vec<Pos> = land
        .iter()
        .copied()
        .filter(|pos| wm.tiles[pos].terrain == "mountain")
        .filter(|pos| {
            hex::neighbors(*pos)
                .into_iter()
                .map(|neighbor| hex::canon(neighbor, width))
                .any(|neighbor| !land.contains(&neighbor))
        })
        .collect();
    for pos in coastal_peaks {
        if rng.below(10) < 9 {
            let tile = wm.tiles.get_mut(&pos).unwrap();
            // The climate pass, which runs next, repaints every tile that is
            // no longer a mountain, so only the elevation matters here.
            tile.terrain = "grassland".into();
            tile.hills = true;
        }
    }
}

/// Climate, the way `TerrainGenerator.lua` paints it: latitude bands whose
/// borders are roughened by a variation fractal, with Desert and Plains cut
/// out of two further fractals so that both arrive as regions. Desert is
/// additionally confined to the subtropics, which is why Civ VI worlds have
/// desert belts either side of a green equator rather than desert everywhere.
fn assign_biomes(wm: &mut WorldMap, land: &[Pos], rng: &mut Rng) {
    let (width, height) = (wm.width, wm.height);
    let deserts = Fractal::new(rng, width, height, 3);
    let plains = Fractal::new(rng, width, height, 3);
    let variation = Fractal::new(rng, width, height, 3);
    let desert_bottom = deserts.percentile(100 - DESERT_PERCENT);
    let plains_bottom = plains.percentile(100 - PLAINS_PERCENT);

    for pos in land {
        let (col, row) = hex::axial_to_offset(pos.0, pos.1);
        if wm.tiles[pos].terrain == "mountain" {
            continue;
        }
        let base = (2.0 * row as f64 / (height - 1).max(1) as f64 - 1.0).abs();
        let latitude =
            (base + (128.0 - variation.at(col, row) as f64) / (255.0 * 5.0)).clamp(0.0, 1.0);
        let terrain = if latitude >= SNOW_LATITUDE {
            "snow"
        } else if latitude >= TUNDRA_LATITUDE {
            "tundra"
        } else if latitude < GRASS_LATITUDE {
            "grassland"
        } else if deserts.at(col, row) >= desert_bottom
            && (DESERT_BOTTOM_LATITUDE..DESERT_TOP_LATITUDE).contains(&latitude)
        {
            "desert"
        } else if plains.at(col, row) >= plains_bottom {
            "plains"
        } else {
            "grassland"
        };
        wm.tiles.get_mut(pos).unwrap().terrain = terrain.into();
    }
}

/// Feature shares from the Gathering Storm `FeatureGenerator.lua` at Normal
/// rainfall: Rainforest fills 40% of the tropical band it is allowed in,
/// Woods 18% of land, Marsh 3%, Oasis 1%, and Reef 9% of eligible water.
const JUNGLE_PERCENT: usize = 40;
const FOREST_PERCENT: usize = 18;
const MARSH_PERCENT: usize = 3;
const OASIS_PERCENT: usize = 1;
const REEF_PERCENT: usize = 9;

/// The shipped clustering weight. A tile with two or three neighbours already
/// carrying the feature is the most likely to take it, and one ringed by five
/// is the least, so vegetation grows as forests and rainforests instead of
/// speckling every eligible tile independently.
fn cluster_score(adjacent: usize) -> i32 {
    match adjacent {
        0 => 300,
        1 => 350,
        2 | 3 => 450,
        4 => 250,
        _ => 100,
    }
}

fn adjacent_feature_count(wm: &WorldMap, pos: Pos, feature: &str) -> usize {
    hex::neighbors(pos)
        .into_iter()
        .map(|neighbor| hex::canon(neighbor, wm.width))
        .filter(|neighbor| {
            wm.get(*neighbor)
                .is_some_and(|tile| tile.feature.as_deref() == Some(feature))
        })
        .count()
}

/// Running-share cap: a feature stops being placed once it holds its quota of
/// the tiles considered so far, exactly as the stock generator's counters work.
fn within_share(count: usize, considered: usize, percent: usize) -> bool {
    considered == 0 || (count * 100).div_ceil(considered) <= percent
}

fn add_features(wm: &mut WorldMap, land: &BTreeSet<Pos>, rng: &mut Rng) {
    let (width, height) = (wm.width, wm.height);
    let equator = (height + 1) / 2;
    // Rainforest keeps to twenty degrees either side of the equator.
    let tropics = (20 * height / 180).max(2);

    let mut considered_land = 0;
    let mut jungle_candidates = 0;
    let (mut jungles, mut forests, mut marshes, mut oases) = (0, 0, 0, 0);

    for row in 0..height {
        for col in 0..width {
            let pos = hex::offset_to_axial(col, row);
            if !land.contains(&pos) {
                continue;
            }
            let (terrain, hills, river, has_feature) = {
                let tile = &wm.tiles[&pos];
                (
                    tile.terrain.clone(),
                    tile.hills,
                    tile.has_river(),
                    tile.feature.is_some(),
                )
            };
            if terrain == "mountain" {
                continue;
            }
            considered_land += 1;
            if has_feature {
                continue;
            }

            // Every desert tile on a river floods, as in the stock generator.
            // 🟡 The Grassland and Plains variants stand in for river size,
            // which this generator does not model.
            if river {
                let floodplain = match terrain.as_str() {
                    "desert" => Some("floodplains"),
                    "grassland" if rng.chance(0.18) => Some("grassland_floodplains"),
                    "plains" if rng.chance(0.18) => Some("plains_floodplains"),
                    _ => None,
                };
                if let Some(feature) = floodplain {
                    wm.tiles.get_mut(&pos).unwrap().feature = Some(feature.into());
                    continue;
                }
            }

            if terrain == "desert" && !hills && !river {
                if within_share(oases, considered_land, OASIS_PERCENT) && rng.below(4) == 1 {
                    wm.tiles.get_mut(&pos).unwrap().feature = Some("oasis".into());
                    oases += 1;
                }
                continue;
            }

            // Marsh, then Rainforest, then Woods — the shipped precedence.
            if terrain == "grassland"
                && !hills
                && within_share(marshes, considered_land, MARSH_PERCENT)
                && (rng.below(300) as i32)
                    <= cluster_score(adjacent_feature_count(wm, pos, "marsh"))
            {
                wm.tiles.get_mut(&pos).unwrap().feature = Some("marsh".into());
                marshes += 1;
                continue;
            }

            let tropical = (row - equator).abs() <= tropics;
            if tropical && matches!(terrain.as_str(), "grassland" | "plains") {
                jungle_candidates += 1;
                if within_share(jungles, jungle_candidates, JUNGLE_PERCENT)
                    && (rng.below(450) as i32)
                        <= cluster_score(adjacent_feature_count(wm, pos, "jungle"))
                {
                    let tile = wm.tiles.get_mut(&pos).unwrap();
                    // Rainforest leaves the ground beneath it Plains.
                    tile.terrain = "plains".into();
                    tile.feature = Some("jungle".into());
                    jungles += 1;
                    continue;
                }
            }

            if matches!(terrain.as_str(), "grassland" | "plains" | "tundra")
                && within_share(forests, considered_land, FOREST_PERCENT)
                && (rng.below(300) as i32)
                    <= cluster_score(adjacent_feature_count(wm, pos, "forest"))
            {
                wm.tiles.get_mut(&pos).unwrap().feature = Some("forest".into());
                forests += 1;
            }
        }
    }

    // Reefs favour warm water and thin out where they are already dense, so
    // they form scattered banks rather than a border around every continent.
    let mut reefable = 0;
    let mut reefs = 0;
    for row in 0..height {
        for col in 0..width {
            let pos = hex::offset_to_axial(col, row);
            let latitude = (2.0 * row as f64 / (height - 1).max(1) as f64 - 1.0).abs();
            let eligible = wm
                .get(pos)
                .is_some_and(|tile| tile.terrain == "coast" && tile.feature.is_none());
            if !eligible || latitude >= 0.78 * 0.9 {
                continue;
            }
            reefable += 1;
            if !within_share(reefs, reefable, REEF_PERCENT) {
                continue;
            }
            let crowding = match adjacent_feature_count(wm, pos, "reef") {
                0 => 100,
                1 => 125,
                2 => 150,
                3 | 4 => 175,
                _ => 10_000,
            };
            let score = 3 * (row - equator).abs() + crowding;
            if (rng.below(200) as i32) >= score {
                wm.tiles.get_mut(&pos).unwrap().feature = Some("reef".into());
                reefs += 1;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SpawnLayoutScore {
    /// No two civilizations should begin unavoidably crowded.
    minimum_separation: i32,
    /// Cover the complete landmass rather than leaving a large empty end.
    negative_coverage_radius: i32,
    /// Similar nearest-neighbor distances avoid isolated and crowded starts.
    negative_neighbor_range: i32,
    /// Voronoi area is a useful proxy for the land available to each start.
    minimum_territory: i32,
    negative_territory_range: i32,
    /// Only after spatial fairness, prefer layouts without a weak outlier.
    minimum_quality: i32,
    negative_quality_range: i32,
    total_quality: i32,
}

/// A compact estimate of the capital site rather than just its center tile.
/// Early food/production, fresh water and room to work land all matter, while
/// only the best nearby tiles count so a large empty desert is not rewarded.
fn start_quality(rules: &Rules, wm: &WorldMap, pos: Pos) -> i32 {
    let center = &wm.tiles[&pos];
    let fresh_water = center.has_river()
        || hex::neighbors(pos)
            .into_iter()
            .map(|neighbor| hex::canon(neighbor, wm.width))
            .any(|neighbor| {
                wm.get(neighbor)
                    .is_some_and(|tile| tile.feature.as_deref() == Some("oasis"))
            });
    let coastal = hex::neighbors(pos)
        .into_iter()
        .map(|neighbor| hex::canon(neighbor, wm.width))
        .any(|neighbor| wm.get(neighbor).is_some_and(|tile| rules.is_water(tile)));

    let mut nearby_yields = Vec::new();
    let mut workable_land = 0;
    let mut seen = BTreeSet::new();
    for raw in hex::disk(pos, 3) {
        let tile_pos = hex::canon(raw, wm.width);
        if !seen.insert(tile_pos) {
            continue;
        }
        let Some(tile) = wm.get(tile_pos) else {
            continue;
        };
        if !rules.is_water(tile) && rules.is_passable(tile) {
            workable_land += 1;
        }
        if tile_pos == pos || hex::wdistance(pos, tile_pos, wm.width) > 2 {
            continue;
        }
        let yields = rules.tile_yields(tile);
        nearby_yields.push(
            (yields.food * 4.0
                + yields.production * 5.0
                + yields.gold
                + yields.science * 2.0
                + yields.culture * 2.0
                + yields.faith) as i32,
        );
    }
    nearby_yields.sort_unstable_by(|a, b| b.cmp(a));
    let best_nearby: i32 = nearby_yields.into_iter().take(8).sum();
    best_nearby
        + workable_land * 2
        + if fresh_water {
            32
        } else if coastal {
            12
        } else {
            0
        }
}

fn spawn_layout_score(
    wm: &WorldMap,
    landmass: &BTreeSet<Pos>,
    layout: &[Pos],
    qualities: &BTreeMap<Pos, i32>,
) -> SpawnLayoutScore {
    if layout.is_empty() {
        return SpawnLayoutScore {
            minimum_separation: 0,
            negative_coverage_radius: 0,
            negative_neighbor_range: 0,
            minimum_territory: 0,
            negative_territory_range: 0,
            minimum_quality: 0,
            negative_quality_range: 0,
            total_quality: 0,
        };
    }

    let mut ordered = layout.to_vec();
    ordered.sort();
    let nearest: Vec<i32> = if ordered.len() == 1 {
        vec![0]
    } else {
        ordered
            .iter()
            .map(|start| {
                ordered
                    .iter()
                    .filter(|other| *other != start)
                    .map(|other| hex::wdistance(*start, *other, wm.width))
                    .min()
                    .unwrap()
            })
            .collect()
    };
    let minimum_separation = nearest.iter().copied().min().unwrap_or(0);
    let neighbor_range = nearest.iter().copied().max().unwrap_or(0) - minimum_separation;

    let mut territory = vec![0_i32; ordered.len()];
    let mut coverage_radius = 0;
    for tile in landmass {
        let (distance, owner) = ordered
            .iter()
            .enumerate()
            .map(|(index, start)| (hex::wdistance(*tile, *start, wm.width), index))
            .min()
            .unwrap();
        coverage_radius = coverage_radius.max(distance);
        territory[owner] += 1;
    }
    let territory_range =
        territory.iter().copied().max().unwrap_or(0) - territory.iter().copied().min().unwrap_or(0);
    let minimum_territory = territory.iter().copied().min().unwrap_or(0);

    let qualities: Vec<i32> = ordered.iter().map(|start| qualities[start]).collect();
    let minimum_quality = qualities.iter().copied().min().unwrap_or(0);
    let maximum_quality = qualities.iter().copied().max().unwrap_or(0);

    SpawnLayoutScore {
        minimum_separation,
        negative_coverage_radius: -coverage_radius,
        negative_neighbor_range: -neighbor_range,
        minimum_territory,
        negative_territory_range: -territory_range,
        minimum_quality,
        negative_quality_range: -(maximum_quality - minimum_quality),
        total_quality: qualities.iter().sum(),
    }
}

fn layout_balance_percentages(
    score: SpawnLayoutScore,
    civilization_count: usize,
    landmass_tiles: usize,
) -> (i32, i32, i32) {
    let territory =
        score.minimum_territory * civilization_count as i32 * 100 / landmass_tiles.max(1) as i32;
    let neighbor = if civilization_count <= 1 {
        100
    } else {
        let maximum = score.minimum_separation - score.negative_neighbor_range;
        score.minimum_separation * 100 / maximum.max(1)
    };
    let maximum_quality = score.minimum_quality - score.negative_quality_range;
    let quality = score.minimum_quality * 100 / maximum_quality.max(1);
    (territory, neighbor, quality)
}

fn farthest_layout(
    wm: &WorldMap,
    candidates: &[Pos],
    qualities: &BTreeMap<Pos, i32>,
    first: Pos,
    count: usize,
) -> Vec<Pos> {
    let mut layout = vec![first];
    while layout.len() < count {
        let Some(next) = candidates
            .iter()
            .filter(|candidate| !layout.contains(candidate))
            .max_by_key(|candidate| {
                let nearest = layout
                    .iter()
                    .map(|start| hex::wdistance(**candidate, *start, wm.width))
                    .min()
                    .unwrap_or(0);
                (nearest, qualities[*candidate], **candidate)
            })
            .copied()
        else {
            break;
        };
        layout.push(next);
    }
    layout
}

/// Try farthest-point layouts from seeds spread throughout the candidate set,
/// then retain the layout with the best spacing, coverage, territory balance
/// and site quality. This removes the large positional bias caused by making
/// a single random tile the permanent anchor for every other civilization.
fn balanced_major_spawns(
    rules: &Rules,
    wm: &WorldMap,
    landmass: &BTreeSet<Pos>,
    candidates: &[Pos],
    count: usize,
    rng: &mut Rng,
) -> Vec<Pos> {
    if count == 0 || candidates.is_empty() {
        return Vec::new();
    }
    let count = count.min(candidates.len());
    let qualities: BTreeMap<Pos, i32> = candidates
        .iter()
        .map(|candidate| (*candidate, start_quality(rules, wm, *candidate)))
        .collect();
    let mut quality_values: Vec<i32> = qualities.values().copied().collect();
    quality_values.sort_unstable();
    let quality_floor = quality_values[quality_values.len() / 4];
    let preferred_candidates: Vec<Pos> = candidates
        .iter()
        .filter(|candidate| qualities[*candidate] >= quality_floor)
        .copied()
        .collect();

    let mut layouts = Vec::with_capacity(82);
    for (pool, trial_limit) in [
        (candidates, 64_usize),
        (preferred_candidates.as_slice(), 16_usize),
    ] {
        if pool.len() < count {
            continue;
        }
        let trial_count = pool.len().min(trial_limit);
        let mut seeds = Vec::with_capacity(trial_count + 1);
        for index in 0..trial_count {
            let candidate_index = index * pool.len() / trial_count;
            if seeds.last() != pool.get(candidate_index) {
                seeds.push(pool[candidate_index]);
            }
        }
        if let Some(best_site) = pool
            .iter()
            .max_by_key(|candidate| (qualities[*candidate], **candidate))
            .copied()
        {
            if !seeds.contains(&best_site) {
                seeds.push(best_site);
            }
        }
        for seed in seeds {
            let layout = farthest_layout(wm, pool, &qualities, seed, count);
            let score = spawn_layout_score(wm, landmass, &layout, &qualities);
            layouts.push((score, layout));
        }
    }
    let best_separation = layouts
        .iter()
        .map(|(score, _)| score.minimum_separation)
        .max()
        .unwrap();
    // One hex off the theoretical maximum can buy more even neighbors,
    // territory and capital quality — but only while every seat still starts
    // comfortably apart. Below that, distance is the fairness that matters.
    let separation_floor = if best_separation > 10 {
        best_separation - 1
    } else {
        best_separation
    };
    layouts.retain(|(score, _)| score.minimum_separation >= separation_floor);
    let best_coverage = layouts
        .iter()
        .map(|(score, _)| score.negative_coverage_radius)
        .max()
        .unwrap();
    layouts.retain(|(score, _)| score.negative_coverage_radius >= best_coverage - 1);
    let mut layout = layouts
        .into_iter()
        .max_by_key(|(score, _)| {
            let (territory_balance, neighbor_balance, quality_balance) =
                layout_balance_percentages(*score, count, landmass.len());
            let worst_balance = territory_balance.min(neighbor_balance).min(quality_balance);
            (
                worst_balance,
                territory_balance + neighbor_balance + quality_balance,
                score.minimum_territory,
                score.negative_neighbor_range,
                score.minimum_quality,
                score.negative_territory_range,
                score.negative_quality_range,
                score.total_quality,
                score.minimum_separation,
                score.negative_coverage_radius,
            )
        })
        .unwrap()
        .1;

    // Farthest-point sampling fixes the coarse grid but cannot see that one
    // seat ends up with a thin territory wedge. Hill-climb each start over its
    // immediate neighbourhood, keeping any single swap that lifts the balance
    // ranking, so no seat is left an outlier the sampler simply never offered.
    let rank = |layout: &[Pos]| {
        let score = spawn_layout_score(wm, landmass, layout, &qualities);
        let (territory_balance, neighbor_balance, quality_balance) =
            layout_balance_percentages(score, count, landmass.len());
        (
            territory_balance.min(neighbor_balance).min(quality_balance),
            territory_balance + neighbor_balance + quality_balance,
            score.minimum_separation,
            score.minimum_territory,
            score.minimum_quality,
            score.total_quality,
        )
    };
    let mut best_rank = rank(&layout);
    for _ in 0..4 {
        let mut improved = false;
        for index in 0..layout.len() {
            let current = layout[index];
            let Some((candidate_rank, candidate)) = candidates
                .iter()
                .filter(|candidate| {
                    hex::wdistance(**candidate, current, wm.width) <= 3
                        && !layout.contains(candidate)
                })
                .map(|candidate| {
                    let mut trial = layout.clone();
                    trial[index] = *candidate;
                    (rank(&trial), *candidate)
                })
                // A balance win must not spend the separation the layout
                // stage just guaranteed.
                .filter(|((_, _, separation, _, _, _), _)| *separation >= separation_floor)
                .max()
            else {
                continue;
            };
            if candidate_rank > best_rank {
                best_rank = candidate_rank;
                layout[index] = candidate;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }

    // Seat order should not correlate with an anchor, edge, or the order in
    // which farthest-point sampling filled the landmass.
    for index in (1..layout.len()).rev() {
        let other = rng.below(index + 1);
        layout.swap(index, other);
    }
    layout
}

/// City-states fill the remaining largest gaps after major civilizations are
/// fixed, so they cannot pull a major start away from an otherwise fair grid.
fn add_minor_spawns(
    rules: &Rules,
    wm: &WorldMap,
    candidates: &[Pos],
    spawns: &mut Vec<Pos>,
    count: usize,
) {
    let qualities: BTreeMap<Pos, i32> = candidates
        .iter()
        .map(|candidate| (*candidate, start_quality(rules, wm, *candidate)))
        .collect();
    let target = spawns.len() + count;
    while spawns.len() < target {
        let Some(next) = candidates
            .iter()
            .filter(|candidate| !spawns.contains(candidate))
            .max_by_key(|candidate| {
                let nearest = spawns
                    .iter()
                    .map(|start| hex::wdistance(**candidate, *start, wm.width))
                    .min()
                    .unwrap_or(i32::MAX);
                (nearest, qualities[*candidate], **candidate)
            })
            .copied()
        else {
            break;
        };
        spawns.push(next);
    }
}

type RiverEdge = (Pos, Pos);

fn canonical_river_edge(a: Pos, b: Pos) -> RiverEdge {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn all_shared_edges(wm: &WorldMap) -> BTreeSet<RiverEdge> {
    let mut edges = BTreeSet::new();
    for pos in wm.tiles.keys().copied() {
        for neighbor in hex::neighbors(pos)
            .into_iter()
            .map(|p| hex::canon(p, wm.width))
            .filter(|p| wm.tiles.contains_key(p))
        {
            edges.insert(canonical_river_edge(pos, neighbor));
        }
    }
    edges
}

/// The other shared edges touching either endpoint of a hex edge. For two
/// adjacent hexes A/B, each endpoint also touches one common neighbor C; the
/// four possible continuations are A/C and B/C at those two vertices.
fn connected_river_edges(wm: &WorldMap, edge: RiverEdge) -> Vec<RiverEdge> {
    let (a, b) = edge;
    let b_neighbors: BTreeSet<Pos> = hex::neighbors(b)
        .into_iter()
        .map(|p| hex::canon(p, wm.width))
        .collect();
    let mut connected = BTreeSet::new();
    for common in hex::neighbors(a)
        .into_iter()
        .map(|p| hex::canon(p, wm.width))
        .filter(|p| *p != b && wm.tiles.contains_key(p) && b_neighbors.contains(p))
    {
        connected.insert(canonical_river_edge(a, common));
        connected.insert(canonical_river_edge(b, common));
    }
    connected.remove(&edge);
    connected.into_iter().collect()
}

fn river_edge_depth(
    edge: RiverEdge,
    is_water: &impl Fn(Pos) -> bool,
    distance_to_water: &impl Fn(Pos) -> i32,
) -> i32 {
    [edge.0, edge.1]
        .into_iter()
        .filter(|p| !is_water(*p))
        .map(distance_to_water)
        .max()
        .unwrap_or(0)
}

fn generate_rivers(wm: &mut WorldMap, land: &[Pos], rng: &mut Rng) {
    let water_tiles: Vec<Pos> = wm
        .tiles
        .iter()
        .filter(|(_, tile)| matches!(tile.terrain.as_str(), "ocean" | "coast"))
        .map(|(pos, _)| *pos)
        .collect();
    if water_tiles.is_empty() || land.is_empty() {
        return;
    }

    let width = wm.width;
    let is_water = |pos: Pos| {
        wm.tiles
            .get(&pos)
            .is_some_and(|tile| matches!(tile.terrain.as_str(), "ocean" | "coast"))
    };
    let distance_to_water = |pos: Pos| {
        water_tiles
            .iter()
            .map(|water| hex::wdistance(pos, *water, width))
            .min()
            .unwrap_or(0)
    };
    let mut outlets: Vec<RiverEdge> = all_shared_edges(wm)
        .into_iter()
        .filter(|(a, b)| is_water(*a) != is_water(*b))
        .filter(|edge| {
            connected_river_edges(wm, *edge)
                .into_iter()
                .any(|next| !is_water(next.0) && !is_water(next.1))
        })
        .collect();
    let river_count = 2.max(land.len() / 45).min(outlets.len());
    let mut rivers = BTreeSet::new();

    for _ in 0..river_count {
        let outlet = outlets.swap_remove(rng.below(outlets.len()));
        if rivers.contains(&outlet) {
            continue;
        }
        let mut current = outlet;
        let mut local = BTreeSet::new();
        let target_length = rng.randint(7, 16) as usize;
        for _ in 0..target_length {
            local.insert(current);
            rivers.insert(current);
            let current_depth = river_edge_depth(current, &is_water, &distance_to_water);
            let candidates: Vec<RiverEdge> = connected_river_edges(wm, current)
                .into_iter()
                .filter(|edge| !local.contains(edge))
                .filter(|(a, b)| !(is_water(*a) && is_water(*b)))
                .filter(|edge| {
                    river_edge_depth(*edge, &is_water, &distance_to_water) >= current_depth
                })
                .collect();
            if candidates.is_empty() {
                break;
            }
            let best_depth = candidates
                .iter()
                .map(|edge| river_edge_depth(*edge, &is_water, &distance_to_water))
                .max()
                .unwrap();
            let deepest: Vec<RiverEdge> = candidates
                .into_iter()
                .filter(|edge| river_edge_depth(*edge, &is_water, &distance_to_water) == best_depth)
                .collect();
            current = deepest[rng.below(deepest.len())];
            if rivers.contains(&current) {
                break;
            }
        }
    }

    for (a, b) in rivers {
        wm.set_river_edge(a, b, true);
    }
}

/// Top every strategic resource up to a supply the map can actually sustain.
///
/// Civ VI does not leave strategic supply to the same per-tile lottery it uses
/// for luxuries: its resource script places each strategic resource against a
/// quota derived from the land area and the number of civilizations, because
/// an empire that never finds Iron cannot train or upgrade into a single unit
/// on the Swordsman line. Rolling them against the whole 52-entry catalog put
/// **one** Iron and **one** Horses deposit on a 957-tile six-player map, which
/// left the Swordsman, Knight, Man-at-Arms and Musketman branches unbuildable
/// for everyone and armies of Warriors and Archers still in the field in the
/// Industrial era.
///
/// The eligibility test is the shipped one used by the lottery above, so this
/// changes how many deposits appear, never where they are allowed to appear.
fn place_strategic_quotas(
    rules: &Rules,
    wm: &mut WorldMap,
    land: &BTreeSet<Pos>,
    num_major_spawns: usize,
    rng: &mut Rng,
) {
    // Enough for every civilization to hold a source with some left to fight
    // over, and enough on a large map that the deposits are not all in one
    // empire's borders.
    let quota = (num_major_spawns + 1).max(land.len() / 90);
    let strategics: Vec<String> = rules
        .resources
        .iter()
        .filter(|(_, spec)| spec.class == "strategic")
        .map(|(name, _)| name.clone())
        .collect();
    let land_list: Vec<Pos> = land.iter().cloned().collect();
    for resource in strategics {
        let spec = &rules.resources[resource.as_str()];
        let placed = wm
            .tiles
            .values()
            .filter(|tile| tile.resource.as_deref() == Some(resource.as_str()))
            .count();
        let mut wanted = quota.saturating_sub(placed);
        if wanted == 0 {
            continue;
        }
        let mut candidates: Vec<Pos> = land_list
            .iter()
            .copied()
            .filter(|pos| {
                let tile = &wm.tiles[pos];
                if tile.resource.is_some() || !rules.is_passable(tile) {
                    return false;
                }
                let natural_wonder = tile
                    .feature
                    .as_deref()
                    .and_then(|feature| rules.features.get(feature))
                    .is_some_and(|feature| feature.natural_wonder);
                if natural_wonder {
                    return false;
                }
                let by_feature = tile
                    .feature
                    .as_ref()
                    .is_some_and(|feature| spec.feature.contains(feature));
                let by_terrain = tile.feature.is_none() && spec.terrain.contains(&tile.terrain);
                (by_feature || by_terrain) && spec.hills.is_none_or(|want| want == tile.hills)
            })
            .collect();
        while wanted > 0 && !candidates.is_empty() {
            let pick = rng.below(candidates.len());
            let pos = candidates.swap_remove(pick);
            wm.tiles.get_mut(&pos).unwrap().resource = Some(resource.clone());
            wanted -= 1;
        }
    }
}

/// Divide land into the stock number of named geographic regions. Civ VI's
/// continent count is not a promise of disconnected landmasses; a large
/// landmass can span several continents, so farthest-point Voronoi regions
/// are a closer model than equating one flood-fill component to one continent.
fn assign_continents(
    wm: &mut WorldMap,
    land: &BTreeSet<Pos>,
    width: i32,
    requested: usize,
    rng: &mut Rng,
) {
    if land.is_empty() || requested == 0 {
        return;
    }
    let count = requested.min(land.len());
    let land_vec: Vec<Pos> = land.iter().cloned().collect();
    let mut centers = vec![land_vec[rng.below(land_vec.len())]];
    while centers.len() < count {
        let next = *land_vec
            .iter()
            .filter(|p| !centers.contains(p))
            .max_by_key(|p| {
                let nearest = centers
                    .iter()
                    .map(|c| hex::wdistance(**p, *c, width))
                    .min()
                    .unwrap_or(0);
                (nearest, **p)
            })
            .unwrap();
        centers.push(next);
    }
    for pos in land {
        let continent = centers
            .iter()
            .enumerate()
            .min_by_key(|(id, center)| (hex::wdistance(*pos, **center, width), *id))
            .map(|(id, _)| id);
        wm.tiles.get_mut(pos).unwrap().continent = continent;
    }
}

fn connected_components(cells: &BTreeSet<Pos>, width: i32) -> Vec<BTreeSet<Pos>> {
    let mut seen: BTreeSet<Pos> = BTreeSet::new();
    let mut components = Vec::new();
    for start in cells {
        if seen.contains(start) {
            continue;
        }
        let mut comp: BTreeSet<Pos> = BTreeSet::new();
        comp.insert(*start);
        let mut stack = vec![*start];
        while let Some(cur) = stack.pop() {
            for n0 in hex::neighbors(cur) {
                let n = hex::canon(n0, width);
                if cells.contains(&n) && !comp.contains(&n) {
                    comp.insert(n);
                    stack.push(n);
                }
            }
        }
        seen.extend(comp.iter().cloned());
        components.push(comp);
    }
    components.sort_by_key(|component| std::cmp::Reverse(component.len()));
    components
}

#[cfg(test)]
fn largest_component(cells: &BTreeSet<Pos>, width: i32) -> BTreeSet<Pos> {
    connected_components(cells, width)
        .into_iter()
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod river_tests {
    use super::*;
    use crate::setup::{MapScript, CIV6_MAP_SIZES};

    fn land_components(world: &WorldMap, rules: &Rules) -> Vec<BTreeSet<Pos>> {
        let land = world
            .tiles
            .iter()
            .filter(|(_, tile)| !rules.is_water(tile))
            .map(|(position, _)| *position)
            .collect();
        connected_components(&land, world.width)
    }

    #[test]
    fn stock_map_scripts_create_distinct_playable_topologies() {
        let rules = Rules::embedded();
        for (index, script) in [
            MapScript::Pangaea,
            MapScript::Continents,
            MapScript::SmallContinents,
            MapScript::InlandSea,
        ]
        .into_iter()
        .enumerate()
        {
            let mut rng = Rng::new(72_000 + index as u64);
            let (world, spawns) =
                generate_with_script(&rules, 60, 38, 6, 6, 0, 3, script, &mut rng);
            assert_eq!(spawns.len(), 12, "{script:?} spawn count");
            for (spawn_index, start) in spawns.iter().enumerate() {
                assert!(
                    spawns[spawn_index + 1..].iter().all(|other| hex::wdistance(
                        *start,
                        *other,
                        world.width
                    ) >= 4),
                    "{script:?} starts must leave room for distinct cities"
                );
            }
            // A fractal coastline sheds islands, exactly as the stock scripts
            // do, so a topology is judged by how much land its main bodies
            // hold rather than by an exact component count.
            let components = land_components(&world, &rules);
            let total: usize = components.iter().map(|component| component.len()).sum();
            let share = |count: usize| components[..count.min(components.len())]
                .iter()
                .map(|component| component.len())
                .sum::<usize>()
                * 100
                / total.max(1);
            match script {
                MapScript::Pangaea | MapScript::InlandSea => assert!(
                    share(1) >= 80,
                    "{script:?} should be one continent with at most a few islets, \n                     largest holds {}%",
                    share(1)
                ),
                MapScript::Continents => {
                    assert!(
                        share(2) >= 80 && components[1].len() * 3 >= components[0].len(),
                        "Continents needs two comparable landmasses, got {:?}",
                        components.iter().map(|c| c.len()).collect::<Vec<_>>()
                    )
                }
                MapScript::SmallContinents => assert!(
                    components.iter().filter(|component| component.len() >= 20).count() >= 4,
                    "Small Continents needs several separated landmasses, got {:?}",
                    components.iter().map(|c| c.len()).collect::<Vec<_>>()
                ),
            }

            let occupied_components = components
                .iter()
                .filter(|component| spawns[..6].iter().any(|spawn| component.contains(spawn)))
                .count();
            let expected = match script {
                MapScript::Continents => 2,
                MapScript::SmallContinents => 4,
                _ => 1,
            };
            assert!(
                occupied_components >= expected,
                "{script:?} should distribute majors across its landmasses"
            );

            if script == MapScript::InlandSea {
                let center = hex::offset_to_axial(world.width / 2, world.height / 2);
                assert!(rules.is_water(&world.tiles[&center]));
                for col in 0..world.width {
                    for row in [0, world.height - 1] {
                        assert!(!rules.is_water(&world.tiles[&hex::offset_to_axial(col, row)]));
                    }
                }
            }
        }
    }

    /// Rolling strategic resources against the whole 52-entry catalog, one
    /// 13% chance per tile, put a single Iron and a single Horses deposit on a
    /// six-player Pangaea. With no Iron nobody can train or upgrade into a
    /// Swordsman, Legion, Man-at-Arms or Knight, so every civilization fought
    /// the whole game on the branches that cost no material - Warriors,
    /// Archers, Crossbowmen - and the Gold upgrade pass had nothing to buy.
    #[test]
    fn every_strategic_resource_reaches_a_playable_supply() {
        let rules = Rules::embedded();
        let strategics: Vec<&str> = rules
            .resources
            .iter()
            .filter(|(_, spec)| spec.class == "strategic")
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(strategics.contains(&"iron") && strategics.contains(&"horses"));
        for (index, script) in [
            MapScript::Pangaea,
            MapScript::Continents,
            MapScript::SmallContinents,
            MapScript::InlandSea,
        ]
        .into_iter()
        .enumerate()
        {
            let mut rng = Rng::new(81_000 + index as u64);
            let (world, _) = generate_with_script(&rules, 60, 38, 6, 6, 0, 3, script, &mut rng);
            for resource in &strategics {
                let count = world
                    .tiles
                    .values()
                    .filter(|tile| tile.resource.as_deref() == Some(*resource))
                    .count();
                // Six majors and six city-states: every civilization needs to
                // be able to reach a source without conquering for it.
                assert!(
                    count >= 7,
                    "{script:?} placed only {count} {resource} for six civilizations"
                );
            }
        }
    }

    #[test]
    fn generated_rivers_are_mirrored_connected_edge_chains_with_outlets() {
        let mut wm = WorldMap::new(24, 16);
        let mut land = Vec::new();
        for row in 3..13 {
            for col in 5..19 {
                let pos = hex::offset_to_axial(col, row);
                wm.tiles.get_mut(&pos).unwrap().terrain = "plains".to_string();
                land.push(pos);
            }
        }
        let mut rng = Rng::new(73);
        generate_rivers(&mut wm, &land, &mut rng);
        let river_edges: BTreeSet<RiverEdge> = all_shared_edges(&wm)
            .into_iter()
            .filter(|(a, b)| wm.has_river_edge(*a, *b))
            .collect();
        assert!(!river_edges.is_empty());
        assert!(
            river_edges.iter().any(|(a, b)| {
                wm.tiles[a].terrain == "plains" && wm.tiles[b].terrain == "plains"
            }),
            "a generated river should extend inland from its coastal outlet"
        );

        // Every serialized tile mask agrees with the neighbor's opposite edge.
        for (pos, tile) in &wm.tiles {
            for (direction, present) in tile.river_edges.iter().copied().enumerate() {
                let neighbor = hex::canon(hex::neighbors(*pos)[direction], wm.width);
                if let Some(other) = wm.get(neighbor) {
                    assert_eq!(
                        present,
                        other.river_edges[(direction + 3) % 6],
                        "river edge mismatch between {pos:?} and {neighbor:?}",
                    );
                } else {
                    assert!(!present, "river cannot leave the north/south map boundary");
                }
            }
        }

        // Each edge-connected river component reaches a land/water boundary.
        let is_water = |p: Pos| wm.tiles[&p].terrain == "ocean";
        let mut unseen = river_edges.clone();
        while let Some(start) = unseen.iter().next().copied() {
            let mut stack = vec![start];
            let mut has_outlet = false;
            unseen.remove(&start);
            while let Some(edge) = stack.pop() {
                has_outlet |= is_water(edge.0) != is_water(edge.1);
                for next in connected_river_edges(&wm, edge) {
                    if river_edges.contains(&next) && unseen.remove(&next) {
                        stack.push(next);
                    }
                }
            }
            assert!(
                has_outlet,
                "every generated river component needs a coastal outlet"
            );
        }
    }

    #[test]
    fn balanced_layout_is_independent_of_a_random_first_anchor() {
        let rules = Rules::embedded();
        let mut wm = WorldMap::new(32, 18);
        let mut landmass = BTreeSet::new();
        for row in 2..16 {
            for col in 3..29 {
                let pos = hex::offset_to_axial(col, row);
                wm.tiles.get_mut(&pos).unwrap().terrain = "plains".to_string();
                landmass.insert(pos);
            }
        }
        let candidates: Vec<Pos> = landmass.iter().copied().collect();
        let mut first_rng = Rng::new(1);
        let mut second_rng = Rng::new(999);
        let first = balanced_major_spawns(&rules, &wm, &landmass, &candidates, 6, &mut first_rng);
        let second = balanced_major_spawns(&rules, &wm, &landmass, &candidates, 6, &mut second_rng);

        assert_eq!(
            first.iter().copied().collect::<BTreeSet<_>>(),
            second.iter().copied().collect(),
            "RNG may randomize seats, but must not anchor the spatial layout"
        );
        let qualities = candidates
            .iter()
            .map(|candidate| (*candidate, start_quality(&rules, &wm, *candidate)))
            .collect();
        let score = spawn_layout_score(&wm, &landmass, &first, &qualities);
        assert!(score.minimum_separation >= 8, "{score:?}");
        assert!(score.negative_neighbor_range >= -2, "{score:?}");
    }

    #[test]
    fn stock_map_profiles_produce_spread_and_complete_spawn_sets() {
        let rules = Rules::embedded();
        for (index, size) in CIV6_MAP_SIZES.iter().enumerate() {
            let mut rng = Rng::new(10_001 + index as u64);
            let (wm, spawns) = generate(
                &rules,
                size.width,
                size.height,
                size.default_players,
                size.default_city_states,
                size.natural_wonders,
                size.continents,
                &mut rng,
            );
            assert_eq!(
                spawns.len(),
                size.default_players + size.default_city_states,
                "{} did not receive every requested spawn",
                size.name
            );

            let passable: BTreeSet<Pos> = wm
                .tiles
                .iter()
                .filter(|(_, tile)| !rules.is_water(tile) && rules.is_passable(tile))
                .map(|(pos, _)| *pos)
                .collect();
            let landmass = largest_component(&passable, wm.width);
            let majors = &spawns[..size.default_players];
            assert!(majors.iter().all(|start| landmass.contains(start)));
            assert_eq!(
                spawns.iter().copied().collect::<BTreeSet<_>>().len(),
                spawns.len(),
                "{} assigned two civilizations the same start",
                size.name
            );
            for (spawn_index, start) in spawns.iter().enumerate() {
                assert!(
                    spawns[spawn_index + 1..]
                        .iter()
                        .all(|other| hex::wdistance(*start, *other, wm.width) >= 4),
                    "{} produced starts too close to found distinct cities",
                    size.name
                );
            }
            let qualities = majors
                .iter()
                .map(|start| (*start, start_quality(&rules, &wm, *start)))
                .collect();
            let score = spawn_layout_score(&wm, &landmass, majors, &qualities);
            let balance = layout_balance_percentages(score, size.default_players, landmass.len());
            assert!(score.minimum_separation >= 6, "{}: {score:?}", size.name);
            assert!(
                balance.0 >= 50 && balance.1 >= 50 && balance.2 >= 50,
                "{} has an unfair start outlier: territory/neighbor/quality balance = {balance:?}, {score:?}",
                size.name,
            );
        }
    }

    #[test]
    fn varied_seeds_keep_major_start_outliers_within_a_roughly_equal_band() {
        let rules = Rules::embedded();
        for seed in 0..8 {
            let mut rng = Rng::new(30_000 + seed);
            let (wm, spawns) = generate(&rules, 48, 30, 4, 6, 3, 2, &mut rng);
            assert_eq!(spawns.len(), 10, "seed {seed}");
            let passable: BTreeSet<Pos> = wm
                .tiles
                .iter()
                .filter(|(_, tile)| !rules.is_water(tile) && rules.is_passable(tile))
                .map(|(pos, _)| *pos)
                .collect();
            let landmass = largest_component(&passable, wm.width);
            let majors = &spawns[..4];
            let qualities = majors
                .iter()
                .map(|start| (*start, start_quality(&rules, &wm, *start)))
                .collect();
            let score = spawn_layout_score(&wm, &landmass, majors, &qualities);
            let balance = layout_balance_percentages(score, majors.len(), landmass.len());
            assert!(
                score.minimum_separation >= 10
                    && balance.0 >= 50
                    && balance.1 >= 50
                    && balance.2 >= 50,
                "seed {seed} has an unfair start outlier: territory/neighbor/quality balance = {balance:?}, {score:?}",
            );
        }
    }

    #[test]
    fn complete_civ6_feature_roster_is_modeled_and_generated_in_valid_biomes() {
        let rules = Rules::embedded();
        let modeled = [
            "forest",
            "jungle",
            "marsh",
            "floodplains",
            "grassland_floodplains",
            "plains_floodplains",
            "oasis",
            "reef",
            "geothermal_fissure",
            "ice",
            "volcano",
            "volcanic_soil",
            "impact_zone",
            "burning_forest",
            "burnt_forest",
            "burning_jungle",
            "burnt_jungle",
        ];
        for feature in modeled {
            assert!(
                rules.features.contains_key(feature),
                "rules are missing Civ VI feature {feature}"
            );
        }

        let mut generated = BTreeSet::new();
        for seed in [7_001, 7_002, 7_003] {
            let mut rng = Rng::new(seed);
            let (world, _) = generate(&rules, 60, 38, 4, 0, 4, 3, &mut rng);
            for (position, tile) in &world.tiles {
                let Some(feature) = tile.feature.as_deref() else {
                    continue;
                };
                generated.insert(feature.to_string());
                match feature {
                    "ice" => assert!(
                        matches!(tile.terrain.as_str(), "coast" | "ocean"),
                        "sea ice generated on {} at {position:?}",
                        tile.terrain
                    ),
                    "reef" => assert_eq!(tile.terrain, "coast", "reef at {position:?}"),
                    "volcano" => {
                        assert_eq!(tile.terrain, "mountain", "volcano at {position:?}")
                    }
                    "volcanic_soil" => assert!(
                        hex::neighbors(*position)
                            .into_iter()
                            .map(|neighbor| hex::canon(neighbor, world.width))
                            .any(|neighbor| world.tiles.get(&neighbor).is_some_and(
                                |neighbor_tile| {
                                    neighbor_tile.feature.as_deref() == Some("volcano")
                                }
                            )),
                        "volcanic soil at {position:?} has no volcano"
                    ),
                    "geothermal_fissure" => assert!(
                        hex::neighbors(*position)
                            .into_iter()
                            .map(|neighbor| hex::canon(neighbor, world.width))
                            .any(|neighbor| world.tiles.get(&neighbor).is_some_and(
                                |neighbor_tile| { neighbor_tile.terrain == "mountain" }
                            )),
                        "geothermal fissure at {position:?} is not tectonic"
                    ),
                    _ => {}
                }
            }
        }
        for feature in [
            "forest",
            "jungle",
            "marsh",
            "floodplains",
            "grassland_floodplains",
            "plains_floodplains",
            "oasis",
            "reef",
            "geothermal_fissure",
            "ice",
            "volcano",
            "volcanic_soil",
        ] {
            assert!(
                generated.contains(feature),
                "ordinary generated worlds never produced {feature}: {generated:?}"
            );
        }
    }

    #[test]
    fn natural_wonders_use_their_connected_multi_tile_footprints() {
        let rules = Rules::embedded();
        let mut rng = Rng::new(88_104);
        let (world, _) = generate(&rules, 50, 32, 2, 0, 8, 3, &mut rng);
        let expected = [
            ("great_barrier_reef", 2usize),
            ("crater_lake", 1),
            ("pantanal", 4),
            ("uluru", 1),
            ("yosemite", 2),
            ("dead_sea", 2),
            ("mount_everest", 3),
            ("pamukkale", 2),
        ];
        for (wonder, footprint) in expected {
            let tiles: BTreeSet<Pos> = world
                .tiles
                .iter()
                .filter(|(_, tile)| tile.feature.as_deref() == Some(wonder))
                .map(|(position, _)| *position)
                .collect();
            assert_eq!(tiles.len(), footprint, "{wonder} footprint");
            let mut reached = BTreeSet::new();
            let mut frontier = vec![*tiles.iter().next().unwrap()];
            while let Some(position) = frontier.pop() {
                if !reached.insert(position) {
                    continue;
                }
                frontier.extend(
                    hex::neighbors(position)
                        .into_iter()
                        .map(|neighbor| hex::canon(neighbor, world.width))
                        .filter(|neighbor| tiles.contains(neighbor)),
                );
            }
            assert_eq!(reached, tiles, "{wonder} must be contiguous");
        }
    }

    /// Civ VI's natural wonder allowance is a per-map-size row, not a range,
    /// and `NaturalWonderGenerator` keeps the ones it draws apart. Both halves
    /// matter to a player: a map short of its allowance is missing content it
    /// paid a whole biome for, and two wonders sharing a mountain range read
    /// on screen as one oversized feature rather than two discoveries.
    #[test]
    fn every_map_size_draws_its_full_wonder_allowance_well_spaced() {
        let rules = Rules::embedded();
        for size in CIV6_MAP_SIZES.iter() {
            for script in [
                MapScript::Pangaea,
                MapScript::Continents,
                MapScript::SmallContinents,
                MapScript::InlandSea,
            ] {
                for seed in 0..3u64 {
                    let mut rng = Rng::new(seed);
                    let (world, _) = generate_with_script(
                        &rules,
                        size.width,
                        size.height,
                        size.default_players,
                        size.default_city_states,
                        size.natural_wonders,
                        size.continents,
                        script,
                        &mut rng,
                    );
                    let mut footprints: BTreeMap<String, Vec<Pos>> = BTreeMap::new();
                    for (position, tile) in world.tiles.iter() {
                        if let Some(feature) = &tile.feature {
                            if rules.features[feature.as_str()].natural_wonder {
                                footprints
                                    .entry(feature.clone())
                                    .or_default()
                                    .push(*position);
                            }
                        }
                    }
                    let where_ = format!("{} {script:?} seed {seed}", size.id);
                    assert_eq!(
                        footprints.len(),
                        size.natural_wonders,
                        "{where_} placed {:?}",
                        footprints.keys().collect::<Vec<_>>()
                    );
                    let names: Vec<&String> = footprints.keys().collect();
                    for (index, first) in names.iter().enumerate() {
                        for second in &names[index + 1..] {
                            let gap = footprints[*first]
                                .iter()
                                .flat_map(|left| {
                                    footprints[*second]
                                        .iter()
                                        .map(move |right| hex::wdistance(*left, *right, size.width))
                                })
                                .min()
                                .unwrap();
                            assert!(
                                gap >= MIN_WONDER_SEPARATION,
                                "{where_}: {first} and {second} are only {gap} hexes apart"
                            );
                        }
                    }
                }
            }
        }
    }
}
