//! StrategicAi: rollout-based victory routing on top of AdvancedAi.
//!
//! Every `review_every` turns the agent simulates staying adaptive and committing
//! to each enabled victory lane for `horizon` rounds (rivals remain AdvancedAi
//! opponents). It commits only when a targeted policy beats its adaptive parent
//! by a real margin — macro search applied to victory routing, generalizing the
//! war-decision rollouts `NeuralAi` proved. Positions are judged by the trained
//! value net when `evolved/valuenet.json` exists, otherwise by score share. Public
//! victory threats interrupt the periodic search before they can end the game,
//! while irreversible Prophet investment and duel victory geometry supply
//! priors that a short economic rollout cannot discover in time. The learned
//! estimate is deliberately regularized toward score share because the
//! counterfactual rollout endpoints remain out of distribution for ordinary
//! self-play trajectories.
use crate::ai::{AdvancedAi, Ai, PlanReport, VictoryTarget, Weights};
use crate::evolve::features;
use crate::game::{Action, Game, Item};
use crate::valuenet::ValueNet;

const TARGET_COMMITMENT_MARGIN: f64 = 0.01;
const VALUE_NET_WEIGHT: f64 = 0.25;

pub struct StrategicAi {
    inner: AdvancedAi,
    weights: Weights,
    net: Option<ValueNet>,
    pub review_every: u32,
    pub horizon: u32,
    next_review: u32,
}

impl Default for StrategicAi {
    fn default() -> Self {
        Self::new()
    }
}

impl StrategicAi {
    pub fn new() -> StrategicAi {
        Self::with_weights(Weights::default())
    }

    pub fn with_weights(weights: Weights) -> StrategicAi {
        StrategicAi {
            inner: AdvancedAi::with_weights(weights.clone()),
            weights,
            net: ValueNet::load("evolved"),
            review_every: 40,
            horizon: 40,
            // The opening book plays itself; the first lane choice lands
            // once the empire exists enough for lanes to differ.
            next_review: 30,
        }
    }

    pub fn current_target(&self) -> Option<VictoryTarget> {
        self.inner.victory_target()
    }

    fn position_value(&self, g: &Game, pid: usize) -> f64 {
        if !g.players[pid].alive {
            return 0.0;
        }
        // Score share among living majors; 1/majors is parity.
        let mut own = 0.0;
        let mut total = 0.0;
        for p in &g.players {
            if p.is_minor || p.is_barbarian || !p.alive {
                continue;
            }
            let s = g.score(p.id).max(0) as f64;
            total += s;
            if p.id == pid {
                own = s;
            }
        }
        let score_share = if total <= 0.0 {
            0.5
        } else {
            own / total
        };
        if let Some(net) = &self.net {
            let learned = net.eval(&features(g, pid));
            if learned.is_finite() {
                score_share + VALUE_NET_WEIGHT * (learned - score_share)
            } else {
                score_share
            }
        } else {
            score_share
        }
    }

    fn target_enabled(g: &Game, target: VictoryTarget) -> bool {
        match target {
            VictoryTarget::Science => g.victory_conditions.science,
            VictoryTarget::Culture => g.victory_conditions.culture,
            VictoryTarget::Religion => g.victory_conditions.religious,
            VictoryTarget::Diplomacy => g.victory_conditions.diplomatic,
            VictoryTarget::Domination => g.victory_conditions.domination,
            VictoryTarget::Score => g.victory_conditions.score,
        }
    }

    /// Prophet slots are an irreversible global race. Once the opening book
    /// has invested in Astrology, a Holy Site, or Prophet points, a 30-turn
    /// score projection must not throw that option away while a slot remains.
    fn viable_religious_commitment(g: &Game, pid: usize) -> bool {
        let player = &g.players[pid];
        if !g.victory_conditions.religious || player.religion.is_some() {
            return false;
        }
        if player.prophet_pending {
            return true;
        }
        let claimed = g.religions_founded()
            + g.players
                .iter()
                .filter(|candidate| candidate.prophet_pending)
                .count();
        if claimed >= g.max_religions() {
            return false;
        }
        let cities = g.player_city_ids(pid);
        let holy_site = cities.iter().any(|city| {
            g.cities[city].districts.contains_key("holy_site")
                || matches!(
                    g.cities[city].queue.first(),
                    Some(Item::District { district, .. }) if district == "holy_site"
                )
        });
        holy_site
            || player.techs.contains("astrology")
            || player.research.as_deref() == Some("astrology")
            || player.gpp.get("prophet").copied().unwrap_or(0.0) > 0.0
    }

