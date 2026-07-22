//! NeuralAi: champion-weight BasicAi play with value-net-guided strategy.
//! War declarations are decided AlphaZero-style-in-miniature: clone the game,
//! roll each branch forward with fast scripted AIs, and let the learned value
//! net judge the resulting positions.
use crate::ai::{Ai, BasicAi, Weights};
use crate::evolve::features;
use crate::game::{Action, Game};
use crate::valuenet::ValueNet;

pub struct NeuralAi {
    base: BasicAi,
    net: ValueNet,
    every: u32,   // reconsider war every N turns
    horizon: u32, // rollout depth in game rounds
}

impl NeuralAi {
    pub fn new(mut w: Weights, net: ValueNet) -> NeuralAi {
        w.war_ratio = 99.0; // the scripted layer never declares; the net does
        NeuralAi {
            base: BasicAi::with_weights(w),
            net,
            every: 4,
            horizon: 12,
        }
    }

    /// Win probability after playing `horizon` rounds out with default AIs.
    fn rollout(&self, g: &Game, pid: usize) -> f64 {
        let mut sim = g.clone();
        let mut ais = BasicAi::fleet(&sim);
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
            None => self.net.eval(&features(&sim, pid)),
        }
    }

    fn consider_war(&mut self, g: &mut Game, pid: usize) {
        if g.turn % self.every != 0 || g.player_city_ids(pid).len() < 2 {
            return;
        }
        let others: Vec<usize> = g
            .players
            .iter()
            .filter(|o| o.id != pid && o.alive && !o.is_minor && !o.is_barbarian)
            .map(|o| o.id)
            .collect();
        if others.is_empty() || others.iter().any(|o| g.is_at_war(pid, *o)) {
            return;
        }
        let peace = self.rollout(g, pid);
        let mut best: Option<(f64, usize)> = None;
        for o in others {
            let mut sim = g.clone();
            if sim.apply(pid, &Action::DeclareWar { player: o }).is_err() {
                continue;
            }
            let v = self.rollout(&sim, pid);
            if best.map(|(b, _)| v > b).unwrap_or(true) {
                best = Some((v, o));
            }
        }
        if let Some((v, o)) = best {
            if v > peace + 0.03 {
                let _ = g.apply(pid, &Action::DeclareWar { player: o });
            }
        }
    }
}

impl Ai for NeuralAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        if !g.players[pid].is_minor && !g.players[pid].is_barbarian && g.winner.is_none() {
            self.consider_war(g, pid);
        }
        self.base.take_turn(g, pid);
    }
}
