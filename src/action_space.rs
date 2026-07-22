//! Fixed action encoding for learned policies.
//!
//! Civ's action space is enormous and variable, so a fixed one-hot head is
//! the wrong shape. Instead every legal action is featurized into a
//! fixed-width vector; a policy scores the candidate set and picks one
//! (pointer-style). That keeps the network's output size constant while the
//! number of legal actions varies from turn to turn.
//!
//! `legal_encoded(g, pid)` returns the legal actions alongside their kind
//! indices and feature rows. The kind mask says which of the [`KINDS`]
//! categories are available at all, which is what a hierarchical policy
//! (choose kind, then choose among that kind's actions) needs.
use crate::game::{Action, Game};
use crate::Pos;

/// Every `Action` discriminant, in a stable order. Appending is safe;
/// reordering invalidates trained policies.
pub const KINDS: [&str; 70] = [
    "move", "move_to", "attack", "ranged", "found_city", "improve",
    "found_corporation", "move_product", "contribute_project",
    "contribute_district", "perform_concert", "pillage", "repair_improvement",
    "coastal_raid", "air_rebase", "air_strike", "air_patrol", "produce", "buy",
    "buy_building", "buy_district", "research", "civic", "declare_war",
    "declare_war_with_casus_belli", "make_peace", "denounce", "propose_deal",
    "accept_deal", "reject_deal", "trade", "congress_vote", "assign_spy",
    "spy_mission", "promote_spy", "choose_dedication", "fortify", "promote",
    "combine_units", "link_units", "unlink_units", "government", "slot_policy",
    "unslot_policy", "trade_route", "send_envoy", "levy_military",
    "recruit_great_person", "patronize_great_person", "choose_pantheon",
    "choose_secret_society", "assign_governor", "appoint_governor",
    "reassign_governor", "promote_governor", "found_religion", "spread",
    "theological_attack", "condemn_heretic", "heal_religious", "remove_heresy",
    "launch_inquisition", "evangelize_belief", "convert_barbarians",
    "city_strike", "encampment_strike", "keep_city", "raze_city",
    "liberate_city", "end_turn",
];

/// Width of one action's feature row: kind one-hot plus the shared
/// numeric block described in [`features`].
pub const FEATURE_WIDTH: usize = KINDS.len() + 12;

pub fn kind_index(action: &Action) -> usize {
    let name = kind_name(action);
    KINDS
        .iter()
        .position(|k| *k == name)
        .expect("every Action variant is listed in KINDS")
}

pub fn kind_name(action: &Action) -> &'static str {
    match action {
        Action::Move { .. } => "move",
        Action::MoveTo { .. } => "move_to",
        Action::Attack { .. } => "attack",
        Action::Ranged { .. } => "ranged",
        Action::FoundCity { .. } => "found_city",
        Action::Improve { .. } => "improve",
        Action::FoundCorporation { .. } => "found_corporation",
        Action::MoveProduct { .. } => "move_product",
        Action::ContributeProject { .. } => "contribute_project",
        Action::ContributeDistrict { .. } => "contribute_district",
        Action::PerformConcert { .. } => "perform_concert",
        Action::Pillage { .. } => "pillage",
        Action::RepairImprovement { .. } => "repair_improvement",
        Action::CoastalRaid { .. } => "coastal_raid",
        Action::AirRebase { .. } => "air_rebase",
        Action::AirStrike { .. } => "air_strike",
        Action::AirPatrol { .. } => "air_patrol",
        Action::Produce { .. } => "produce",
        Action::Buy { .. } => "buy",
        Action::BuyBuilding { .. } => "buy_building",
        Action::BuyDistrict { .. } => "buy_district",
        Action::Research { .. } => "research",
        Action::Civic { .. } => "civic",
        Action::DeclareWar { .. } => "declare_war",
        Action::DeclareWarWithCasusBelli { .. } => "declare_war_with_casus_belli",
        Action::MakePeace { .. } => "make_peace",
        Action::Denounce { .. } => "denounce",
        Action::ProposeDeal { .. } => "propose_deal",
        Action::AcceptDeal { .. } => "accept_deal",
        Action::RejectDeal { .. } => "reject_deal",
        Action::Trade { .. } => "trade",
        Action::CongressVote { .. } => "congress_vote",
        Action::AssignSpy { .. } => "assign_spy",
        Action::SpyMission { .. } => "spy_mission",
        Action::PromoteSpy { .. } => "promote_spy",
        Action::ChooseDedication { .. } => "choose_dedication",
        Action::Fortify { .. } => "fortify",
        Action::Promote { .. } => "promote",
        Action::CombineUnits { .. } => "combine_units",
        Action::LinkUnits { .. } => "link_units",
        Action::UnlinkUnits { .. } => "unlink_units",
        Action::Government { .. } => "government",
        Action::SlotPolicy { .. } => "slot_policy",
        Action::UnslotPolicy { .. } => "unslot_policy",
        Action::TradeRoute { .. } => "trade_route",
        Action::SendEnvoy { .. } => "send_envoy",
        Action::LevyMilitary { .. } => "levy_military",
        Action::RecruitGreatPerson { .. } => "recruit_great_person",
        Action::PatronizeGreatPerson { .. } => "patronize_great_person",
        Action::ChoosePantheon { .. } => "choose_pantheon",
        Action::ChooseSecretSociety { .. } => "choose_secret_society",
        Action::AssignGovernor { .. } => "assign_governor",
        Action::AppointGovernor { .. } => "appoint_governor",
        Action::ReassignGovernor { .. } => "reassign_governor",
        Action::PromoteGovernor { .. } => "promote_governor",
        Action::FoundReligion { .. } => "found_religion",
        Action::Spread { .. } => "spread",
        Action::TheologicalAttack { .. } => "theological_attack",
        Action::CondemnHeretic { .. } => "condemn_heretic",
        Action::HealReligious { .. } => "heal_religious",
        Action::RemoveHeresy { .. } => "remove_heresy",
        Action::LaunchInquisition { .. } => "launch_inquisition",
        Action::EvangelizeBelief { .. } => "evangelize_belief",
        Action::ConvertBarbarians { .. } => "convert_barbarians",
        Action::CityStrike { .. } => "city_strike",
        Action::EncampmentStrike { .. } => "encampment_strike",
        Action::KeepCity { .. } => "keep_city",
        Action::RazeCity { .. } => "raze_city",
        Action::LiberateCity { .. } => "liberate_city",
        Action::EndTurn => "end_turn",
    }
}

