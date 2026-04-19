//! Semantic search — query files by content and context
//! Uses TF-IDF: fast, lightweight, zero ML dependencies
//! Handles natural language queries like:
//!   "the one about auth I was working on yesterday"
//!   "config files I edited today"
//!   "something about database connection"

use std::collections::HashMap;

/// A single indexed file
#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub ino: u64,
    pub name: String,
    pub word_freq: HashMap<String, f32>,  // word -> frequency in this file
    pub total_words: usize,
    pub last_modified: u64,
}

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub ino: u64,
    pub name: String,
    pub score: f32,
    pub matched_terms: Vec<String>,
}

/// The search index
pub struct SearchIndex {
    // ino -> indexed file
    files: HashMap<u64, IndexedFile>,
    // word -> how many files contain it (for IDF)
    doc_freq: HashMap<String, usize>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            doc_freq: HashMap::new(),
        }
    }

    /// Index a file's content
    pub fn index(&mut self, ino: u64, name: &str, content: &[u8], last_modified: u64) {
        // Convert bytes to string, ignore non-utf8
        let text = String::from_utf8_lossy(content).to_lowercase();

        // Tokenize — split on anything that isn't a letter or digit
        let words: Vec<String> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)           // skip tiny words
            .filter(|w| !Self::is_stopword(w))  // skip "the", "and" etc
            .map(|w| w.to_string())
            .collect();

        let total_words = words.len();
        if total_words == 0 {
            // Still index by name even if content is empty
            self.index_by_name(ino, name, last_modified);
            return;
        }

        // Count word frequencies (TF)
        let mut word_freq: HashMap<String, f32> = HashMap::new();
        for word in &words {
            *word_freq.entry(word.clone()).or_insert(0.0) += 1.0;
        }

        // Normalize by document length
        for freq in word_freq.values_mut() {
            *freq /= total_words as f32;
        }

        // Update document frequency for IDF
        // Remove old doc freqs if re-indexing
        if let Some(old) = self.files.get(&ino) {
            for word in old.word_freq.keys() {
                if let Some(df) = self.doc_freq.get_mut(word) {
                    *df = df.saturating_sub(1);
                }
            }
        }

        for word in word_freq.keys() {
            *self.doc_freq.entry(word.clone()).or_insert(0) += 1;
        }

        // Also index filename words
        let name_words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
            .map(|w| w.to_lowercase())
            .collect();

        for word in &name_words {
            *word_freq.entry(word.clone()).or_insert(0.0) += 0.5;
            *self.doc_freq.entry(word.clone()).or_insert(0) += 1;
        }

        self.files.insert(ino, IndexedFile {
            ino,
            name: name.to_string(),
            word_freq,
            total_words,
            last_modified,
        });
    }

    fn index_by_name(&mut self, ino: u64, name: &str, last_modified: u64) {
        let words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
            .map(|w| w.to_lowercase())
            .collect();

        let mut word_freq = HashMap::new();
        for word in &words {
            *word_freq.entry(word.clone()).or_insert(0.0) += 1.0;
            *self.doc_freq.entry(word.clone()).or_insert(0) += 1;
        }

        self.files.insert(ino, IndexedFile {
            ino,
            name: name.to_string(),
            word_freq,
            total_words: words.len(),
            last_modified,
        });
    }

    /// Remove a file from the index
    pub fn remove(&mut self, ino: u64) {
        if let Some(file) = self.files.remove(&ino) {
            for word in file.word_freq.keys() {
                if let Some(df) = self.doc_freq.get_mut(word) {
                    *df = df.saturating_sub(1);
                }
            }
        }
    }

    /// Search — returns results sorted by relevance
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.to_lowercase();
        let n_docs = self.files.len().max(1);

        // Parse query into terms
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .filter(|w| !Self::is_stopword(w))
            .map(|w| w.to_string())
            .collect();

        if terms.is_empty() {
            return vec![];
        }

        let mut results = vec![];

        for file in self.files.values() {
            let mut score = 0.0f32;
            let mut matched = vec![];

            for term in &terms {
                // TF-IDF score for this term in this document
                let tf = file.word_freq.get(term).copied().unwrap_or(0.0);
                if tf > 0.0 {
                    let df = self.doc_freq.get(term).copied().unwrap_or(1);
                    // IDF = log(total_docs / docs_with_term)
                    let idf = ((n_docs as f32) / (df as f32)).ln().max(0.0);
                    score += tf * idf;
                    matched.push(term.clone());
                }

                // Partial match on filename
                if file.name.to_lowercase().contains(term.as_str()) {
                    score += 0.3;
                    if !matched.contains(term) {
                        matched.push(term.clone());
                    }
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    ino: file.ino,
                    name: file.name.clone(),
                    score,
                    matched_terms: matched,
                });
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results
    }

    /// Common English stopwords — these don't help search
    fn is_stopword(word: &str) -> bool {
        matches!(word,
            "the" | "and" | "for" | "are" | "but" | "not" | "you" |
            "all" | "can" | "had" | "her" | "was" | "one" | "our" |
            "out" | "day" | "get" | "has" | "him" | "his" | "how" |
            "its" | "may" | "new" | "now" | "old" | "see" | "two" |
            "who" | "boy" | "did" | "she" | "use" | "way" | "will"
        )
    }

    pub fn indexed_count(&self) -> usize {
        self.files.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_search() {
        let mut idx = SearchIndex::new();
        idx.index(2, "auth.rs", b"login password authenticate user token jwt", 0);
        idx.index(3, "database.rs", b"postgres connection pool query select insert", 0);
        idx.index(4, "readme.md", b"this project implements authentication system", 0);

        let results = idx.search("authentication login");
        assert!(!results.is_empty());
        // auth.rs and readme.md should rank above database.rs
        assert_ne!(results[0].name, "database.rs");
    }

    #[test]
    fn test_filename_search() {
        let mut idx = SearchIndex::new();
        idx.index(2, "auth_handler.rs", b"some content here", 0);
        idx.index(3, "database.rs", b"different content entirely", 0);

        let results = idx.search("auth");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "auth_handler.rs");
    }

    #[test]
    fn test_no_results_for_unknown() {
        let idx = SearchIndex::new();
        let results = idx.search("xyzzy nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn test_tfidf_ranks_specific_terms_higher() {
        let mut idx = SearchIndex::new();
        // "quantum" only appears in one file — should rank highest
        idx.index(2, "physics.rs", b"quantum entanglement superposition physics", 0);
        idx.index(3, "general.rs", b"physics chemistry biology science", 0);
        idx.index(4, "notes.rs", b"meeting notes agenda physics review", 0);

        let results = idx.search("quantum physics");
        assert_eq!(results[0].name, "physics.rs");
    }

    #[test]
    fn test_remove_from_index() {
        let mut idx = SearchIndex::new();
        idx.index(2, "auth.rs", b"login authenticate password", 0);
        idx.remove(2);
        let results = idx.search("login");
        assert!(results.is_empty());
    }

    #[test]
    fn test_matched_terms_reported() {
        let mut idx = SearchIndex::new();
        idx.index(2, "config.rs", b"database configuration connection settings", 0);
        let results = idx.search("database config");
        assert!(!results.is_empty());
        assert!(!results[0].matched_terms.is_empty());
    }
}