    /// A rival founding first does not close the religious race when this
    /// empire can still claim another prophet slot and place a Holy Site.
    fn religious_option_open(g: &Game, pid: usize) -> bool {
        let player = &g.players[pid];
        if !g.victory_conditions.religious || player.religion.is_some() {
            return false;
        }
        let claimed = g.religions_founded()
            + g.players
                .iter()
                .filter(|candidate| candidate.prophet_pending)
                .count();
        let cities = g.player_city_ids(pid);
        claimed < g.max_religions()
            && cities.len() >= 2
            && cities.iter().any(|city| {
                g.cities[city].districts.contains_key("holy_site")
                    || matches!(
                        g.cities[city].queue.first(),
                        Some(Item::District { district, .. }) if district == "holy_site"
                    )
                    || !g.district_sites(*city, "holy_site").is_empty()
            })
    }

    /// In a duel, Religious Victory needs only one foreign conversion. The
    /// prophet race is therefore a must-contest game-ending objective, not an
    /// optional yield specialization. Multiplayer keeps the normal search.
    fn duel_religious_race(g: &Game, pid: usize) -> bool {
        if !g.victory_conditions.religious {
            return false;
        }
        let living = g
            .players
            .iter()
            .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
            .count();
        if living != 2 {
            return false;
        }
        if g.players[pid].religion.is_some() || g.players[pid].prophet_pending {
            return true;
        }
        let claimed = g.religions_founded()
            + g.players
                .iter()
                .filter(|candidate| candidate.prophet_pending)
                .count();
        claimed < g.max_religions()
    }

    /// Public victory-screen progress distilled to the same 0..100 scale for
    /// every lane. This deliberately scores only concrete endgame progress;
    /// the rollouts remain responsible for comparing ordinary development.
    fn victory_progress(g: &Game, pid: usize, target: VictoryTarget) -> i32 {
        let player = &g.players[pid];
        let starting_majors: Vec<usize> = g
            .players
            .iter()
            .filter(|candidate| !candidate.is_minor && !candidate.is_barbarian)
            .map(|candidate| candidate.id)
            .collect();
        let living_majors: Vec<usize> = starting_majors
            .iter()
            .copied()
            .filter(|candidate| g.players[*candidate].alive)
            .collect();
        match target {
            VictoryTarget::Science => {
                if player.science_projects.contains("exoplanet_expedition") {
                    75 + (25.0 * player.exoplanet_distance / 50.0).clamp(0.0, 25.0) as i32
                } else if player.science_projects.contains("launch_mars_colony") {
                    65
                } else if player.science_projects.contains("launch_moon_landing") {
                    45
                } else if player.science_projects.contains("launch_earth_satellite") {
                    25
                } else {
                    0
                }
            }
            VictoryTarget::Culture => {
                let target = living_majors
                    .iter()
                    .filter(|other| **other != pid)
                    .map(|other| g.domestic_tourists(*other))
                    .max()
                    .unwrap_or(1)
                    .max(1);
                (100 * g.foreign_tourists(pid) / target).clamp(0, 100) as i32
            }
            VictoryTarget::Religion => player.religion.as_ref().map_or(0, |religion| {
                let converted = living_majors
                    .iter()
                    .filter(|other| {
                        let cities = g.player_city_ids(**other);
                        let following = cities
                            .iter()
                            .filter(|city| {
                                g.city_religion(&g.cities[city]) == Some(religion.as_str())
                            })
                            .count();
                        !cities.is_empty() && following * 2 > cities.len()
                    })
                    .count();
                (100 * converted / living_majors.len().max(1)) as i32
            }),
            VictoryTarget::Diplomacy => (player.dvp * 5).clamp(0, 100) as i32,
            VictoryTarget::Domination => {
                let foreign_capitals = starting_majors
                    .iter()
                    .filter(|owner| **owner != pid)
                    .count();
                let controlled = g
                    .cities
                    .values()
                    .filter(|city| {
                        city.is_capital && city.original_owner != pid && city.owner == pid
                    })
                    .count();
                (100 * controlled)
                    .checked_div(foreign_capitals)
                    .unwrap_or(0) as i32
            }
            VictoryTarget::Score => {
                let leading = living_majors
                    .iter()
                    .map(|candidate| g.score(*candidate))
                    .max();
                if g.max_turns > 0
                    && g.turn.saturating_mul(4) >= g.max_turns.saturating_mul(3)
                    && leading == Some(g.score(pid))
                {
                    (40 + 60 * g.turn.min(g.max_turns) / g.max_turns) as i32
                } else {
                    0
                }
            }
        }
    }

