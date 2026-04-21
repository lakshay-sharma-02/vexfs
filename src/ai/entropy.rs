//! Entropy-based ransomware detection
//!
//! Normal plaintext files:  H ≈ 3.5–5.0 bits/byte
//! Compressed data:         H ≈ 6.5–7.5 bits/byte
//! Encrypted / random data: H ≈ 7.5–8.0 bits/byte
//!
//! Ransomware signature:
//!   Several rapid writes with H > 7.2 AND suspicious filename endings.
//!   Even one write with H > 7.8 to a file that was previously plaintext is a flag.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Bits-per-byte entropy (0.0 – 8.0)
pub fn shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() { return 0.0; }

    let mut freq = [0u64; 256];
    for &b in data {
        freq[b as usize] += 1;
    }

    let len = data.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Suspicious file extension list
fn is_suspicious_extension(name: &str) -> bool {
    let s = name.to_lowercase();
    s.ends_with(".locked")  ||
    s.ends_with(".enc")     ||
    s.ends_with(".crypt")   ||
    s.ends_with(".crypted") ||
    s.ends_with(".encrypt") ||
    s.ends_with(".crypz")   ||
    s.ends_with(".cerber")  ||
    s.ends_with(".locky")   ||
    s.ends_with(".wncry")   ||
    s.ends_with(".wnry")    ||
    s.ends_with(".wcry")    ||
    s.ends_with(".petya")
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreatLevel {
    /// Single very-high entropy write (H > 7.8) on a file that was plaintext
    Critical,
    /// High entropy write (H > 7.2) — suspicious but not confirmed
    Warning,
    /// Suspicious file extension regardless of entropy
    Extension,
    /// Multiple high-entropy writes in a short window
    Pattern,
}

impl ThreatLevel {
    pub fn label(&self) -> &str {
        match self {
            ThreatLevel::Critical  => "🚨 CRITICAL",
            ThreatLevel::Warning   => "⚠️  WARNING",
            ThreatLevel::Extension => "🔍 SUSPICIOUS",
            ThreatLevel::Pattern   => "🚨 PATTERN",
        }
    }
}

#[derive(Debug, Clone)]
struct WriteRecord {
    timestamp: u64,
    entropy: f64,
}

/// Guards the filesystem against ransomware-like write patterns.
///
/// Call `check_write()` on every FUSE write. It returns `Some(ThreatLevel)`
/// if the write looks suspicious — the caller decides whether to block or alert.
pub struct EntropyGuard {
    /// ino → recent high-entropy write records (rolling 60 second window)
    history: HashMap<u64, Vec<WriteRecord>>,
    /// ino → baseline entropy of the file (established on first write)
    baseline: HashMap<u64, f64>,
    /// Total number of threats detected (for status reporting)
    pub threat_count: u64,
    /// High-entropy write threshold for WARNING
    threshold_warn: f64,
    /// High-entropy write threshold for CRITICAL
    threshold_crit: f64,
    /// How many high-entropy writes in the window trigger PATTERN
    pattern_count: usize,
    /// Window size in seconds
    window_secs: u64,
}

impl EntropyGuard {
    pub fn new() -> Self {
        Self {
            history: HashMap::new(),
            baseline: HashMap::new(),
            threat_count: 0,
            threshold_warn: 7.2,
            threshold_crit: 7.8,
            pattern_count: 3,
            window_secs: 60,
        }
    }

    /// Check a write for ransomware signals.
    /// Returns `Some(ThreatLevel)` if suspicious.
    pub fn check_write(&mut self, ino: u64, name: &str, data: &[u8]) -> Option<ThreatLevel> {
        if data.len() < 512 {
            // Too small to compute meaningful entropy — skip
            return None;
        }

        let entropy = shannon_entropy(data);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Establish or update baseline on the first write
        let baseline = self.baseline.entry(ino).or_insert(entropy);
        let prev_baseline = *baseline;

        // Track high-entropy writes
        let mut threat_level = None;

        // 1. Suspicious extension — always flag
        if is_suspicious_extension(name) {
            self.threat_count += 1;
            threat_level = Some(ThreatLevel::Extension);
        }

        if entropy >= self.threshold_crit {
            // 2. Critical: very high entropy AND file was plaintext before
            if prev_baseline < 6.0 {
                self.threat_count += 1;
                threat_level = Some(ThreatLevel::Critical);
            } else {
                // Still warn on very high entropy regardless
                if threat_level.is_none() {
                    self.threat_count += 1;
                    threat_level = Some(ThreatLevel::Warning);
                }
            }
        } else if entropy >= self.threshold_warn && threat_level.is_none() {
            self.threat_count += 1;
            threat_level = Some(ThreatLevel::Warning);
        }

        // 3. Record high-entropy write in rolling window
        if entropy >= self.threshold_warn {
            let records = self.history.entry(ino).or_default();

            // Evict expired records
            records.retain(|r| now.saturating_sub(r.timestamp) <= self.window_secs);

            records.push(WriteRecord { timestamp: now, entropy });

            // 4. Pattern detection: N high-entropy writes in window
            if records.len() >= self.pattern_count {
                self.threat_count += 1;
                threat_level = Some(ThreatLevel::Pattern);
            }
        }

        threat_level
    }

    /// Remove tracking data for a deleted file
    pub fn remove(&mut self, ino: u64) {
        self.history.remove(&ino);
        self.baseline.remove(&ino);
    }

    /// Status summary
    pub fn status(&self) -> String {
        format!(
            "EntropyGuard: {} files monitored, {} threats detected",
            self.baseline.len(),
            self.threat_count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_all_zeros() {
        let data = vec![0u8; 1024];
        let h = shannon_entropy(&data);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn test_entropy_uniform_random() {
        // All 256 byte values equally — maximum entropy
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let h = shannon_entropy(&data);
        assert!((h - 8.0).abs() < 0.01, "expected ~8.0, got {}", h);
    }

    #[test]
    fn test_entropy_text_range() {
        let text = b"The quick brown fox jumps over the lazy dog. ".repeat(100);
        let h = shannon_entropy(&text);
        // English text typically 3.5 - 5.5
        assert!(h > 3.0 && h < 6.0, "text entropy out of range: {}", h);
    }

    #[test]
    fn test_suspicious_extension() {
        assert!(is_suspicious_extension("document.locked"));
        assert!(is_suspicious_extension("photo.enc"));
        assert!(is_suspicious_extension("BACKUP.WNCRY"));
        assert!(!is_suspicious_extension("main.rs"));
        assert!(!is_suspicious_extension("readme.txt"));
    }

    #[test]
    fn test_critical_threat_on_plaintext_to_encrypted() {
        let mut guard = EntropyGuard::new();

        // First write: plaintext (low entropy) — establishes baseline
        let plaintext = b"Hello world, this is a normal text file with boring content.".repeat(20);
        let r1 = guard.check_write(2, "doc.txt", &plaintext);
        assert!(r1.is_none(), "plain text should not trigger");

        // Second write: encrypted data (high entropy)
        let encrypted: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let r2 = guard.check_write(2, "doc.txt", &encrypted);
        assert_eq!(r2, Some(ThreatLevel::Critical));
    }

    #[test]
    fn test_pattern_detection() {
        let mut guard = EntropyGuard::new();
        let encrypted: Vec<u8> = (0..=255u8).cycle().take(4096).collect();

        // Three high-entropy writes in a row → pattern
        guard.check_write(3, "file.txt", &encrypted);
        guard.check_write(3, "file.txt", &encrypted);
        let r = guard.check_write(3, "file.txt", &encrypted);
        assert_eq!(r, Some(ThreatLevel::Pattern));
    }

    #[test]
    fn test_small_writes_ignored() {
        let mut guard = EntropyGuard::new();
        let encrypted: Vec<u8> = (0..=255u8).cycle().take(256).collect();
        let r = guard.check_write(2, "file.txt", &encrypted);
        assert!(r.is_none(), "writes < 512 bytes should be ignored");
    }
}
