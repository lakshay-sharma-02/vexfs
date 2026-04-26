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
use super::memory::MemoryEngine;

pub enum FsEvent {
    Open   { ino: u64, name: String, size: u64 },
    Write  { ino: u64, name: String, data: Vec<u8> },
    Close  { ino: u64, name: String, duration: u64 },
    Delete { ino: u64, name: String },
    SyncCacheSize { used: u64, max: u64 },
    SearchQuery { query: String },
    AskQuery    { query: String, file_list: Vec<String> },
    SyncAI,         // trigger full sync for persistence
    EndSession,     // called on unmount — closes and archives current session
}

#[derive(Default)]
pub struct SharedAIState {
    pub markov_entries:  usize,
    pub neural_vocab:    usize,
    pub search_indexed:  usize,
    pub entropy_threats: usize,
    pub cache_used:      u64,
    pub cache_max:       u64,
    pub ranked_files:    Vec<(String, f32, String)>, // (name, score, tier label)

    // Result caches for virtual files
    pub search_result:  Vec<u8>,
    pub ask_result:     Vec<u8>,
    /// Live context summary for .vexfs-context virtual file
    pub context_result: Vec<u8>,

    // Full AI data for persistence (populated on SyncAI / EndSession)
    pub markov_data:     HashMap<u64, Vec<(u64, String, u32)>>,
    pub importance_data: HashMap<u64, (String, u32, u64, u64)>,
    pub neural_weights:  Vec<u8>,
    /// Serialized MemoryEngine bytes for persistence
    pub memory_bytes:    Vec<u8>,

    // Memory stats for dashboard
    pub memory_total_sessions:  u64,
    pub memory_tracked_files:   usize,
    pub memory_active_streaks:  usize,
    pub memory_trending_count:  usize,
    pub memory_co_access_pairs: usize,
}

pub struct AIEngine {
    pub markov:        MarkovPrefetcher,
    pub neural:        NeuralPrefetcher,
    pub importance:    ImportanceEngine,
    pub entropy_guard: EntropyGuard,
    pub search:        SearchIndex,
    pub log:           AccessLog,
    pub memory:        MemoryEngine,

    last_opened_ino:   Option<u64>,
    write_accumulator: HashMap<u64, Vec<u8>>,
}

impl AIEngine {
    pub fn new(
        markov:        MarkovPrefetcher,
        neural:        NeuralPrefetcher,
        importance:    ImportanceEngine,
        entropy_guard: EntropyGuard,
        search:        SearchIndex,
        log:           AccessLog,
        memory:        MemoryEngine,
    ) -> Self {
        Self {
            markov, neural, importance, entropy_guard,
            search, log, memory,
            last_opened_ino: None,
            write_accumulator: HashMap::new(),
        }
    }

    pub fn spawn(mut self) -> (Sender<FsEvent>, Arc<RwLock<SharedAIState>>) {
        let (tx, rx) = mpsc::channel();
        let state    = Arc::new(RwLock::new(SharedAIState::default()));
        let state_c  = Arc::clone(&state);

        thread::spawn(move || {
            self.run_loop(rx, state_c);
        });

        (tx, state)
    }

    fn run_loop(&mut self, rx: Receiver<FsEvent>, state: Arc<RwLock<SharedAIState>>) {
        for event in rx {
            self.handle_event(event);
            self.sync_state(&state);
        }
    }