    /// A short economic rollout must not choose a prosperous losing line.
    /// Interrupt it when public race state says a rival can end the game
    /// before the next review. Religious progress advances in whole-civ jumps,
    /// so it warns with two holdouts left and becomes unconditional at match
    /// point; continuous races use the same 78% / 15-point urgency margin as
    /// AdvancedAi's adaptive planner.
    fn urgent_counter_target(&self, g: &Game, pid: usize) -> Option<VictoryTarget> {
        let own_progress = VictoryTarget::ALL
            .into_iter()
            .filter(|target| Self::target_enabled(g, *target))
            .map(|target| Self::victory_progress(g, pid, target))
            .max()
            .unwrap_or(0);
        let mut threat: Option<(i32, usize, VictoryTarget)> = None;
        for rival in g.players.iter().filter(|player| {
            player.id != pid && player.alive && !player.is_minor && !player.is_barbarian
        }) {
            for target in VictoryTarget::ALL {
                if !Self::target_enabled(g, target) {
                    continue;
                }
                let progress = Self::victory_progress(g, rival.id, target);
                let candidate = (progress, usize::MAX - rival.id, target);
                if threat.as_ref().is_none_or(|best| {
                    candidate.0 > best.0 || (candidate.0 == best.0 && candidate.1 > best.1)
                }) {
                    threat = Some(candidate);
                }
            }
        }
        let (progress, _, target) = threat?;
        if target == VictoryTarget::Religion {
            let living = g
                .players
                .iter()
                .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
                .count()
                .max(1) as i32;
            let match_point = 100 * living.saturating_sub(1) / living;
            let early_warning = (100 * living.saturating_sub(2) / living)
                .max(50)
                .min(match_point);
            if progress < early_warning || (progress < match_point && progress < own_progress + 15)
            {
                return None;
            }
            return Some(if g.players[pid].religion.is_some() {
                VictoryTarget::Religion
            } else if Self::viable_religious_commitment(g, pid)
                || Self::religious_option_open(g, pid)
            {
                VictoryTarget::Religion
            } else {
                VictoryTarget::Domination
            });
        }
        if progress < 78 || progress < own_progress + 15 {
            return None;
        }
        Some(match target {
            VictoryTarget::Culture => VictoryTarget::Culture,
            VictoryTarget::Diplomacy => VictoryTarget::Diplomacy,
            VictoryTarget::Science | VictoryTarget::Domination | VictoryTarget::Score => {
                VictoryTarget::Domination
            }
            VictoryTarget::Religion => unreachable!(),
        })
    }

