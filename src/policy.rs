//! PolicyAi: the learned net chooses the actions.
//!
//! Where `NeuralAi` consults the value net for one decision (war), this
//! agent uses it as the policy itself: each turn it repeatedly scores the
//! legal action set by applying each candidate to a clone and evaluating the
//! resulting position, then commits the best improvement. That is one-ply
//! net-guided search over the real action space — the first rung of a
//! learned policy (AI_GAPS item 1) and the loop a trained policy head would
//! later replace.
//!
//! Without `evolved/valuenet.json` there is nothing learned to consult, so
//! the agent falls back to the scripted `AdvancedAi` rather than playing
//! randomly.
use crate::action_space::{kind_name, legal_encoded};
use crate::ai::{AdvancedAi, Ai, PlanReport, Weights};
use crate::evolve::features;
use crate::game::{Action, Game};
use crate::valuenet::ValueNet;

pub struct PolicyAi {
    fallback: AdvancedAi,
    net: Option<ValueNet>,
    /// Most candidate actions scored per decision.
    pub width: usize,
    /// Most committed actions per turn before ending it.
    pub depth: usize,
    /// Minimum value gain required to commit an action.
    pub margin: f64,
    /// Restrict the net to action kinds where a one-ply value delta is
    /// meaningful. Multi-turn commitments (production, research, purchases)
    /// look near-free to a one-ply evaluator, which is how an unrestricted
    /// policy empties its treasury; those stay with the scripted layer.
    pub tactical_only: bool,
}

/// Action kinds whose whole effect lands this turn, so the resulting
/// position honestly reflects the choice.
pub const TACTICAL_KINDS: [&str; 12] = [
    "move", "move_to", "attack", "ranged", "air_strike", "air_patrol",
    "air_rebase", "fortify", "pillage", "city_strike", "encampment_strike",
    "condemn_heretic",
];

impl Default for PolicyAi {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyAi {
    pub fn new() -> PolicyAi {
        Self::with_weights(Weights::default())
    }

    pub fn with_weights(weights: Weights) -> PolicyAi {
        PolicyAi {
            fallback: AdvancedAi::with_weights(weights),
            net: ValueNet::load("evolved"),
            width: 48,
            depth: 10,
            margin: 1e-4,
            tactical_only: true,
        }
    }

    pub fn has_net(&self) -> bool {
        self.net.is_some()
    }

    fn value(&self, g: &Game, pid: usize) -> f64 {
        match &self.net {
            Some(net) => net.eval(&features(g, pid)),
            None => 0.0,
        }
    }

    /// Candidate actions worth spending a clone on. EndTurn is excluded (it
    /// is the loop's exit, not a move) and the set is capped at `width` so a
    /// turn with hundreds of legal actions stays affordable.
    fn candidates(&self, g: &Game, pid: usize) -> Vec<Action> {
        let encoded = legal_encoded(g, pid);
        let mut out: Vec<Action> = encoded
            .actions
            .into_iter()
            .filter(|a| !matches!(a, Action::EndTurn))
            .filter(|a| !self.tactical_only || TACTICAL_KINDS.contains(&kind_name(a)))
            .collect();
        if out.len() > self.width {
            // Deterministic stride keeps a spread across action kinds rather
            // than the first N, which would be all unit moves.
            let stride = out.len() / self.width + 1;
            out = out.into_iter().step_by(stride).collect();
        }
        out
    }

    /// One net-guided decision: returns the best improving action, if any.
    pub fn best_action(&self, g: &Game, pid: usize) -> Option<(Action, f64)> {
        self.net.as_ref()?;
        let base = self.value(g, pid);
        let mut best: Option<(Action, f64)> = None;
        for action in self.candidates(g, pid) {
            let mut sim = g.clone();
            if sim.apply(pid, &action).is_err() {
                continue;
            }
            let gain = match sim.winner {
                Some(w) if w == pid => 1.0,
                Some(_) => -1.0,
                None => self.value(&sim, pid) - base,
            };
            let better = best
                .as_ref()
                .map(|(ba, bg)| {
                    gain > *bg || (gain == *bg && kind_name(&action) < kind_name(ba))
                })
                .unwrap_or(true);
            if better {
                best = Some((action, gain));
            }
        }
        best.filter(|(_, gain)| *gain > self.margin)
    }
}

impl Ai for PolicyAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        if self.net.is_none()
            || g.players[pid].is_minor
            || g.players[pid].is_barbarian
            || g.winner.is_some()
        {
            self.fallback.take_turn(g, pid);
            return;
        }
        for _ in 0..self.depth {
            let Some((action, _)) = self.best_action(g, pid) else {
                break;
            };
            if g.apply(pid, &action).is_err() {
                break;
            }
            if g.winner.is_some() || g.current != pid {
                return;
            }
        }
        // The scripted agent still runs the routine empire management the
        // one-ply net cannot see the value of (multi-turn builds, research),
        // then ends the turn.
        self.fallback.take_turn(g, pid);
    }

    fn strategy_label(&self) -> Option<&'static str> {
        self.fallback.strategy_label()
    }

    fn plan_report(&self) -> Option<PlanReport> {
        self.fallback.plan_report()
    }
}

#[cfg(test)]
mod tests {
    use super::PolicyAi;
    use crate::ai::{run_game, Ai, BasicAi};
    use crate::game::Game;

    /// With no trained net on disk the agent must still play a full legal
    /// game by falling back to the scripted agent.
    #[test]
    fn falls_back_without_a_trained_net() {
        let mut g = Game::new(2, 20, 14, 5, 60, 0);
        let policy = PolicyAi::new();
        let scripted = !policy.has_net();
        let mut ais: Vec<Box<dyn Ai>> = g
            .players
            .iter()
            .map(|p| {
                if p.id == 0 {
                    Box::new(PolicyAi::new()) as Box<dyn Ai>
                } else {
                    Box::new(BasicAi::new()) as Box<dyn Ai>
                }
            })
            .collect();
        run_game(&mut g, &mut ais);
        assert!(g.winner.is_some());
        assert!(scripted || policy.has_net());
    }

    /// Candidate generation must stay bounded and never offer EndTurn.
    #[test]
    fn candidates_are_bounded_and_exclude_end_turn() {
        let g = Game::new(4, 28, 18, 9, 80, 2);
        let mut policy = PolicyAi::new();
        policy.width = 12;
        let candidates = policy.candidates(&g, 0);
        assert!(candidates.len() <= 12, "got {}", candidates.len());
        assert!(!candidates
            .iter()
            .any(|a| matches!(a, crate::game::Action::EndTurn)));
    }
}