/// The tile an action points at, when it has one. Policies use this to look
/// up the corresponding cell of the spatial observation.
pub fn target_tile(g: &Game, action: &Action) -> Option<Pos> {
    match action {
        Action::Move { to, .. } | Action::MoveTo { to, .. } => Some(*to),
        Action::AirRebase { to, .. } | Action::AirPatrol { to, .. } => Some(*to),
        Action::Attack { target, .. }
        | Action::Ranged { target, .. }
        | Action::AirStrike { target, .. }
        | Action::CityStrike { target, .. }
        | Action::EncampmentStrike { target, .. } => Some(*target),
        Action::FoundCity { unit }
        | Action::Improve { unit, .. }
        | Action::Pillage { unit }
        | Action::RepairImprovement { unit }
        | Action::CoastalRaid { unit, .. }
        | Action::Fortify { unit }
        | Action::Promote { unit, .. }
        | Action::Spread { unit }
        | Action::PerformConcert { unit } => g.units.get(unit).map(|u| u.pos),
        Action::Produce { city, .. }
        | Action::Buy { city, .. }
        | Action::BuyBuilding { city, .. }
        | Action::BuyDistrict { city, .. }
        | Action::KeepCity { city }
        | Action::RazeCity { city }
        | Action::LiberateCity { city } => g.cities.get(city).map(|c| c.pos),
        _ => None,
    }
}

/// One action's fixed-width feature row: the kind one-hot, then a shared
/// numeric block — has-target, own-tile, enemy-tile, target city HP,
/// attacker HP/strength, distance to the acting unit, whether the target is
/// ours, and normalized costs. Everything is derived from `pid`'s own view,
/// so a fog-honest policy stays fog-honest.
pub fn features(g: &Game, pid: usize, action: &Action) -> Vec<f32> {
    let mut row = vec![0.0f32; FEATURE_WIDTH];
    row[kind_index(action)] = 1.0;
    let base = KINDS.len();
    let tile = target_tile(g, action);
    if let Some(pos) = tile {
        row[base] = 1.0;
        if let Some(t) = g.map.get(pos) {
            let owned = t
                .owner_city
                .and_then(|c| g.cities.get(&c))
                .map(|c| c.owner == pid);
            row[base + 1] = matches!(owned, Some(true)) as u8 as f32;
            row[base + 2] = matches!(owned, Some(false)) as u8 as f32;
        }
        if let Some(cid) = g.city_at(pos) {
            let city = &g.cities[&cid];
            row[base + 3] = (city.hp as f32 / 200.0).clamp(0.0, 1.0);
            row[base + 4] = (city.owner == pid) as u8 as f32;
        }
        let enemy = g.units_at(pos).into_iter().any(|uid| {
            let u = &g.units[&uid];
            u.owner != pid && g.is_at_war(pid, u.owner)
        });
        row[base + 5] = enemy as u8 as f32;
    }
    if let Some(uid) = acting_unit(action) {
        if let Some(unit) = g.units.get(&uid) {
            row[base + 6] = (unit.hp as f32 / 100.0).clamp(0.0, 1.0);
            row[base + 7] =
                (g.unit_strength(unit, false) as f32 / 100.0).clamp(0.0, 1.0);
            row[base + 8] = (unit.moves_left as f32 / 6.0).clamp(0.0, 1.0);
            if let Some(pos) = tile {
                row[base + 9] = (g.wdist(unit.pos, pos) as f32 / 10.0).clamp(0.0, 1.0);
            }
        }
    }
    // Treasury and Faith give the policy the context that makes purchase
    // actions comparable across turns.
    row[base + 10] = (g.players[pid].gold as f32 / 2000.0).clamp(0.0, 1.0);
    row[base + 11] = (g.players[pid].faith as f32 / 1000.0).clamp(0.0, 1.0);
    row
}