    /// Projected value of staying adaptive (`None`) or committing to `target`
    /// for `horizon` rounds.
    fn rollout(&self, g: &Game, pid: usize, target: Option<VictoryTarget>) -> f64 {
        let mut sim = g.clone();
        let mut ais: Vec<Box<dyn Ai>> = sim
            .players
            .iter()
            .map(|p| {
                if p.id == pid {
                    if let Some(target) = target {
                        Box::new(AdvancedAi::with_weights_and_target(
                            self.weights.clone(),
                            target,
                        )) as Box<dyn Ai>
                    } else {
                        Box::new(AdvancedAi::with_weights(self.weights.clone())) as Box<dyn Ai>
                    }
                } else {
                    // The counterfactual must preserve the opponent class the
                    // strategic layer is trying to beat. BasicAi understates
                    // victory pressure (especially religion), so a locally
                    // attractive Science rollout can be globally losing.
                    Box::new(AdvancedAi::new()) as Box<dyn Ai>
                }
            })
            .collect();
        let stop = sim.turn + self.horizon;
        while sim.winner.is_none() && sim.turn < stop {
            let p = sim.current;
            ais[p].take_turn(&mut sim, p);
            if sim.winner.is_none() && sim.current == p {
                let _ = sim.apply(p, &Action::EndTurn);
            }
        }
        match sim.winner {
            Some(w) if w == pid => 1.0,
            Some(_) => 0.0,
            None => self.position_value(&sim, pid),
        }
    }

    fn choose_rollout_target(
        &self,
        values: &[(f64, Option<VictoryTarget>)],
    ) -> Option<VictoryTarget> {
        let adaptive = values
            .iter()
            .find_map(|(value, target)| target.is_none().then_some(*value))
            .expect("adaptive rollout is always present");
        let mut best_target = None;
        for candidate in values
            .iter()
            .copied()
            .filter(|(_, target)| target.is_some())
        {
            if best_target.is_none_or(|best: (f64, Option<VictoryTarget>)| candidate.0 > best.0) {
                best_target = Some(candidate);
            }
        }
        best_target
            .filter(|(value, _)| *value > adaptive + TARGET_COMMITMENT_MARGIN)
            .and_then(|(_, target)| target)
    }

    /// Compare the adaptive parent with every enabled victory lane. Deterministic:
    /// rollouts are seed-free clones and ties keep declaration order; an explicit
    /// lane must clear the adaptive value by `TARGET_COMMITMENT_MARGIN`.
    pub fn review(&self, g: &Game, pid: usize) -> Option<VictoryTarget> {
        if Self::duel_religious_race(g, pid) {
            return Some(VictoryTarget::Religion);
        }
        if let Some(counter) = self.urgent_counter_target(g, pid) {
            return Some(counter);
        }
        if Self::viable_religious_commitment(g, pid) {
            return Some(VictoryTarget::Religion);
        }
        let mut values = vec![(self.rollout(g, pid, None), None)];
        for target in VictoryTarget::ALL
            .into_iter()
            .filter(|target| Self::target_enabled(g, *target))
        {
            values.push((self.rollout(g, pid, Some(target)), Some(target)));
        }
        self.choose_rollout_target(&values)
    }
}

impl Ai for StrategicAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        let major = !g.players[pid].is_minor && !g.players[pid].is_barbarian;
        let counter = major.then(|| self.urgent_counter_target(g, pid)).flatten();
        let interrupted = counter.is_some_and(|target| self.inner.victory_target() != Some(target));
        if major && g.winner.is_none() && (g.turn >= self.next_review || interrupted) {
            self.next_review = g.turn + self.review_every;
            // Public victory threats are cheap to inspect every turn and may
            // end the game before the next expensive six-lane review. Reuse
            // the already-computed counter rather than running rollouts.
            let target = counter.map(Some).unwrap_or_else(|| self.review(g, pid));
            match target {
                Some(target) if self.inner.victory_target() != Some(target) => {
                    self.inner.retarget(target)
                }
                None if self.inner.victory_target().is_some() => self.inner.adapt(),
                _ => {}
            }
        }
        self.inner.take_turn(g, pid);
    }

    fn strategy_label(&self) -> Option<&'static str> {
        self.inner.strategy_label()
    }

    fn plan_report(&self) -> Option<PlanReport> {
        self.inner.plan_report()
    }
}

#[cfg(test)]
mod tests {
    use super::StrategicAi;
    use crate::ai::{run_game, Ai, BasicAi, VictoryTarget};
    use crate::game::{Action, Game};
    use crate::valuenet::ValueNet;