    fn handle_event(&mut self, event: FsEvent) {
        match event {
            // ── Open ─────────────────────────────────────────────────────
            FsEvent::Open { ino, name, size } => {
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Open, size));

                if let Some(prev) = self.last_opened_ino {
                    if prev != ino {
                        self.markov.record_transition(prev, ino, &name);
                    }
                }
                self.last_opened_ino = Some(ino);
                self.importance.record_access(ino, &name, 0);
                self.neural.record_access(ino, &name);

                // ── Memory: record this access ──────────────────────────
                self.memory.record_access(ino, &name);
                self.memory.record_read(ino);

                // Prefetch predictions
                if let Some((pred_ino, pred_name, conf)) = self.neural.top_prediction() {
                    let tier   = self.importance.tier(pred_ino);
                    let streak = self.memory.streak(pred_ino);
                    let streak_str = if streak >= 2 {
                        format!(" 🔥{}d", streak)
                    } else {
                        String::new()
                    };
                    println!(
                        "VexFS Neural: '{}' → predicting '{}' next ({:.0}%) [{}]{}",
                        name, pred_name, conf * 100.0, tier.label(), streak_str
                    );
                } else if let Some((pred_ino, pred_name, prob)) = self.markov.top_prediction(ino) {
                    let tier = self.importance.tier(pred_ino);
                    println!(
                        "VexFS Markov: '{}' → predicting '{}' next ({:.0}%) [{}]",
                        name, pred_name, prob * 100.0, tier.label()
                    );
                }

                // Co-access hint
                let cofiles = self.memory.top_cofiles(ino, 2);
                if !cofiles.is_empty() {
                    println!("VexFS Memory: '{}' usually opened with: {}",
                        name, cofiles.join(", "));
                }

                let tier  = self.importance.tier(ino);
                let score = self.importance.score(ino);
                let trend = self.memory.trends.trend(ino);
                println!("VexFS AI: '{}' score={:.2} [{}] {}",
                    name, score, tier.label(), trend.label());
            }

            // ── Write ─────────────────────────────────────────────────────
            FsEvent::Write { ino, name, data } => {
                let bytes_len = data.len();

                // Entropy / ransomware check
                if let Some(threat) = self.entropy_guard.check_write(ino, &name, &data) {
                    let h = crate::ai::entropy::shannon_entropy(&data);
                    println!("\n{} VexFS EntropyGuard: '{}' (ino={}) entropy={:.2}",
                        threat.label(), name, ino, h);
                    match threat {
                        ThreatLevel::Critical  => {
                            println!("  ↳ File was plaintext, now receiving encrypted data!");
                            println!("  ↳ Possible ransomware encryption in progress.");
                        }
                        ThreatLevel::Pattern   => {
                            println!("  ↳ Repeated high-entropy writes in 60s window.");
                        }
                        ThreatLevel::Extension => {
                            println!("  ↳ Suspicious file extension detected.");
                        }
                        ThreatLevel::Warning   => {
                            println!("  ↳ High-entropy write — may be compressed/encrypted.");
                        }
                    }
                }

                // Accumulate write chunks per inode until Close
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

                // Memory: count the write
                self.memory.record_write(ino);
            }