fn acting_unit(action: &Action) -> Option<u32> {
    match action {
        Action::Move { unit, .. }
        | Action::MoveTo { unit, .. }
        | Action::Attack { unit, .. }
        | Action::Ranged { unit, .. }
        | Action::FoundCity { unit }
        | Action::Improve { unit, .. }
        | Action::Pillage { unit }
        | Action::RepairImprovement { unit }
        | Action::CoastalRaid { unit, .. }
        | Action::AirRebase { unit, .. }
        | Action::AirStrike { unit, .. }
        | Action::AirPatrol { unit, .. }
        | Action::Fortify { unit }
        | Action::Promote { unit, .. }
        | Action::Spread { unit }
        | Action::PerformConcert { unit } => Some(*unit),
        _ => None,
    }
}

pub struct Encoded {
    pub actions: Vec<Action>,
    pub kinds: Vec<usize>,
    /// `actions.len() * FEATURE_WIDTH`, row-major.
    pub features: Vec<f32>,
    /// Which of [`KINDS`] appear in this legal set.
    pub kind_mask: [bool; KINDS.len()],
}

pub fn legal_encoded(g: &Game, pid: usize) -> Encoded {
    let actions = g.legal_actions(pid);
    let mut kinds = Vec::with_capacity(actions.len());
    let mut features = Vec::with_capacity(actions.len() * FEATURE_WIDTH);
    let mut kind_mask = [false; KINDS.len()];
    for action in &actions {
        let k = kind_index(action);
        kinds.push(k);
        kind_mask[k] = true;
        features.extend(features_row(g, pid, action));
    }
    Encoded {
        actions,
        kinds,
        features,
        kind_mask,
    }
}

fn features_row(g: &Game, pid: usize, action: &Action) -> Vec<f32> {
    features(g, pid, action)
}

#[cfg(test)]
mod tests {
    use super::{kind_index, legal_encoded, FEATURE_WIDTH, KINDS};
    use crate::ai::{Ai, AdvancedAi};
    use crate::game::{Action, Game};

    #[test]
    fn kinds_are_unique_and_cover_every_legal_action() {
        let mut sorted = KINDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), KINDS.len(), "duplicate kind name");

        // Play a real game and encode every legal action seen along the way:
        // any unlisted variant panics in kind_index.
        let mut g = Game::new(4, 28, 18, 12, 60, 2);
        let mut ais = AdvancedAi::fleet(&g);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            if g.winner.is_some() {
                break;
            }
            let pid = g.current;
            for action in g.legal_actions(pid) {
                seen.insert(kind_index(&action));
            }
            ais[pid].take_turn(&mut g, pid);
            if g.winner.is_none() && g.current == pid {
                let _ = g.apply(pid, &Action::EndTurn);
            }
        }
        assert!(seen.len() > 5, "expected a varied action set, saw {seen:?}");
    }

    #[test]
    fn encoding_shape_matches_the_legal_set() {
        let g = Game::new(4, 28, 18, 4, 80, 2);
        let e = legal_encoded(&g, 0);
        assert!(!e.actions.is_empty());
        assert_eq!(e.kinds.len(), e.actions.len());
        assert_eq!(e.features.len(), e.actions.len() * FEATURE_WIDTH);
        assert!(e.features.iter().all(|v| v.is_finite()));
        // The mask must agree with the encoded kinds exactly.
        for (index, present) in e.kind_mask.iter().enumerate() {
            assert_eq!(*present, e.kinds.contains(&index), "mask disagrees at {index}");
        }
        // Each row's one-hot names that row's kind.
        for (row, kind) in e.kinds.iter().enumerate() {
            let slice = &e.features[row * FEATURE_WIDTH..(row + 1) * FEATURE_WIDTH];
            assert_eq!(slice[*kind], 1.0);
            assert_eq!(slice[..KINDS.len()].iter().sum::<f32>(), 1.0);
        }
    }
}
