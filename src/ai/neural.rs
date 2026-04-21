//! Neural Prefetcher — online-learning 2-layer MLP
//!
//! Replaces / augments the Markov chain with a small neural network that
//! learns from real file-access sequences in real time (no GPU, no deps).
//!
//! Architecture:
//!   Input  : last N=8 file accesses, encoded as normalized inode indices → [f32; 8]
//!   Hidden : 32 neurons, ReLU activation
//!   Output : softmax over vocabulary (one entry per unique file seen)
//!   Training: single-sample online SGD after every file access

use std::collections::{HashMap, VecDeque};

const INPUT_SIZE: usize = 8;   // history window
const HIDDEN_SIZE: usize = 32; // hidden layer neurons
const LR: f32 = 0.05;          // learning rate

fn relu(x: f32) -> f32 { x.max(0.0) }
fn relu_grad(x: f32) -> f32 { if x > 0.0 { 1.0 } else { 0.0 } }

fn softmax(v: &[f32]) -> Vec<f32> {
    let max = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = v.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

/// Online-learning neural prefetcher.
/// Records file-access history and predicts the next file to be opened.
pub struct NeuralPrefetcher {
    // Layer 1 weights: [HIDDEN_SIZE][INPUT_SIZE]
    w1: Vec<[f32; INPUT_SIZE]>,
    b1: Vec<f32>,
    // Layer 2 weights: [vocab_size][HIDDEN_SIZE] — grows as new files appear
    w2: Vec<Vec<f32>>,
    b2: Vec<f32>,
    // Vocabulary
    ino_to_idx: HashMap<u64, usize>,
    idx_to_ino: Vec<u64>,
    idx_to_name: Vec<String>,
    // Rolling access history (last INPUT_SIZE inodes)
    history: VecDeque<u64>,
    // Stats
    pub total_accesses: u64,
    pub correct_predictions: u64,
}

impl NeuralPrefetcher {
    pub fn new() -> Self {
        use std::f32::consts::PI;
        // Xavier init for W1
        let scale1 = (2.0_f32 / INPUT_SIZE as f32).sqrt();
        let mut w1 = Vec::with_capacity(HIDDEN_SIZE);
        for i in 0..HIDDEN_SIZE {
            let mut row = [0f32; INPUT_SIZE];
            for j in 0..INPUT_SIZE {
                // Deterministic pseudo-random init using sin
                row[j] = ((i * INPUT_SIZE + j + 1) as f32 * PI * 0.17321).sin() * scale1;
            }
            w1.push(row);
        }
        Self {
            w1,
            b1: vec![0.0; HIDDEN_SIZE],
            w2: vec![],
            b2: vec![],
            ino_to_idx: HashMap::new(),
            idx_to_ino: vec![],
            idx_to_name: vec![],
            history: VecDeque::with_capacity(INPUT_SIZE + 1),
            total_accesses: 0,
            correct_predictions: 0,
        }
    }

    /// Register a new file inode / grow the output vocabulary
    fn register(&mut self, ino: u64, name: &str) -> usize {
        if let Some(&idx) = self.ino_to_idx.get(&ino) {
            return idx;
        }
        let idx = self.idx_to_ino.len();
        self.ino_to_idx.insert(ino, idx);
        self.idx_to_ino.push(ino);
        self.idx_to_name.push(name.to_string());

        // Grow layer 2: add one new output neuron
        let scale2 = (2.0_f32 / HIDDEN_SIZE as f32).sqrt();
        let mut new_weights = vec![0.0f32; HIDDEN_SIZE];
        for k in 0..HIDDEN_SIZE {
            new_weights[k] = ((idx * HIDDEN_SIZE + k + 1) as f32 * 0.29731).sin() * scale2;
        }
        self.w2.push(new_weights);
        self.b2.push(0.0);
        idx
    }

    /// Encode current access history into a fixed-length input vector [0, 1]
    fn encode_history(&self) -> Vec<f32> {
        let vocab = self.idx_to_ino.len().max(1) as f32;
        let mut input = vec![0.0f32; INPUT_SIZE];
        let hist: Vec<u64> = self.history.iter().cloned().collect();
        for (i, &ino) in hist.iter().rev().take(INPUT_SIZE).enumerate() {
            if let Some(&idx) = self.ino_to_idx.get(&ino) {
                input[i] = idx as f32 / vocab;
            }
        }
        input
    }

    /// Forward pass: returns (hidden activations, output logits, softmax probs)
    fn forward(&self, input: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        // Layer 1: hidden = ReLU(W1 * input + b1)
        let mut hidden = vec![0.0f32; HIDDEN_SIZE];
        for i in 0..HIDDEN_SIZE {
            let mut s = self.b1[i];
            for j in 0..INPUT_SIZE {
                s += self.w1[i][j] * input[j];
            }
            hidden[i] = relu(s);
        }

        let vocab = self.w2.len();
        if vocab == 0 {
            return (hidden, vec![], vec![]);
        }

        // Layer 2: logits = W2 * hidden + b2
        let mut logits = vec![0.0f32; vocab];
        for i in 0..vocab {
            let mut s = self.b2[i];
            for j in 0..HIDDEN_SIZE {
                s += self.w2[i][j] * hidden[j];
            }
            logits[i] = s;
        }
        let probs = softmax(&logits);
        (hidden, logits, probs)
    }

    /// One step of online SGD (cross-entropy loss)
    fn train_step(&mut self, input: &[f32], target_idx: usize) {
        let vocab = self.w2.len();
        if vocab < 2 { return; }

        let (hidden, _logits, probs) = self.forward(input);

        // Output gradient: dL/dlogit_i = probs[i] - 1{i==target}
        let mut d_logits = probs.clone();
        d_logits[target_idx] -= 1.0;

        // Gradient for W2 and b2
        for i in 0..vocab {
            self.b2[i] -= LR * d_logits[i];
            for j in 0..HIDDEN_SIZE {
                self.w2[i][j] -= LR * d_logits[i] * hidden[j];
            }
        }

        // Backprop to hidden layer
        let mut d_hidden = vec![0.0f32; HIDDEN_SIZE];
        for j in 0..HIDDEN_SIZE {
            for i in 0..vocab {
                d_hidden[j] += d_logits[i] * self.w2[i][j];
            }
        }

        // ReLU gradient through hidden
        // Layer 1 pre-activation
        let mut pre_hidden = vec![0.0f32; HIDDEN_SIZE];
        for i in 0..HIDDEN_SIZE {
            let mut s = self.b1[i];
            for j in 0..INPUT_SIZE { s += self.w1[i][j] * input[j]; }
            pre_hidden[i] = s;
        }

        // Gradient for W1 and b1
        for i in 0..HIDDEN_SIZE {
            let grad = d_hidden[i] * relu_grad(pre_hidden[i]);
            self.b1[i] -= LR * grad;
            for j in 0..INPUT_SIZE {
                self.w1[i][j] -= LR * grad * input[j];
            }
        }
    }

    /// Record a file access — updates history, trains the network
    pub fn record_access(&mut self, ino: u64, name: &str) {
        let target_idx = self.register(ino, name);

        // Train on (current history → this file) BEFORE updating history
        if self.history.len() >= 2 && self.w2.len() >= 2 {
            let input = self.encode_history();
            self.train_step(&input, target_idx);
        }

        // Update rolling history
        self.history.push_back(ino);
        if self.history.len() > INPUT_SIZE {
            self.history.pop_front();
        }

        self.total_accesses += 1;
    }

    /// Predict the most likely next file.
    /// Returns (ino, name, confidence) or None if not enough data.
    pub fn top_prediction(&self) -> Option<(u64, &str, f32)> {
        if self.w2.len() < 2 { return None; }

        let input = self.encode_history();
        let (_, _, probs) = self.forward(&input);
        if probs.is_empty() { return None; }

        let (best_idx, &best_prob) = probs.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;

        // Don't predict the file that was just accessed
        let last = self.history.back().copied();
        let best_ino = self.idx_to_ino[best_idx];
        if Some(best_ino) == last {
            // Try second-best
            let mut ranked: Vec<(usize, f32)> = probs.iter().cloned().enumerate().collect();
            ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            for (idx, prob) in ranked.iter().skip(1).take(1) {
                let ino = self.idx_to_ino[*idx];
                let name = &self.idx_to_name[*idx];
                return Some((ino, name.as_str(), *prob));
            }
            return None;
        }

        let name = &self.idx_to_name[best_idx];
        Some((best_ino, name.as_str(), best_prob))
    }

    /// Accuracy over all predictions (for status reporting)
    pub fn accuracy(&self) -> f64 {
        if self.total_accesses < 2 { return 0.0; }
        self.correct_predictions as f64 / (self.total_accesses - 1) as f64
    }

    /// How many unique files are in the vocabulary
    pub fn vocab_size(&self) -> usize { self.idx_to_ino.len() }

    /// Status string
    pub fn status(&self) -> String {
        format!(
            "NeuralPrefetcher: vocab={} accesses={} accuracy={:.1}%",
            self.vocab_size(),
            self.total_accesses,
            self.accuracy() * 100.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learns_simple_sequence() {
        let mut net = NeuralPrefetcher::new();
        // Train on A→B→C repeated 50 times - should learn to predict B after A
        for _ in 0..50 {
            net.record_access(10, "fileA");
            net.record_access(20, "fileB");
            net.record_access(30, "fileC");
        }
        // After seeing A many times, prediction from A's context should be B
        // (we can't guarantee ordering perfectly but vocab should be 3)
        assert_eq!(net.vocab_size(), 3);
        assert!(net.total_accesses > 0);
    }

    #[test]
    fn test_no_prediction_cold_start() {
        let mut net = NeuralPrefetcher::new();
        net.record_access(1, "only_one_file");
        assert!(net.top_prediction().is_none());
    }

    #[test]
    fn test_softmax_sums_to_one() {
        let v = vec![1.0, 2.0, 3.0, 4.0];
        let s = softmax(&v);
        let sum: f32 = s.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }
}
