//! Map generation (mirrors civvis/mapgen.py).
use std::collections::BTreeSet;

use crate::rng::Rng;
use crate::rules::Rules;
use crate::world::WorldMap;
use crate::{hex, Pos};

pub fn generate(
    rules: &Rules,
    width: i32,
    height: i32,
    num_spawns: usize,
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
        if t.terrain == "grassland" || t.terrain == "plains" {
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

    // --- natural wonders: use the stock per-map-size count. The engine's
    // wonders are single-tile simplifications, but remain unique and favor
    // the same broad terrain families as their Civ VI counterparts.
    let wonder_names = [
        "great_barrier_reef",
        "crater_lake",
        "pantanal",
        "uluru",
        "yosemite",
        "dead_sea",
        "mount_everest",
    ];
    for wonder in wonder_names.iter().take(num_natural_wonders) {
        let mut cands: Vec<Pos> = wm
            .tiles
            .iter()
            .filter(|(_, t)| {
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
                        matches!(t.terrain.as_str(), "desert" | "plains")
                            && !t.hills
                            && !t.has_river()
                    }
                    _ => false,
                }
            })
            .map(|(p, _)| *p)
            .collect();
        // Very unusual seeds can lack a preferred biome. Preserve the stock
        // wonder count by falling back to an otherwise empty land tile.
        if cands.is_empty() {
            cands = wm
                .tiles
                .iter()
                .filter(|(_, t)| {
                    t.terrain != "ocean"
                        && t.terrain != "coast"
                        && t.feature.is_none()
                        && t.resource.is_none()
                })
                .map(|(p, _)| *p)
                .collect();
        }
        if !cands.is_empty() {
            let p = cands[rng.below(cands.len())];
            wm.tiles.get_mut(&p).unwrap().feature = Some((*wonder).into());
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
        .filter(|p| wm.tiles[p].terrain != "mountain")
        .cloned()
        .collect();
    let largest = largest_component(&passable, width);
    let mut cands: Vec<Pos> = largest
        .iter()
        .filter(|p| {
            let t = &wm.tiles[p];
            (t.terrain == "grassland" || t.terrain == "plains") && t.feature.is_none()
        })
        .cloned()
        .collect();
    if cands.len() < num_spawns {
        cands = largest.iter().cloned().collect();
    }
    cands.sort();
    let mut spawns = vec![cands[rng.below(cands.len())]];
    while spawns.len() < num_spawns {
        let pool: Vec<Pos> = cands
            .iter()
            .filter(|c| !spawns.contains(c))
            .cloned()
            .collect();
        if pool.is_empty() {
            break;
        }
        let best = *pool
            .iter()
            .max_by_key(|c| {
                let d = spawns
                    .iter()
                    .map(|s| hex::wdistance(**c, *s, width))
                    .min()
                    .unwrap();
                (d, **c)
            })
            .unwrap();
        spawns.push(best);
    }
    for s in &spawns {
        let t = wm.tiles.get_mut(s).unwrap();
        t.feature = None;
        t.resource = None;
    }
    (wm, spawns)
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
}