            // ── Close ─────────────────────────────────────────────────────
            FsEvent::Close { ino, name, duration } => {
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Close, 0));
                self.importance.record_access(ino, &name, duration);
                self.write_accumulator.remove(&ino);
            }

            // ── Delete ────────────────────────────────────────────────────
            FsEvent::Delete { ino, name } => {
                self.search.remove(ino);
                self.write_accumulator.remove(&ino);
                self.log.record(AccessEvent::now(ino, &name, AccessKind::Delete, 0));
            }

            // ── Search ────────────────────────────────────────────────────
            FsEvent::SearchQuery { query } => {
                let results = self.search.search(&query);
                let mut out = format!(
                    "VexFS Search: \"{}\" -- {} result(s)\n{}\n",
                    query, results.len(), "-".repeat(48)
                );
                if results.is_empty() {
                    out.push_str("  No results found.\n");
                } else {
                    for (i, r) in results.iter().enumerate() {
                        out.push_str(&format!(
                            "  {}. {} (score: {:.3})\n", i + 1, r.name, r.score
                        ));
                        if !r.matched_terms.is_empty() {
                            out.push_str(&format!(
                                "     terms: {}\n", r.matched_terms.join(", ")
                            ));
                        }
                    }
                }
                println!("VexFS Search: query='{}' → {} results", query, results.len());
                self.search.last_query_result = out.into_bytes();
            }

            // ── Ask ───────────────────────────────────────────────────────
            FsEvent::AskQuery { query, file_list } => {
                self.run_ask_query(&query, file_list);
            }

            // ── End Session ───────────────────────────────────────────────
            FsEvent::EndSession => {
                self.memory.close_session();
                let stats = self.memory.stats();
                println!(
                    "VexFS Memory: session closed. Total: {} sessions, {} files tracked",
                    stats.total_sessions, stats.tracked_files
                );
            }

            // ── SyncAI / SyncCacheSize ────────────────────────────────────
            FsEvent::SyncAI | FsEvent::SyncCacheSize { .. } => {
                // sync_state called after every event handles these
            }
        }
    }

    fn run_ask_query(&mut self, question: &str, _file_list: Vec<String>) {
        let results     = self.search.search(question);
        let neural_hint = self.neural.top_prediction()
            .map(|(_, name, conf)| format!(
                "Neural prefetcher predicts '{}' is next (confidence: {:.0}%)",
                name, conf * 100.0
            ))
            .unwrap_or_default();

        // Enrich answer with memory context
        let memory_context = {
            let trending = self.memory.trends.trending_files();
            if !trending.is_empty() {
                let names: Vec<String> = trending.iter().take(2)
                    .filter_map(|(ino, _, _)| self.memory.names.get(ino).cloned())
                    .collect();
                if !names.is_empty() {
                    format!("\n📈 Trending this week: {}", names.join(", "))
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };

        let mut out = format!("[VexFS Ask — Semantic Search]\n\nQ: {}\n\n", question);
        if results.is_empty() {
            out.push_str("No relevant files found for that query.\n");
        } else {
            out.push_str("Most relevant files:\n\n");
            for (i, r) in results.iter().take(5).enumerate() {
                out.push_str(&format!(
                    "  {}. {} (relevance: {:.1}%)",
                    i + 1, r.name, r.score * 100.0
                ));
                if !r.matched_terms.is_empty() {
                    out.push_str(&format!(" — keywords: {}", r.matched_terms.join(", ")));
                }
                out.push('\n');
            }
        }

        if !neural_hint.is_empty()    { out.push_str(&format!("\n💡 {}\n", neural_hint)); }
        if !memory_context.is_empty() { out.push_str(&memory_context); out.push('\n'); }

        println!("VexFS Ask: answered ({} results)", results.len());
        self.search.last_ask_result = out.into_bytes();
    }

    fn sync_state(&self, state_lock: &Arc<RwLock<SharedAIState>>) {
        let mut w = state_lock.write().unwrap();

        // Standard AI state
        w.markov_entries  = self.markov.entry_count();
        w.neural_vocab    = self.neural.vocab_size();
        w.search_indexed  = self.search.indexed_count();
        w.entropy_threats = self.entropy_guard.threat_count as usize;

        w.ranked_files = self.importance.ranked_files()
            .into_iter()
            .take(10)
            .map(|f| (f.name, f.score, f.tier.label().to_string()))
            .collect();

        w.search_result = self.search.last_query_result.clone();
        w.ask_result    = self.search.last_ask_result.clone();

        // Persistence data
        w.markov_data     = self.markov.transitions.clone();
        w.importance_data = self.importance.stats.clone();
        w.neural_weights  = self.neural.to_bytes();
        w.memory_bytes    = self.memory.to_bytes();

        // Memory stats for dashboard
        let stats = self.memory.stats();
        w.memory_total_sessions  = stats.total_sessions;
        w.memory_tracked_files   = stats.tracked_files;
        w.memory_active_streaks  = stats.active_streaks;
        w.memory_trending_count  = stats.trending_count;
        w.memory_co_access_pairs = stats.co_access_pairs;

        // Context summary for .vexfs-context virtual file
        w.context_result = self.memory
            .context_summary(&self.memory.names.clone())
            .into_bytes();
    }
}
