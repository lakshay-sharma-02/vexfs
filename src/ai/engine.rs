use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::collections::HashMap;

use super::markov::MarkovPrefetcher;
use super::neural::NeuralPrefetcher;
use super::importance::ImportanceEngine;
use super::entropy::{EntropyGuard, ThreatLevel};
use super::search::SearchIndex;
use super::logger::{AccessLog, AccessEvent, AccessKind};

pub enum FsEvent {
    Open { ino: u64, name: String, size: u64 },
    Write { ino: u64, name: String, data: Vec<u8> },
    Close { ino: u64, name: String, duration: u64 },
    Delete { ino: u64, name: String },
    SyncCacheSize { used: u64, max: u64 },
    SearchQuery { query: String },
    AskQuery { query: String, file_list: Vec<String> }, // FUSE supplies the filesystem context
    SyncAI, // Trigger a full sync of all AI data for persistence
}

#[derive(Default)]
pub struct SharedAIState {
    pub markov_entries: usize,
    pub neural_vocab: usize,
    pub search_indexed: usize,
    pub entropy_threats: usize,
    pub cache_used: u64,
    pub cache_max: u64,
    pub ranked_files: Vec<(String, f32, String)>, // (name, score, tier label)
    
    // Result caches for virtual files
    pub search_result: Vec<u8>,
    pub ask_result: Vec<u8>,

    // Full AI data for persistence (populated on SyncAI)
    pub markov_data: HashMap<u64, Vec<(u64, String, u32)>>,
    pub importance_data: HashMap<u64, (String, u32, u64, u64)>,
    pub neural_weights: Vec<u8>,
}

pub struct AIEngine {
    pub markov: MarkovPrefetcher,
    pub neural: NeuralPrefetcher,
    pub importance: ImportanceEngine,
    pub entropy_guard: EntropyGuard,
    pub search: SearchIndex,
    pub log: AccessLog,
    
    last_opened_ino: Option<u64>,
    // NEW: accumulates write chunks per inode until file is closed
    write_accumulator: std::collections::HashMap<u64, Vec<u8>>,
}

impl AIEngine {
    pub fn new(
        markov: MarkovPrefetcher,
        neural: NeuralPrefetcher,
        importance: ImportanceEngine,
        entropy_guard: EntropyGuard,
        search: SearchIndex,
        log: AccessLog,
    ) -> Self {
        Self {
            markov,
            neural,
            importance,
            entropy_guard,
            search,
            log,
            last_opened_ino: None,
            write_accumulator: std::collections::HashMap::new(),
        }
    }

    /// Spawns the background AI processing loop
    pub fn spawn(mut self) -> (Sender<FsEvent>, Arc<RwLock<SharedAIState>>) {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(RwLock::new(SharedAIState::default()));
        let state_clone = Arc::clone(&state);

        thread::spawn(move || {
            self.run_loop(rx, state_clone);
        });

        (tx, state)
    }

    fn run_loop(&mut self, rx: Receiver<FsEvent>, state: Arc<RwLock<SharedAIState>>) {
        for event_wrapper in rx {
            self.handle_event(event_wrapper);
            self.sync_state(&state);
        }
    }

    fn handle_event(&mut self, event: FsEvent) {
        match event {
            FsEvent::Open { ino, name, size } => {
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Open, size));

                if let Some(prev) = self.last_opened_ino {
                    if prev != ino {
                        self.markov.record_transition(prev, ino, &name);
                    }
                }
                self.last_opened_ino = Some(ino);
                self.importance.record_access(ino, &name, 0);

                // Neural and Markov predictions
                self.neural.record_access(ino, &name);
                if let Some((pred_ino, pred_name, conf)) = self.neural.top_prediction() {
                    let tier = self.importance.tier(pred_ino);
                    println!(
                        "VexFS Neural: '{}' → predicting '{}' next ({:.0}%) [{}]",
                        name, pred_name, conf * 100.0, tier.label()
                    );
                } else if let Some((pred_ino, pred_name, prob)) = self.markov.top_prediction(ino) {
                    let tier = self.importance.tier(pred_ino);
                    println!(
                        "VexFS Markov: '{}' → predicting '{}' next ({:.0}%) [{}]",
                        name, pred_name, prob * 100.0, tier.label()
                    );
                }
                
                let tier = self.importance.tier(ino);
                let score = self.importance.score(ino);
                println!("VexFS AI: '{}' score={:.2} [{}]", name, score, tier.label());
            }