    fn found_capitals(game: &mut Game, players: usize) {
        for pid in 0..players {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.current = pid;
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
    }

    #[test]
    fn default_macro_search_looks_forty_rounds_ahead() {
        assert_eq!(StrategicAi::new().horizon, 40);
    }

    #[test]
    fn learned_values_are_regularized_toward_score_share() {
        let game = Game::new(4, 24, 16, 20, 180, 0);
        let total: i64 = game
            .players
            .iter()
            .filter(|player| !player.is_minor && !player.is_barbarian)
            .map(|player| game.score(player.id).max(0))
            .sum();
        let score_share = if total > 0 {
            game.score(0).max(0) as f64 / total as f64
        } else {
            0.5
        };
        let mut strategic = StrategicAi::new();
        strategic.net = None;
        assert!((strategic.position_value(&game, 0) - score_share).abs() < 1e-12);
        strategic.net = Some(ValueNet {
            sizes: vec![25, 64, 32, 1],
            weights: vec![
                vec![vec![0.0; 64]; 25],
                vec![vec![0.0; 32]; 64],
                vec![vec![0.0; 1]; 32],
            ],
            biases: vec![vec![0.0; 64], vec![0.0; 32], vec![0.0; 1]],
        });

        let expected = score_share + 0.25 * (0.5 - score_share);
        assert!((strategic.position_value(&game, 0) - expected).abs() < 1e-12);
    }

    /// The review must pick a lane deterministically and commit it to the
    /// wrapped agent; a tiny horizon keeps the six rollouts fast.
    #[test]
    fn review_selects_and_commits_a_victory_lane() {
        let mut g = Game::new(2, 20, 14, 21, 120, 0);
        let mut ais = BasicAi::fleet(&g);
        for _ in 0..12 {
            for pid in 0..g.players.len() {
                if g.winner.is_some() {
                    break;
                }
                ais[pid].take_turn(&mut g, pid);
            }
        }
        let mut strategic = StrategicAi::new();
        strategic.horizon = 4;
        strategic.next_review = 0;
        let first = strategic.review(&g, 0);
        assert_eq!(first, strategic.review(&g, 0), "review must be deterministic");
        assert_eq!(strategic.current_target(), None);
        strategic.take_turn(&mut g, 0);
        assert_eq!(strategic.current_target(), first);
    }

    #[test]
    fn religious_match_point_interrupts_the_economic_rollouts() {
        let mut game = Game::new_full(2, 24, 16, 22, 180, 0, false);
        found_capitals(&mut game, 2);
        game.players[0].religion = Some("Home Faith".to_string());
        game.players[1].religion = Some("Rival Faith".to_string());
        for (owner, religion) in [(0, "Home Faith"), (1, "Rival Faith")] {
            let capital = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&capital)
                .unwrap()
                .pressure
                .insert(religion.to_string(), 1_000.0);
        }

        let strategic = StrategicAi::new();
        assert_eq!(
            strategic.urgent_counter_target(&game, 0),
            Some(VictoryTarget::Religion)
        );
        assert_eq!(strategic.review(&game, 0), Some(VictoryTarget::Religion));

        game.players[0].religion = None;
        assert_eq!(
            strategic.urgent_counter_target(&game, 0),
            Some(VictoryTarget::Domination)
        );
    }

    #[test]
    fn strategic_review_preserves_a_viable_prophet_investment() {
        let mut game = Game::new_full(3, 24, 16, 24, 180, 0, false);
        found_capitals(&mut game, 3);
        game.players[0].research = Some("astrology".to_string());

        assert!(StrategicAi::viable_religious_commitment(&game, 0));
        assert_eq!(
            StrategicAi::new().review(&game, 0),
            Some(VictoryTarget::Religion)
        );

        game.victory_conditions.religious = false;
        assert!(!StrategicAi::viable_religious_commitment(&game, 0));
    }

    #[test]
    fn duel_treats_the_prophet_race_as_a_mandatory_objective() {
        let mut game = Game::new_full(2, 24, 16, 26, 180, 0, false);
        found_capitals(&mut game, 2);
        let strategic = StrategicAi::new();

        assert!(StrategicAi::duel_religious_race(&game, 0));
        assert_eq!(strategic.review(&game, 0), Some(VictoryTarget::Religion));

        game.victory_conditions.religious = false;
        assert!(!StrategicAi::duel_religious_race(&game, 0));
    }

