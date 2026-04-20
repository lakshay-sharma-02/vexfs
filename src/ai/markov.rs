//! Markov prefetcher — predicts what file you'll open next
//! Based on sequences: "after opening A, you open B 80% of the time"
//! Memory cost: ~2-4MB for thousands of files
//! Inference time: single hash lookup — nanoseconds

use std::collections::HashMap;

/// Transition table: after seeing file A, what comes next?
pub struct MarkovPrefetcher {
    // ino -> Vec<(next_ino, next_name, count)>
    pub transitions: HashMap<u64, Vec<(u64, String, u32)>>,
    // total memory used (approximate)
    entry_count: usize,
    max_entries: usize,
}

impl MarkovPrefetcher {
    pub fn new(max_entries: usize) -> Self {
        Self {
            transitions: HashMap::new(),
            entry_count: 0,
            max_entries,
        }
    }

    /// Record that `next_ino` was opened after `prev_ino`
    pub fn record_transition(&mut self, prev_ino: u64, next_ino: u64, next_name: &str) {
        if self.entry_count >= self.max_entries {
            return; // hard memory cap
        }

        let transitions = self.transitions.entry(prev_ino).or_default();

        // Update count if transition already exists
        for (ino, _, count) in transitions.iter_mut() {
            if *ino == next_ino {
                *count += 1;
                return;
            }
        }

        // New transition
        transitions.push((next_ino, next_name.to_string(), 1));
        self.entry_count += 1;
    }

    /// Predict the most likely next file after `ino`
    /// Returns (ino, name, probability)
    pub fn predict(&self, ino: u64) -> Vec<(u64, &str, f32)> {
        let transitions = match self.transitions.get(&ino) {
            Some(t) => t,
            None => return vec![],
        };

        let total: u32 = transitions.iter().map(|(_, _, c)| c).sum();
        if total == 0 { return vec![]; }

        let mut predictions: Vec<(u64, &str, f32)> = transitions
            .iter()
            .map(|(ino, name, count)| {
                (*ino, name.as_str(), *count as f32 / total as f32)
            })
            .collect();

        // Sort by probability descending
        predictions.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        predictions
    }

    /// Top prediction only — what to prefetch
    pub fn top_prediction(&self, ino: u64) -> Option<(u64, &str, f32)> {
        self.predict(ino).into_iter().next()
    }

    pub fn entry_count(&self) -> usize {
        self.entry_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learns_sequence() {
        let mut m = MarkovPrefetcher::new(10000);

        // Simulate: main.rs → lib.rs (3 times)
        m.record_transition(2, 3, "lib.rs");
        m.record_transition(2, 3, "lib.rs");
        m.record_transition(2, 3, "lib.rs");
        // main.rs → mod.rs (1 time)
        m.record_transition(2, 4, "mod.rs");

        let preds = m.predict(2);
        assert_eq!(preds.len(), 2);
        // lib.rs should be top prediction (75%)
        assert_eq!(preds[0].0, 3);
        assert!((preds[0].2 - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_no_prediction_for_unknown() {
        let m = MarkovPrefetcher::new(10000);
        assert!(m.top_prediction(999).is_none());
    }

    #[test]
    fn test_memory_cap() {
        let mut m = MarkovPrefetcher::new(5);
        for i in 0..10 {
            m.record_transition(1, i, "file");
        }
        assert!(m.entry_count() <= 5);
    }
}