            FsEvent::Write { ino, name, data } => {
                let bytes_len = data.len();

                // --- Entropy / ransomware check ---
                if let Some(threat) = self.entropy_guard.check_write(ino, &name, &data) {
                    let h = crate::ai::entropy::shannon_entropy(&data);
                    println!("\n{} VexFS EntropyGuard: '{}' (ino={}) entropy={:.2}",
                        threat.label(), name, ino, h);
                    match threat {
                        ThreatLevel::Critical => {
                            println!("  ↳ File was plaintext, now receiving encrypted data!");
                            println!("  ↳ Possible ransomware encryption in progress.");
                        }
                        ThreatLevel::Pattern => {
                            println!("  ↳ Repeated high-entropy writes detected in 60s window.");
                        }
                        ThreatLevel::Extension => {
                            println!("  ↳ Suspicious file extension detected.");
                        }
                        ThreatLevel::Warning => {
                            println!("  ↳ High-entropy write — may be compressed or encrypted data.");
                        }
                    }
                }

                // Accumulate write chunks per inode so TF-IDF sees full content
                let full_content = {
                    let acc = self.write_accumulator.entry(ino).or_default();
                    acc.extend_from_slice(&data);
                    acc.clone()
                };

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.search.index(ino, &name, &full_content, now);
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Write, bytes_len as u64));
            }

            FsEvent::Close { ino, name, duration } => {
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Close, 0));
                self.importance.record_access(ino, &name, duration);
                // Clear write accumulator — file is closed, index is up to date
                self.write_accumulator.remove(&ino);
            }

            FsEvent::Delete { ino, name } => {
                self.search.remove(ino);
                self.write_accumulator.remove(&ino);
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Delete, 0));
            }
            
            FsEvent::SyncCacheSize { .. } => {
                // Handled in sync_state implicitly or we could store cache metrics inside AIEngine
                // We'll let the FUSE main thread pass cache size into the event to sync it in the AI thread. 
                // But honestly, the dashboard reads cache size from FUSE... Wait. 
                // We'll update the `AIState` cache size in `sync_state`.
            }

            FsEvent::SearchQuery { query } => {
                let results = self.search.search(&query);
                let mut out = format!("VexFS Search: \"{}\" -- {} result(s)\n{}\n", query, results.len(), "-".repeat(48));
                if results.is_empty() {
                    out.push_str("  No results found.\n");
                } else {
                    for (i, r) in results.iter().enumerate() {
                        out.push_str(&format!("  {}. {} (score: {:.3})\n", i + 1, r.name, r.score));
                        if !r.matched_terms.is_empty() {
                            out.push_str(&format!("     terms: {}\n", r.matched_terms.join(", ")));
                        }
                    }
                }
                println!("VexFS Search: query='{}' → {} results", query, results.len());
                // We'll stash this in a dedicated internal field and sync it.
                self.search.last_query_result = out.into_bytes();
            }

            FsEvent::AskQuery { query, file_list } => {
                self.run_ask_query(&query, file_list);
            }
            FsEvent::SyncAI => {
                // sync_state is called after every event anyway, 
                // but this event ensures we hit the full data sync logic.
            }
        }
    }

    fn run_ask_query(&mut self, question: &str, _file_list: Vec<String>) {
        // Lightweight TF-IDF semantic search
        let results = self.search.search(question);
        let neural_hint = self.neural.top_prediction()
            .map(|(_, name, conf)| format!("Neural prefetcher predicts '{}' is next (confidence: {:.0}%)", name, conf * 100.0))
            .unwrap_or_default();

        let mut out = format!("[VexFS Ask — Semantic Search Fallback]\n\nQ: {}\n\n", question);
        if results.is_empty() {
            out.push_str("No relevant files found in the filesystem for that query.\n");
        } else {
            out.push_str("Based on your filesystem, the most relevant files are:\n\n");
            for (i, r) in results.iter().take(5).enumerate() {
                out.push_str(&format!("  {}. {} (relevance: {:.1}%)", i + 1, r.name, r.score * 100.0));
                if !r.matched_terms.is_empty() {
                    out.push_str(&format!(" — keywords: {}", r.matched_terms.join(", ")));
                }
                out.push('\n');
            }
        }

        if !neural_hint.is_empty() {
            out.push_str(&format!("\n💡 {}\n", neural_hint));
        }

        println!("VexFS Ask: answered via TF-IDF fallback ({} results)", results.len());
        self.search.last_ask_result = out.into_bytes();
    }

    fn sync_state(&self, state_lock: &Arc<RwLock<SharedAIState>>) {
        let mut w = state_lock.write().unwrap();
        w.markov_entries = self.markov.entry_count();
        w.neural_vocab = self.neural.vocab_size();
        w.search_indexed = self.search.indexed_count();
        w.entropy_threats = self.entropy_guard.threat_count as usize;
        
        let ranked = self.importance.ranked_files()
            .into_iter()
            .take(10)
            .map(|f| (f.name, f.score, f.tier.label().to_string()))
            .collect();
        w.ranked_files = ranked;

        // Since SearchIndex is borrowed mutably during operations, we stash results there or in engine
        w.search_result = self.search.last_query_result.clone();
        w.ask_result = self.search.last_ask_result.clone();

        // Populate full data for persistence
        w.markov_data = self.markov.transitions.clone();
        w.importance_data = self.importance.stats.clone();
        w.neural_weights = self.neural.to_bytes();
    }
}
