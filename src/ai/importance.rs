//! File importance scorer
//! Scores every file 0.0-1.0 based on access patterns.
//! This score drives: desktop surfacing, storage tiering,
//! prefetch priority, and search result ranking.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Storage tier — where a file lives based on importance
#[derive(Debug, Clone, PartialEq)]
pub enum StorageTier {
    Hot,   // NVMe — accessed constantly
    Warm,  // SSD — accessed regularly  
    Cold,  // HDD — rarely accessed
}

impl StorageTier {
    pub fn label(&self) -> &str {
        match self {
            StorageTier::Hot  => "🔥 HOT",
            StorageTier::Warm => "🌤 WARM",
            StorageTier::Cold => "🧊 COLD",
        }
    }
}

/// Score for a single file
#[derive(Debug, Clone)]
pub struct FileScore {
    pub ino: u64,
    pub name: String,
    pub score: f32,          // 0.0 to 1.0
    pub access_count: u32,
    pub last_access: u64,    // unix seconds
    pub tier: StorageTier,
}

impl FileScore {
    pub fn tier_from_score(score: f32) -> StorageTier {
        if score >= 0.6 { StorageTier::Hot }
        else if score >= 0.3 { StorageTier::Warm }
        else { StorageTier::Cold }
    }
}

/// The importance engine
pub struct ImportanceEngine {
    // ino -> (name, access_count, last_access_secs, total_open_secs)
    pub stats: HashMap<u64, (String, u32, u64, u64)>,
}

impl ImportanceEngine {
    pub fn new() -> Self {
        Self { stats: HashMap::new() }
    }

    /// Record an access to a file
    pub fn record_access(&mut self, ino: u64, name: &str, open_duration_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = self.stats.entry(ino).or_insert((name.to_string(), 0, 0, 0));
        entry.1 += 1;                          // increment access count
        entry.2 = now;                         // update last access
        entry.3 += open_duration_secs;         // accumulate open time
    }

    /// Score a file — combines recency + frequency + open time
    /// Returns 0.0 (cold/unimportant) to 1.0 (hot/critical)
    pub fn score(&self, ino: u64) -> f32 {
        let (_, count, last_access, open_secs) = match self.stats.get(&ino) {
            Some(s) => s,
            None => return 0.0,
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Recency score — decays over time
        // 1.0 if accessed just now, 0.0 if not accessed in 30 days
        let age_secs = now.saturating_sub(*last_access) as f32;
        let recency = (1.0 - (age_secs / (30.0 * 86400.0))).max(0.0);

        // Frequency score — log scale so 100 accesses isn't 100x better than 10
        let frequency = (*count as f32).ln().max(0.0) / 10.0_f32.ln();
        let frequency = frequency.min(1.0);

        // Engagement score — time spent with file matters
        let engagement = (*open_secs as f32 / 3600.0).min(1.0); // cap at 1 hour

        // Weighted combination
        let score = (recency * 0.4) + (frequency * 0.4) + (engagement * 0.2);
        score.min(1.0)
    }

    /// Get scored + ranked list of all files
    /// This is what drives the "desktop" surface
    pub fn ranked_files(&self) -> Vec<FileScore> {
        let mut scores: Vec<FileScore> = self.stats.iter()
            .map(|(ino, (name, count, last_access, _))| {
                let score = self.score(*ino);
                FileScore {
                    ino: *ino,
                    name: name.clone(),
                    score,
                    access_count: *count,
                    last_access: *last_access,
                    tier: FileScore::tier_from_score(score),
                }
            })
            .collect();

        scores.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        scores
    }

    /// What tier should this file be on?
    pub fn tier(&self, ino: u64) -> StorageTier {
        FileScore::tier_from_score(self.score(ino))
    }

    /// Files important enough to surface on "desktop"
    pub fn desktop_files(&self, limit: usize) -> Vec<FileScore> {
        self.ranked_files()
            .into_iter()
            .filter(|f| f.score >= 0.3)
            .take(limit)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_increases_with_access() {
        let mut engine = ImportanceEngine::new();
        engine.record_access(2, "main.rs", 0);
        let score1 = engine.score(2);

        for _ in 0..20 {
            engine.record_access(2, "main.rs", 60);
        }
        let score2 = engine.score(2);

        assert!(score2 > score1);
    }

    #[test]
    fn test_unknown_file_scores_zero() {
        let engine = ImportanceEngine::new();
        assert_eq!(engine.score(999), 0.0);
    }

    #[test]
    fn test_tier_assignment() {
        let mut engine = ImportanceEngine::new();
        // High frequency = hot
        for _ in 0..50 {
            engine.record_access(2, "hot.rs", 120);
        }
        assert_eq!(engine.tier(2), StorageTier::Hot);

        // Never accessed = cold
        assert_eq!(engine.tier(999), StorageTier::Cold);
    }

    #[test]
    fn test_ranked_files_sorted() {
        let mut engine = ImportanceEngine::new();
        engine.record_access(2, "rarely.rs", 0);
        for _ in 0..30 {
            engine.record_access(3, "often.rs", 60);
        }

        let ranked = engine.ranked_files();
        assert_eq!(ranked[0].name, "often.rs");
    }

    #[test]
    fn test_desktop_files() {
        let mut engine = ImportanceEngine::new();
        for _ in 0..20 {
            engine.record_access(2, "important.rs", 30);
        }
        engine.record_access(3, "unimportant.rs", 0);

        let desktop = engine.desktop_files(10);
        assert!(desktop.iter().any(|f| f.name == "important.rs"));
    }
}
