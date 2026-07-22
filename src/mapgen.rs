//! Map generation (mirrors civvis/mapgen.py).
use std::collections::BTreeSet;

use crate::rng::Rng;
use crate::rules::Rules;
use crate::world::WorldMap;
use crate::{hex, Pos};

pub fn generate(rules: &Rules, width: i32, height: i32, num_spawns: usize,
                num_natural_wonders: usize, num_continents: usize,
                rng: &mut Rng) -> (WorldMap, Vec<Pos>) {
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
            t.terrain == "ocean" && hex::neighbors(**pos).iter()
                .any(|n| land.contains(&hex::canon(*n, width)))
        })
        .map(|(pos, _)| *pos)
        .collect();
    for pos in coastal {
        wm.tiles.get_mut(&pos).unwrap().terrain = "coast".into();
    }

    // --- rivers: flow across land toward the sea (tile-based simplification)
    let water_tiles: Vec<Pos> = wm.tiles.iter()
        .filter(|(_, t)| t.terrain == "ocean" || t.terrain == "coast")
        .map(|(p, _)| *p)
        .collect();
    if !water_tiles.is_empty() {
        let n_rivers = 2.max(land_list.len() / 45);
        for _ in 0..n_rivers {
            let mut cur = land_list[rng.below(land_list.len())];
            let mut visited: BTreeSet<Pos> = BTreeSet::new();
            for _ in 0..24 {
                if visited.contains(&cur) {
                    break;
                }
                visited.insert(cur);
                {
                    let t = wm.tiles.get_mut(&cur).unwrap();
                    if t.terrain == "coast" || t.terrain == "ocean" {
                        break;
                    }
                    if t.terrain != "mountain" {
                        t.river = true;
                    }
                }
                let nbs: Vec<Pos> = hex::neighbors(cur).into_iter()
                    .map(|n| hex::canon(n, width))
                    .filter(|n| wm.tiles.contains_key(n) && !visited.contains(n))
                    .collect();
                if nbs.is_empty() {
                    break;
                }
                let dist_w = |p: Pos| water_tiles.iter()
                    .map(|w| hex::wdistance(p, *w, width)).min().unwrap_or(99);
                cur = *nbs.iter().min_by_key(|n| (dist_w(**n), **n)).unwrap();
            }
        }
    }

    // --- tribal villages (goody huts), roughly 1 per 40 land tiles
    for pos in &land_list {
        let t = &wm.tiles[pos];
        if t.terrain == "mountain" || t.river {
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
        let mut cands: Vec<Pos> = wm.tiles.iter()
            .filter(|(_, t)| {
                if t.feature.is_some() || t.resource.is_some() {
                    return false;
                }
                match *wonder {
                    "great_barrier_reef" => t.terrain == "coast",
                    "crater_lake" => matches!(t.terrain.as_str(),
                        "grassland" | "plains" | "tundra") && !t.hills && !t.river,
                    "pantanal" => matches!(t.terrain.as_str(),
                        "grassland" | "plains") && !t.hills,
                    "uluru" => t.terrain == "desert" && !t.hills,
                    "yosemite" | "mount_everest" => t.terrain == "mountain",
                    "dead_sea" => matches!(t.terrain.as_str(), "desert" | "plains")
                        && !t.hills && !t.river,
                    _ => false,
                }
            })
            .map(|(p, _)| *p)
            .collect();
        // Very unusual seeds can lack a preferred biome. Preserve the stock
        // wonder count by falling back to an otherwise empty land tile.
        if cands.is_empty() {
            cands = wm.tiles.iter()
                .filter(|(_, t)| {
                    t.terrain != "ocean" && t.terrain != "coast"
                        && t.feature.is_none() && t.resource.is_none()
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
        let natural_wonder = feature.as_ref()
            .and_then(|f| rules.features.get(f))
            .map(|f| f.natural_wonder)
            .unwrap_or(false);
        if terrain == "mountain" || natural_wonder
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
                        feature.as_ref().map(|f| s.feature.contains(f)).unwrap_or(false)
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
        let pool: Vec<Pos> = cands.iter().filter(|c| !spawns.contains(c)).cloned().collect();
        if pool.is_empty() {
            break;
        }
        let best = *pool
            .iter()
            .max_by_key(|c| {
                let d = spawns.iter().map(|s| hex::wdistance(**c, *s, width)).min().unwrap();
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

/// Divide land into the stock number of named geographic regions. Civ VI's
/// continent count is not a promise of disconnected landmasses; a large
/// landmass can span several continents, so farthest-point Voronoi regions
/// are a closer model than equating one flood-fill component to one continent.
fn assign_continents(wm: &mut WorldMap, land: &BTreeSet<Pos>, width: i32,
                     requested: usize, rng: &mut Rng) {
    if land.is_empty() || requested == 0 {
        return;
    }
    let count = requested.min(land.len());
    let land_vec: Vec<Pos> = land.iter().cloned().collect();
    let mut centers = vec![land_vec[rng.below(land_vec.len())]];
    while centers.len() < count {
        let next = *land_vec.iter()
            .filter(|p| !centers.contains(p))
            .max_by_key(|p| {
                let nearest = centers.iter()
                    .map(|c| hex::wdistance(**p, *c, width))
                    .min()
                    .unwrap_or(0);
                (nearest, **p)
            })
            .unwrap();
        centers.push(next);
    }
    for pos in land {
        let continent = centers.iter().enumerate()
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
