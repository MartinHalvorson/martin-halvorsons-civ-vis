//! Map generation (mirrors civvis/mapgen.py).
use std::collections::BTreeSet;

use crate::rng::Rng;
use crate::rules::Rules;
use crate::world::WorldMap;
use crate::{hex, Pos};

pub fn generate(rules: &Rules, width: i32, height: i32, num_spawns: usize,
                rng: &mut Rng) -> (WorldMap, Vec<Pos>) {
    let mut wm = WorldMap::new(width, height);

    // --- landmass via random frontier growth
    let land_target = (0.42 * (width * height) as f64) as usize;
    let mut land: BTreeSet<Pos> = BTreeSet::new();
    for _ in 0..2.max(num_spawns / 2 + 1) {
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
            t.terrain == "ocean" && hex::neighbors(**pos).iter().any(|n| land.contains(n))
        })
        .map(|(pos, _)| *pos)
        .collect();
    for pos in coastal {
        wm.tiles.get_mut(&pos).unwrap().terrain = "coast".into();
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

    // --- resources
    let all_pos: Vec<Pos> = wm.tiles.keys().cloned().collect();
    for pos in all_pos {
        let (terrain, feature) = {
            let t = &wm.tiles[&pos];
            (t.terrain.clone(), t.feature.clone())
        };
        if terrain == "mountain"
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

    // --- spawns on the largest connected passable landmass
    let passable: BTreeSet<Pos> = land
        .iter()
        .filter(|p| wm.tiles[p].terrain != "mountain")
        .cloned()
        .collect();
    let largest = largest_component(&passable);
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
                let d = spawns.iter().map(|s| hex::distance(**c, *s)).min().unwrap();
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

fn largest_component(cells: &BTreeSet<Pos>) -> BTreeSet<Pos> {
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
            for n in hex::neighbors(cur) {
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
