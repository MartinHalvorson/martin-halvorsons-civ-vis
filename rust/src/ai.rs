//! Scripted AIs (mirrors civvis/ai/). BasicAi reads full state (no fog) —
//! sparring partner, not a fair-play agent.
use crate::game::{effective_strength, Action, Game, Item};
use crate::rng::Rng;
use crate::{hex, Pos};

const TECH_PRIORITY: [&str; 15] = ["pottery", "animal_husbandry", "mining", "writing",
    "archery", "bronze_working", "currency", "masonry", "irrigation", "iron_working",
    "mathematics", "construction", "engineering", "education", "machinery"];
const CIVIC_PRIORITY: [&str; 8] = ["code_of_laws", "craftsmanship", "foreign_trade",
    "early_empire", "state_workforce", "military_tradition", "drama_poetry",
    "political_philosophy"];
const DISTRICT_PRIORITY: [&str; 4] = ["campus", "commercial_hub", "holy_site",
    "theater_square"];

pub trait Ai {
    fn take_turn(&mut self, g: &mut Game, pid: usize);
}

pub fn run_game<A: Ai>(g: &mut Game, ais: &mut [A]) {
    while g.winner.is_none() {
        let pid = g.current;
        ais[pid].take_turn(g, pid);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

// ----------------------------------------------------------------- RandomAi

pub struct RandomAi {
    rng: Rng,
}

impl RandomAi {
    pub fn new(seed: u64) -> RandomAi {
        RandomAi { rng: Rng::new(seed) }
    }
}

impl Ai for RandomAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        for _ in 0..60 {
            let acts: Vec<Action> = g
                .legal_actions(pid)
                .into_iter()
                .filter(|a| !matches!(a, Action::EndTurn))
                .collect();
            if acts.is_empty() {
                break;
            }
            let a = acts[self.rng.below(acts.len())].clone();
            let _ = g.apply(pid, &a);
            if g.winner.is_some() {
                break;
            }
        }
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

// ------------------------------------------------------------------ BasicAi

#[derive(Default)]
pub struct BasicAi {
    minor: bool,
}

impl BasicAi {
    pub fn new() -> BasicAi {
        BasicAi { minor: false }
    }

    pub fn fleet(g: &Game) -> Vec<BasicAi> {
        g.players.iter().map(|_| BasicAi::new()).collect()
    }
}

impl Ai for BasicAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        self.minor = g.players[pid].is_minor;
        self.research(g, pid);
        self.diplomacy(g, pid);
        self.cities(g, pid);
        self.units(g, pid);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

impl BasicAi {
    fn research(&self, g: &mut Game, pid: usize) {
        if g.players[pid].research.is_none() {
            let avail = g.available_techs(pid);
            if !avail.is_empty() {
                let pick = TECH_PRIORITY
                    .iter()
                    .find(|t| avail.iter().any(|a| a == *t))
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| avail[0].clone());
                let _ = g.apply(pid, &Action::Research { tech: pick });
            }
        }
        if g.players[pid].civic.is_none() {
            let avail = g.available_civics(pid);
            if !avail.is_empty() {
                let pick = CIVIC_PRIORITY
                    .iter()
                    .find(|c| avail.iter().any(|a| a == *c))
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| avail[0].clone());
                let _ = g.apply(pid, &Action::Civic { civic: pick });
            }
        }
    }

    fn diplomacy(&self, g: &mut Game, pid: usize) {
        let my_power = g.military_power(pid);
        let others: Vec<usize> = g
            .players
            .iter()
            .filter(|o| o.id != pid && o.alive)
            .map(|o| o.id)
            .collect();
        for o in &others {
            if g.is_at_war(pid, *o) && my_power < 0.6 * g.military_power(*o) {
                let _ = g.apply(pid, &Action::MakePeace { player: *o });
            }
        }
        if self.minor {
            return;
        }
        let at_war = others.iter().any(|o| g.is_at_war(pid, *o));
        if !at_war && g.turn > 40 && g.player_city_ids(pid).len() >= 2 && !others.is_empty() {
            let weakest = *others
                .iter()
                .min_by(|a, b| {
                    g.military_power(**a).partial_cmp(&g.military_power(**b)).unwrap()
                })
                .unwrap();
            if my_power > 1.8 * g.military_power(weakest) + 20.0 {
                let _ = g.apply(pid, &Action::DeclareWar { player: weakest });
            }
        }
    }

    fn cities(&self, g: &mut Game, pid: usize) {
        let mut settlers = 0;
        let mut builders = 0;
        let mut military = 0;
        for uid in g.player_unit_ids(pid) {
            let kind = g.units[&uid].kind.clone();
            match kind.as_str() {
                "settler" => settlers += 1,
                "builder" => builders += 1,
                _ => {
                    if g.rules.units[kind.as_str()].class == "military" {
                        military += 1;
                    }
                }
            }
        }
        let city_ids = g.player_city_ids(pid);
        let n_cities = city_ids.len();
        for cid in &city_ids {
            if !g.cities[cid].queue.is_empty() {
                continue;
            }
            if let Some(item) =
                self.pick_item(g, pid, *cid, n_cities, settlers, builders, military)
            {
                if g.apply(pid, &Action::Produce { city: *cid, item: item.clone() }).is_ok() {
                    match &item {
                        Item::Unit { unit } if unit == "settler" => settlers += 1,
                        Item::Unit { unit } if unit == "builder" => builders += 1,
                        Item::Unit { .. } => military += 1,
                        _ => {}
                    }
                }
            }
        }
        if g.players[pid].faith >= 120.0 && builders < n_cities && !city_ids.is_empty() {
            let _ = g.apply(pid, &Action::Buy {
                city: city_ids[0],
                unit: "builder".to_string(),
                currency: "faith".to_string(),
            });
        }
    }

    fn best_military(&self, g: &Game, pid: usize, cid: u32) -> Option<String> {
        let mut best: Option<(f64, String)> = None;
        for (name, spec) in &g.rules.units {
            if spec.class != "military" || spec.domain.as_deref() == Some("sea") {
                continue;
            }
            if !g.can_produce(pid, cid, &Item::Unit { unit: name.clone() }) {
                continue;
            }
            let power = spec.strength.max(spec.ranged_strength);
            if best.as_ref().map(|(b, _)| power > *b).unwrap_or(true) {
                best = Some((power, name.clone()));
            }
        }
        best.map(|(_, n)| n)
    }

    #[allow(clippy::too_many_arguments)]
    fn pick_item(&self, g: &Game, pid: usize, cid: u32, n_cities: usize,
                 settlers: usize, builders: usize, military: usize) -> Option<Item> {
        let city_pop = g.cities[&cid].pop;
        if military < n_cities {
            if let Some(m) = self.best_military(g, pid, cid) {
                return Some(Item::Unit { unit: m });
            }
        }
        if !self.minor && n_cities + settlers < 4 && settlers == 0 && city_pop >= 2
            && g.turn < 150
        {
            return Some(Item::Unit { unit: "settler".to_string() });
        }
        if builders < (n_cities + 1) / 2 {
            return Some(Item::Unit { unit: "builder".to_string() });
        }
        if !g.cities[&cid].buildings.iter().any(|b| b == "monument") {
            return Some(Item::Building { building: "monument".to_string() });
        }
        for dname in DISTRICT_PRIORITY {
            if g.cities[&cid].districts.contains_key(dname) {
                continue;
            }
            let spec = &g.rules.districts[dname];
            let unlocked = spec.tech.as_ref().map(|t| g.players[pid].techs.contains(t)).unwrap_or(true)
                && spec.civic.as_ref().map(|c| g.players[pid].civics.contains(c)).unwrap_or(true);
            if !unlocked {
                continue;
            }
            let sites = g.district_sites(cid, dname);
            if !sites.is_empty() {
                let best = *sites
                    .iter()
                    .max_by(|a, b| {
                        let ya = g.district_yields(dname, **a).total();
                        let yb = g.district_yields(dname, **b).total();
                        ya.partial_cmp(&yb).unwrap().then(a.cmp(b))
                    })
                    .unwrap();
                return Some(Item::District { district: dname.to_string(), pos: best });
            }
        }
        let mut buildable: Vec<(i64, String)> = g
            .rules
            .buildings
            .iter()
            .filter(|(b, _)| {
                g.can_produce(pid, cid, &Item::Building { building: (*b).clone() })
            })
            .map(|(b, s)| (s.cost as i64, b.clone()))
            .collect();
        if !buildable.is_empty() {
            buildable.sort();
            return Some(Item::Building { building: buildable[0].1.clone() });
        }
        self.best_military(g, pid, cid).map(|m| Item::Unit { unit: m })
    }

    fn units(&self, g: &mut Game, pid: usize) {
        for uid in g.player_unit_ids(pid) {
            for _ in 0..8 {
                if !g.units.contains_key(&uid) {
                    break;
                }
                if g.units[&uid].moves_left <= 0.0 {
                    break;
                }
                let kind = g.units[&uid].kind.clone();
                let acted = match kind.as_str() {
                    "settler" => self.settler_step(g, pid, uid),
                    "builder" => self.builder_step(g, pid, uid),
                    _ => self.military_step(g, pid, uid),
                };
                if !acted {
                    break;
                }
            }
        }
    }

    fn step_toward(&self, g: &mut Game, pid: usize, uid: u32, target: Pos) -> bool {
        let cur = g.units[&uid].pos;
        let mut opts: Vec<Pos> = hex::neighbors(cur)
            .into_iter()
            .filter(|n| g.can_move(uid, *n))
            .collect();
        if opts.is_empty() {
            return false;
        }
        opts.sort_by_key(|n| (hex::distance(*n, target), *n));
        let best = opts[0];
        if hex::distance(best, target) >= hex::distance(cur, target) {
            return false;
        }
        g.apply(pid, &Action::Move { unit: uid, to: best }).is_ok()
    }

    fn settle_value(&self, g: &Game, pos: Pos) -> f64 {
        let mut total = 0.0;
        for p in hex::disk(pos, 1) {
            if let Some(t) = g.map.get(p) {
                if t.owner_city.is_some() {
                    continue;
                }
                let ys = g.rules.tile_yields(t);
                total += ys.food * 1.2 + ys.production + ys.gold * 0.3;
            }
        }
        total
    }

    fn settler_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        let mut best: Option<(f64, Pos)> = None;
        for pos in hex::disk(upos, 5) {
            let t = match g.map.get(pos) {
                Some(t) => t,
                None => continue,
            };
            if g.rules.is_water(t) || !g.rules.is_passable(t) {
                continue;
            }
            if g.cities.values().any(|c| hex::distance(c.pos, pos) < 4) {
                continue;
            }
            if let Some(oc) = t.owner_city {
                if g.cities[&oc].owner != pid {
                    continue;
                }
            }
            let val = self.settle_value(g, pos) - 0.4 * hex::distance(upos, pos) as f64;
            let better = match &best {
                None => true,
                Some((bv, bp)) => val > *bv || (val == *bv && pos > *bp),
            };
            if better {
                best = Some((val, pos));
            }
        }
        let target = match best {
            Some((_, p)) => p,
            None => return false,
        };
        if target == upos {
            return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
        }
        self.step_toward(g, pid, uid, target)
    }

    fn builder_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        let imps = g.valid_improvements(pid, upos);
        if !imps.is_empty() {
            return g
                .apply(pid, &Action::Improve { unit: uid, improvement: imps[0].clone() })
                .is_ok();
        }
        let mut best: Option<(i32, Pos)> = None;
        for cid in g.player_city_ids(pid) {
            for pos in g.cities[&cid].owned_tiles.clone() {
                if !g.valid_improvements(pid, pos).is_empty() {
                    let d = hex::distance(upos, pos);
                    if best.map(|b| (d, pos) < b).unwrap_or(true) {
                        best = Some((d, pos));
                    }
                }
            }
        }
        match best {
            Some((_, pos)) => self.step_toward(g, pid, uid, pos),
            None => false,
        }
    }

    fn is_enemy_tile(&self, g: &Game, pos: Pos, enemy_ids: &[usize]) -> bool {
        for oid in g.units_at(pos) {
            if enemy_ids.contains(&g.units[&oid].owner) {
                return true;
            }
        }
        if let Some(cid) = g.city_at(pos) {
            return enemy_ids.contains(&g.cities[&cid].owner);
        }
        false
    }

    fn worth_attacking(&self, g: &Game, uid: u32, pos: Pos) -> bool {
        if let Some(cid) = g.city_at(pos) {
            if g.cities[&cid].owner != g.units[&uid].owner {
                return true;
            }
        }
        let u = &g.units[&uid];
        let mine = effective_strength(g.rules.units[u.kind.as_str()].strength.max(1.0), u.hp);
        for oid in g.units_at(pos) {
            let o = &g.units[&oid];
            let ospec = &g.rules.units[o.kind.as_str()];
            if ospec.class == "military" {
                let theirs = effective_strength(ospec.strength.max(1.0), o.hp);
                return mine >= theirs - 8.0;
            }
        }
        true
    }

    fn nearest_enemy(&self, g: &Game, pos: Pos, enemy_ids: &[usize]) -> Option<Pos> {
        let mut best: Option<(i32, Pos)> = None;
        for c in g.cities.values() {
            if enemy_ids.contains(&c.owner) {
                let d = hex::distance(pos, c.pos);
                if best.map(|b| (d, c.pos) < b).unwrap_or(true) {
                    best = Some((d, c.pos));
                }
            }
        }
        for u in g.units.values() {
            if enemy_ids.contains(&u.owner) {
                let d = hex::distance(pos, u.pos);
                if best.map(|b| (d, u.pos) < b).unwrap_or(true) {
                    best = Some((d, u.pos));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    fn nearest_unexplored(&self, g: &Game, pid: usize, pos: Pos) -> Option<Pos> {
        let mut best: Option<(i32, Pos)> = None;
        for tpos in g.map.tiles.keys() {
            if g.players[pid].explored.contains(tpos) {
                continue;
            }
            let d = hex::distance(pos, *tpos);
            if best.map(|b| (d, *tpos) < b).unwrap_or(true) {
                best = Some((d, *tpos));
            }
        }
        best.map(|(_, p)| p)
    }

    fn military_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        let spec = g.rules.units[g.units[&uid].kind.as_str()].clone();
        let enemy_ids: Vec<usize> = g
            .players
            .iter()
            .filter(|o| o.id != pid && o.alive && g.is_at_war(pid, o.id))
            .map(|o| o.id)
            .collect();
        if !enemy_ids.is_empty() {
            if spec.ranged_strength > 0.0 {
                for pos in hex::disk(upos, spec.range.max(1)) {
                    if pos == upos || g.map.get(pos).is_none() {
                        continue;
                    }
                    if self.is_enemy_tile(g, pos, &enemy_ids) {
                        return g.apply(pid, &Action::Ranged { unit: uid, target: pos }).is_ok();
                    }
                }
            } else {
                for pos in hex::neighbors(upos) {
                    if g.map.get(pos).is_none() {
                        continue;
                    }
                    if self.is_enemy_tile(g, pos, &enemy_ids)
                        && self.worth_attacking(g, uid, pos)
                    {
                        return g.apply(pid, &Action::Attack { unit: uid, target: pos }).is_ok();
                    }
                }
            }
            return match self.nearest_enemy(g, upos, &enemy_ids) {
                Some(t) => self.step_toward(g, pid, uid, t),
                None => false,
            };
        }
        // peace: minors guard home; majors explore, then garrison
        if self.minor {
            let cities = g.player_city_ids(pid);
            if cities.is_empty() {
                return false;
            }
            let cap = g.cities[&cities[0]].pos;
            if hex::distance(upos, cap) > 2 {
                return self.step_toward(g, pid, uid, cap);
            }
            return false;
        }
        let target = match self.nearest_unexplored(g, pid, upos) {
            Some(t) => Some(t),
            None => {
                let cities = g.player_city_ids(pid);
                if cities.is_empty() {
                    None
                } else {
                    let cap = cities
                        .iter()
                        .min_by_key(|c| (hex::distance(upos, g.cities[c].pos), **c))
                        .unwrap();
                    let cpos = g.cities[cap].pos;
                    if cpos == upos {
                        None
                    } else {
                        Some(cpos)
                    }
                }
            }
        };
        match target {
            Some(t) => self.step_toward(g, pid, uid, t),
            None => false,
        }
    }
}