    #[test]
    fn imminent_victory_interrupts_before_the_periodic_review() {
        let mut game = Game::new_full(2, 24, 16, 25, 180, 0, false);
        found_capitals(&mut game, 2);
        game.players[0].religion = Some("Home Faith".to_string());
        game.players[1].religion = Some("Rival Faith".to_string());
        for (owner, religion) in [(0, "Home Faith"), (1, "Rival Faith")] {
            let capital = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&capital)
                .unwrap()
                .pressure
                .insert(religion.to_string(), 1_000.0);
        }

        let mut strategic = StrategicAi::new();
        strategic.inner.retarget(VictoryTarget::Science);
        strategic.next_review = game.turn + 100;
        strategic.take_turn(&mut game, 0);

        assert_eq!(strategic.current_target(), Some(VictoryTarget::Religion));
        assert!(strategic.next_review < game.turn + 100);
    }

    #[test]
    fn imminent_space_race_routes_to_denial() {
        let mut game = Game::new_full(3, 24, 16, 23, 300, 0, false);
        found_capitals(&mut game, 3);
        game.players[2].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        game.players[2].exoplanet_distance = 42.0;

        assert_eq!(
            StrategicAi::new().urgent_counter_target(&game, 0),
            Some(VictoryTarget::Domination)
        );
    }

    #[test]
    fn explicit_lane_must_clear_the_adaptive_rollout() {
        let strategic = StrategicAi::new();
        assert_eq!(
            strategic
                .choose_rollout_target(&[(0.30, None), (0.305, Some(VictoryTarget::Domination)),]),
            None
        );
        assert_eq!(
            strategic.choose_rollout_target(&[(0.30, None), (0.32, Some(VictoryTarget::Science)),]),
            Some(VictoryTarget::Science)
        );
    }

    #[test]
    fn periodic_review_can_return_a_targeted_agent_to_adaptive_planning() {
        let mut game = Game::new_full(3, 24, 16, 31, 300, 0, false);
        found_capitals(&mut game, 3);
        game.victory_conditions.culture = false;
        game.victory_conditions.religious = false;
        game.victory_conditions.diplomatic = false;
        game.victory_conditions.domination = false;
        game.victory_conditions.score = false;
        let mut strategic = StrategicAi::new();
        strategic.inner.retarget(VictoryTarget::Science);
        strategic.horizon = 0;
        strategic.next_review = 0;
        assert_eq!(strategic.current_target(), Some(VictoryTarget::Science));
        strategic.take_turn(&mut game, 0);
        assert_eq!(strategic.current_target(), None);
    }

    #[test]
    fn review_never_selects_a_disabled_lane() {
        let mut game = Game::new_full(3, 24, 16, 30, 300, 0, false);
        found_capitals(&mut game, 3);
        game.victory_conditions.culture = false;
        game.victory_conditions.religious = false;
        game.victory_conditions.diplomatic = false;
        game.victory_conditions.domination = false;
        game.victory_conditions.score = false;
        let mut strategic = StrategicAi::new();
        strategic.horizon = 0;

        assert_eq!(strategic.review(&game, 0), None);
    }

    /// Full smoke game: a strategic seat finishes a real game without
    /// upsetting the loop.
    #[test]
    fn strategic_seat_completes_a_game() {
        let mut g = Game::new(2, 20, 14, 3, 80, 0);
        let mut strategic = StrategicAi::new();
        strategic.horizon = 6;
        strategic.review_every = 25;
        // One AI per player seat, barbarians included.
        let mut ais: Vec<Box<dyn Ai>> = g
            .players
            .iter()
            .map(|p| {
                if p.id == 0 {
                    Box::new(std::mem::take(&mut strategic)) as Box<dyn Ai>
                } else {
                    Box::new(BasicAi::new()) as Box<dyn Ai>
                }
            })
            .collect();
        run_game(&mut g, &mut ais);
        assert!(g.winner.is_some());
    }
}
