//! StrategicAi: rollout-based victory routing on top of AdvancedAi.
//!
//! Every `review_every` turns the agent simulates committing to each victory
//! lane for `horizon` rounds (itself as a lane-targeted AdvancedAi, rivals as
//! fast scripted AIs) and adopts the lane with the best projected position —
//! macro search applied to victory routing, generalizing the war-decision
//! rollouts `NeuralAi` proved. Positions are judged by the trained value net
//! when `evolved/valuenet.json` exists, otherwise by score share.
use crate::ai::{AdvancedAi, Ai, BasicAi, PlanReport, VictoryTarget, Weights};
use crate::evolve::features;
use crate::game::{Action, Game};
use crate::valuenet::ValueNet;

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
            horizon: 30,
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
        if let Some(net) = &self.net {
            return net.eval(&features(g, pid));
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
        if total <= 0.0 {
            0.5
        } else {
            own / total
        }
    }

    /// Projected value of committing to `target` for `horizon` rounds.
    fn rollout(&self, g: &Game, pid: usize, target: VictoryTarget) -> f64 {
        let mut sim = g.clone();
        let mut ais: Vec<Box<dyn Ai>> = sim
            .players
            .iter()
            .map(|p| {
                if p.id == pid {
                    Box::new(AdvancedAi::with_weights_and_target(
                        self.weights.clone(),
                        target,
                    )) as Box<dyn Ai>
                } else {
                    Box::new(BasicAi::new()) as Box<dyn Ai>
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

    /// Evaluate every victory lane and return the best. Deterministic:
    /// rollouts are seed-free clones and ties keep declaration order.
    pub fn review(&self, g: &Game, pid: usize) -> VictoryTarget {
        let mut best: Option<(f64, VictoryTarget)> = None;
        for target in VictoryTarget::ALL {
            let value = self.rollout(g, pid, target);
            if best.map(|(b, _)| value > b).unwrap_or(true) {
                best = Some((value, target));
            }
        }
        best.map(|(_, target)| target)
            .unwrap_or(VictoryTarget::Score)
    }
}

impl Ai for StrategicAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        let major = !g.players[pid].is_minor && !g.players[pid].is_barbarian;
        if major && g.winner.is_none() && g.turn >= self.next_review {
            self.next_review = g.turn + self.review_every;
            let target = self.review(g, pid);
            if self.inner.victory_target() != Some(target) {
                self.inner.retarget(target);
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
    use crate::ai::{run_game, Ai, BasicAi};
    use crate::game::Game;

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
        assert_eq!(strategic.current_target(), Some(first));
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
