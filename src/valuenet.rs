//! Learned position evaluator: a small MLP (25→64→32→1) trained offline on
//! self-play outcomes from evolved/dataset.csv (NNUE-style distillation).
//! Input = evolve::features(); output = win probability for that player.
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct ValueNet {
    pub sizes: Vec<usize>,
    pub weights: Vec<Vec<Vec<f64>>>, // [layer][in][out]
    pub biases: Vec<Vec<f64>>,
}

impl ValueNet {
    pub fn load(dir: &str) -> Option<ValueNet> {
        let raw = fs::read_to_string(Path::new(dir).join("valuenet.json")).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Win probability for a position (features from `evolve::features`).
    pub fn eval(&self, x: &[f32]) -> f64 {
        let mut a: Vec<f64> = x.iter().map(|v| *v as f64).collect();
        let last = self.weights.len() - 1;
        for l in 0..=last {
            let (w, b) = (&self.weights[l], &self.biases[l]);
            let mut next = b.clone();
            for (i, ai) in a.iter().enumerate() {
                for (j, nj) in next.iter_mut().enumerate() {
                    *nj += ai * w[i][j];
                }
            }
            for v in next.iter_mut() {
                *v = if l < last {
                    v.max(0.0)
                } else {
                    1.0 / (1.0 + (-*v).exp())
                };
            }
            a = next;
        }
        a[0]
    }
}

#[cfg(test)]
mod tests {
    use super::ValueNet;

    /// Parity with the Python trainer on a saved fixture. Skips when no net
    /// has been trained yet (evolved/ artifacts are not in the repo).
    #[test]
    fn matches_training_fixture() {
        let net = match ValueNet::load("evolved") {
            Some(n) => n,
            None => return,
        };
        #[derive(serde::Deserialize)]
        struct Fix {
            input: Vec<f32>,
            output: f64,
        }
        let raw = match std::fs::read_to_string("evolved/valuenet_fixture.json") {
            Ok(r) => r,
            Err(_) => return,
        };
        let fix: Fix = serde_json::from_str(&raw).unwrap();
        let got = net.eval(&fix.input);
        assert!(
            (got - fix.output).abs() < 1e-4,
            "rust {got} vs python {}",
            fix.output
        );
    }
}
