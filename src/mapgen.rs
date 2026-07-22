//! Map generation (mirrors civvis/mapgen.py).
use std::collections::{BTreeMap, BTreeSet};

use crate::rng::Rng;
use crate::rules::Rules;
use crate::world::WorldMap;
use crate::{hex, Pos};

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
    let mut wm = WorldMap::new(width, height);

    // --- landmass via random frontier growth
    let land_target = (0.42 * (width * height) as f64) as usize;
    let mut land: BTreeSet<Pos> = BTreeSet::new();
    for _ in 0..2.max(num_continents * 2) {
        let col = rng.randint(width / 5, width - 1 - width / 5);
        let row = rng.randint(height / 5, height - 1 - height / 5);
        land.insert(hex::offset_to_axial(col, row));
    }
    let mut frontier: Vec<Pos> = land.iter().cloned().collect();
    for _ in 0..(40 * width * height) {
        if land.len() >= land_target {
            break;
        }
        if frontier.is_empty() {
            let idx = rng.below(land.len());
            frontier.push(*land.iter().nth(idx).unwrap());
        }
        let ci = rng.below(frontier.len());
        let cur = frontier[ci];
        let nbs: Vec<Pos> = hex::neighbors(cur)
            .into_iter()
            .map(|n| hex::canon(n, width))
            .filter(|n| wm.tiles.contains_key(n) && !land.contains(n))
            .collect();
        if nbs.is_empty() {
            frontier.remove(ci);
            continue;
        }
        let nxt = nbs[rng.below(nbs.len())];
        land.insert(nxt);
        frontier.push(nxt);
        if rng.chance(0.25) {
            if let Some(i) = frontier.iter().position(|p| *p == cur) {
                frontier.remove(i);
            }
        }
    }

    let land_list: Vec<Pos> = land.iter().cloned().collect();
    let latitude = |pos: Pos| -> f64 {
        let (_, row) = hex::axial_to_offset(pos.0, pos.1);
        (2.0 * row as f64 / (height - 1).max(1) as f64 - 1.0).abs()
    };

    // --- climate bands
    for pos in &land_list {
        let v = latitude(*pos) + rng.uniform(-0.15, 0.15);
        let t = wm.tiles.get_mut(pos).unwrap();
        t.terrain = if v > 0.85 {
            "snow".into()
        } else if v > 0.62 {
            "tundra".into()
        } else if v < 0.30 {
            ["desert", "plains", "grassland"][rng.weighted(&[0.25, 0.40, 0.35])].into()
        } else {
            ["grassland", "plains", "desert"][rng.weighted(&[0.50, 0.42, 0.08])].into()
        };
    }

    // --- mountain chains, hills
    for _ in 0..2.max(land_list.len() / 40) {
        let mut cur = land_list[rng.below(land_list.len())];
        let steps = rng.randint(2, 5);
        for _ in 0..steps {
            wm.tiles.get_mut(&cur).unwrap().terrain = "mountain".into();
            let nbs: Vec<Pos> = hex::neighbors(cur)
                .into_iter()
                .map(|n| hex::canon(n, width))
                .filter(|n| land.contains(n))
                .collect();
            if nbs.is_empty() {
                break;
            }
            cur = nbs[rng.below(nbs.len())];
        }
    }
    for pos in &land_list {
        let roll = rng.chance(0.16);
        let t = wm.tiles.get_mut(pos).unwrap();
        if t.terrain != "mountain" && roll {
            t.hills = true;
        }
    }

    // --- coast
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

    // --- features
    for pos in &land_list {
        let lat = latitude(*pos);
        let r = rng.f64();
        let t = wm.tiles.get_mut(pos).unwrap();
        if t.terrain == "mountain" {
            continue;
        }
        if t.has_river() && t.terrain == "desert" && r < 0.55 {
            t.feature = Some("floodplains".into());
        } else if t.has_river() && t.terrain == "grassland" && r < 0.18 {
            t.feature = Some("grassland_floodplains".into());
        } else if t.has_river() && t.terrain == "plains" && r < 0.18 {
            t.feature = Some("plains_floodplains".into());
        } else if r > 0.992 {
            t.feature = Some("geothermal_fissure".into());
        } else if t.terrain == "grassland" || t.terrain == "plains" {
            if lat < 0.25 && r < 0.28 {
                t.feature = Some("jungle".into());
            } else if r < 0.20 {
                t.feature = Some("forest".into());
            } else if t.terrain == "grassland" && r > 0.97 {
                t.feature = Some("marsh".into());
            }
        } else if t.terrain == "tundra" && r < 0.22 {
            t.feature = Some("forest".into());
        } else if t.terrain == "desert" && r < 0.05 {
            t.feature = Some("oasis".into());
        }
    }

    // Reefs are ordinary coastal features and supply the Campus's major
    // Gathering Storm adjacency source.
    for tile in wm.tiles.values_mut() {
        if tile.terrain == "coast" && tile.feature.is_none() && rng.chance(0.08) {
            tile.feature = Some("reef".into());
        }
    }

    // --- natural wonders: use the stock per-map-size count and the actual
    // footprint of each modeled wonder. Multi-tile wonders are grown as a
    // connected cluster so discovery, adjacency and yields operate on every
    // constituent tile rather than on a single representative hex.
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
        let mut cands: Vec<Pos> = wm
            .tiles
            .iter()
            .filter(|(_, t)| preferred(t))
            .map(|(p, _)| *p)
            .collect();
        let cluster_from = |anchor: Pos, preferred_only: bool| {
            let mut cluster = vec![anchor];
            while cluster.len() < footprint {
                let mut frontier: Vec<Pos> = cluster
                    .iter()
                    .flat_map(|position| hex::neighbors(*position))
                    .map(|position| hex::canon(position, width))
                    .filter(|position| wm.tiles.contains_key(position))
                    .filter(|position| !cluster.contains(position))
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
        let mut footprint_tiles = None;
        while !cands.is_empty() && footprint_tiles.is_none() {
            let index = rng.below(cands.len());
            let anchor = cands.swap_remove(index);
            footprint_tiles = cluster_from(anchor, true);
        }
        // Very unusual seeds can lack a large enough preferred biome. Keep
        // the correct footprint and map-size count by shaping an otherwise
        // empty connected region into the wonder's terrain family.
        if footprint_tiles.is_none() {
            cands = wm
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
            while !cands.is_empty() && footprint_tiles.is_none() {
                let index = rng.below(cands.len());
                let anchor = cands.swap_remove(index);
                footprint_tiles = cluster_from(anchor, false);
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
        if terrain == "mountain"
            || natural_wonder
            || feature.as_deref() == Some("oasis")
            || feature.as_deref() == Some("marsh")
        {
            continue;
        }
        if rng.chance(0.13) {
            let valid: Vec<String> = rules
                .resources
                .iter()
                .filter(|(_, s)| {
                    if !s.feature.is_empty() {
                        feature
                            .as_ref()
                            .map(|f| s.feature.contains(f))
                            .unwrap_or(false)
                    } else {
                        s.terrain.contains(&terrain)
                    }
                })
                .map(|(name, _)| name.clone())
                .collect();
            if !valid.is_empty() {
                let pick = valid[rng.below(valid.len())].clone();
                wm.tiles.get_mut(&pos).unwrap().resource = Some(pick);
            }
        }
    }

    assign_continents(&mut wm, &land, width, num_continents, rng);

    // --- spawns on the largest connected passable landmass
    let passable: BTreeSet<Pos> = land
        .iter()
        .filter(|pos| rules.is_passable(&wm.tiles[pos]))
        .cloned()
        .collect();
    let largest = largest_component(&passable, width);
    let mut cands: Vec<Pos> = largest
        .iter()
        .filter(|p| {
            let t = &wm.tiles[p];
            (t.terrain == "grassland" || t.terrain == "plains")
                && t.feature.is_none()
                && t.improvement.is_none()
        })
        .cloned()
        .collect();
    let total_spawns = num_major_spawns + num_minor_spawns;
    if cands.len() < total_spawns {
        cands = largest
            .iter()
            .filter(|pos| {
                let tile = &wm.tiles[pos];
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
    cands.sort();
    let mut spawns = balanced_major_spawns(rules, &wm, &largest, &cands, num_major_spawns, rng);
    add_minor_spawns(rules, &wm, &cands, &mut spawns, num_minor_spawns);
    for s in &spawns {
        let t = wm.tiles.get_mut(s).unwrap();
        t.feature = None;
        t.resource = None;
    }
    (wm, spawns)
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
    // One hex off the theoretical maximum is a small price for substantially
    // more even neighbors, territory and capital quality.
    let separation_floor = best_separation.saturating_sub(1);
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

fn largest_component(cells: &BTreeSet<Pos>, width: i32) -> BTreeSet<Pos> {
    let mut seen: BTreeSet<Pos> = BTreeSet::new();
    let mut best: BTreeSet<Pos> = BTreeSet::new();
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
        if comp.len() > best.len() {
            best = comp;
        }
    }
    best
}

#[cfg(test)]
mod river_tests {
    use super::*;
    use crate::setup::CIV6_MAP_SIZES;

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
}
