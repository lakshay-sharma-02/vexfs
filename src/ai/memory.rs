//! Cross-session memory — the feature that makes VexFS feel alive.
//!
//! Normal filesystems forget everything when you unmount them.
//! VexFS remembers:
//!   - Every session (when it started, what you worked on, how long)
//!   - Temporal patterns (you always touch auth.rs on Monday mornings)
//!   - Co-access clusters (these 5 files always get opened together)
//!   - Trend data (your engagement with this file is increasing)
//!   - Streaks (you've touched this module every day for 8 days)
//!
//! This is the data that makes the `.vexfs-context` virtual file say something
//! meaningful, and what makes VexFS feel like it knows you.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Session ──────────────────────────────────────────────────────────────────

/// A single mount session — from `vexfs mount` to `fusermount -u`.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: u64,
    pub start_ts: u64,     // unix seconds
    pub end_ts: u64,       // 0 if still active
    pub files_touched: Vec<(u64, String)>,  // (ino, name), in order
    pub peak_files: usize, // max simultaneous open files
    pub total_writes: u64,
    pub total_reads: u64,
    pub focus_file: Option<String>, // most-accessed file this session
}

impl Session {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            start_ts: now_secs(),
            end_ts: 0,
            files_touched: Vec::new(),
            peak_files: 0,
            total_writes: 0,
            total_reads: 0,
            focus_file: None,
        }
    }

    pub fn duration_secs(&self) -> u64 {
        let end = if self.end_ts > 0 { self.end_ts } else { now_secs() };
        end.saturating_sub(self.start_ts)
    }

    pub fn is_active(&self) -> bool {
        self.end_ts == 0
    }

    /// Hour of day this session started (0–23), used for temporal patterns.
    pub fn start_hour(&self) -> u8 {
        ((self.start_ts % 86400) / 3600) as u8
    }

    /// Day of week (0=Mon … 6=Sun), approximate from unix epoch.
    pub fn day_of_week(&self) -> u8 {
        // Unix epoch was a Thursday (day 3 if Mon=0)
        let days_since_epoch = self.start_ts / 86400;
        ((days_since_epoch + 3) % 7) as u8
    }
}

// ── Temporal pattern ─────────────────────────────────────────────────────────

/// Tracks when a file tends to be accessed — by hour and day of week.
#[derive(Debug, Clone, Default)]
pub struct TemporalPattern {
    /// hour_of_day → access count
    pub by_hour: [u32; 24],
    /// day_of_week → access count (0=Mon)
    pub by_day: [u32; 7],
    /// Total accesses recorded
    pub total: u32,
}

impl TemporalPattern {
    pub fn record(&mut self, ts: u64) {
        let hour = ((ts % 86400) / 3600) as usize;
        let day = (((ts / 86400) + 3) % 7) as usize;
        self.by_hour[hour] += 1;
        self.by_day[day] += 1;
        self.total += 1;
    }

    /// Peak hour (0–23)
    pub fn peak_hour(&self) -> u8 {
        self.by_hour
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| *c)
            .map(|(h, _)| h as u8)
            .unwrap_or(0)
    }

    /// Peak day (0=Mon … 6=Sun)
    pub fn peak_day(&self) -> u8 {
        self.by_day
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| *c)
            .map(|(d, _)| d as u8)
            .unwrap_or(0)
    }

    pub fn day_name(day: u8) -> &'static str {
        match day {
            0 => "Monday", 1 => "Tuesday", 2 => "Wednesday",
            3 => "Thursday", 4 => "Friday", 5 => "Saturday",
            6 => "Sunday", _ => "Unknown",
        }
    }

    pub fn hour_label(hour: u8) -> String {
        if hour == 0 { "midnight".to_string() }
        else if hour < 12 { format!("{}am", hour) }
        else if hour == 12 { "noon".to_string() }
        else { format!("{}pm", hour - 12) }
    }
}

// ── File streak ──────────────────────────────────────────────────────────────

/// How many consecutive days this file has been touched.
#[derive(Debug, Clone)]
pub struct Streak {
    pub current_days: u32,
    pub longest_days: u32,
    pub last_touched_day: u64,  // days since epoch
}

impl Streak {
    pub fn new() -> Self {
        Self { current_days: 0, longest_days: 0, last_touched_day: 0 }
    }

    pub fn touch(&mut self, ts: u64) {
        let today = ts / 86400;
        if today == self.last_touched_day {
            // Same day — no change to streak
        } else if today == self.last_touched_day + 1 {
            // Consecutive day
            self.current_days += 1;
            if self.current_days > self.longest_days {
                self.longest_days = self.current_days;
            }
        } else {
            // Streak broken
            self.current_days = 1;
        }
        self.last_touched_day = today;
    }
}

// ── Co-access cluster ────────────────────────────────────────────────────────

/// Files that tend to be opened together in the same session.
/// This is the "project context" — VexFS learns which files are related
/// without you ever telling it.
#[derive(Debug, Clone)]
pub struct CoAccessMap {
    /// (ino_a, ino_b) → times seen together in same session (ino_a < ino_b)
    pub pairs: HashMap<(u64, u64), u32>,
}

impl CoAccessMap {
    pub fn new() -> Self {
        Self { pairs: HashMap::new() }
    }

    /// Record all pairs from a session's file list.
    pub fn record_session(&mut self, files: &[(u64, String)]) {
        // Only track meaningful sessions (2+ files)
        if files.len() < 2 { return; }

        // Take the first 20 unique inodes to avoid O(n²) blowup
        let unique: Vec<u64> = {
            let mut seen = std::collections::HashSet::new();
            files.iter()
                .filter(|(ino, _)| seen.insert(*ino))
                .map(|(ino, _)| *ino)
                .take(20)
                .collect()
        };

        for i in 0..unique.len() {
            for j in (i + 1)..unique.len() {
                let a = unique[i].min(unique[j]);
                let b = unique[i].max(unique[j]);
                *self.pairs.entry((a, b)).or_insert(0) += 1;
            }
        }
    }

    /// Top co-accessed files for a given inode.
    /// Returns (partner_ino, co_access_count), sorted descending.
    pub fn top_partners(&self, ino: u64, limit: usize) -> Vec<(u64, u32)> {
        let mut partners: Vec<(u64, u32)> = self.pairs.iter()
            .filter_map(|(&(a, b), &count)| {
                if a == ino { Some((b, count)) }
                else if b == ino { Some((a, count)) }
                else { None }
            })
            .collect();
        partners.sort_by(|x, y| y.1.cmp(&x.1));
        partners.truncate(limit);
        partners
    }
}

// ── Trend tracker ────────────────────────────────────────────────────────────

/// Rolling 7-day access counts per file.
/// Compares this week to last week to detect trending files.
#[derive(Debug, Clone)]
pub struct TrendTracker {
    /// ino → [access_count_per_day; 14]  (index 0 = 13 days ago, 13 = today)
    pub daily: HashMap<u64, [u32; 14]>,
    /// The day-since-epoch when the tracker was last updated
    last_day: u64,
}

impl TrendTracker {
    pub fn new() -> Self {
        Self {
            daily: HashMap::new(),
            last_day: now_secs() / 86400,
        }
    }

    pub fn record(&mut self, ino: u64) {
        let today = now_secs() / 86400;
        // Advance the window if a new day has started
        if today > self.last_day {
            let advance = (today - self.last_day).min(14) as usize;
            for counts in self.daily.values_mut() {
                counts.rotate_left(advance);
                for c in counts.iter_mut().rev().take(advance) {
                    *c = 0;
                }
            }
            self.last_day = today;
        }

        let counts = self.daily.entry(ino).or_insert([0u32; 14]);
        counts[13] += 1;
    }

    /// Is this file trending up? Compares last 7 days to prior 7 days.
    pub fn trend(&self, ino: u64) -> Trend {
        let counts = match self.daily.get(&ino) {
            Some(c) => c,
            None => return Trend::Stable,
        };
        let recent: u32 = counts[7..14].iter().sum();
        let prior: u32 = counts[0..7].iter().sum();

        if prior == 0 && recent > 2 {
            return Trend::New;
        }
        if prior == 0 {
            return Trend::Stable;
        }

        let ratio = recent as f32 / prior as f32;
        if ratio >= 2.0 { Trend::Rising }
        else if ratio <= 0.5 { Trend::Falling }
        else { Trend::Stable }
    }

    /// Files with Rising or New trend, sorted by recent access count desc.
    pub fn trending_files(&self) -> Vec<(u64, Trend, u32)> {
        let mut out: Vec<(u64, Trend, u32)> = self.daily.iter()
            .filter_map(|(&ino, counts)| {
                let trend = self.trend(ino);
                if matches!(trend, Trend::Rising | Trend::New) {
                    let recent: u32 = counts[7..14].iter().sum();
                    Some((ino, trend, recent))
                } else {
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| b.2.cmp(&a.2));
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Trend {
    New,     // first week of access
    Rising,  // 2× more accesses than prior week
    Stable,
    Falling, // less than half prior week
}

impl Trend {
    pub fn label(&self) -> &str {
        match self {
            Trend::New     => "⭐ new",
            Trend::Rising  => "📈 rising",
            Trend::Stable  => "→ stable",
            Trend::Falling => "📉 fading",
        }
    }
}

// ── Memory engine ─────────────────────────────────────────────────────────────

/// The cross-session memory system.
/// Owns all long-term behavioral data about the user.
pub struct MemoryEngine {
    /// All past sessions, capped at MAX_SESSIONS
    pub sessions: Vec<Session>,
    /// Current live session
    pub current_session: Session,
    /// Temporal patterns per file (ino → pattern)
    pub temporal: HashMap<u64, TemporalPattern>,
    /// Streak tracking per file
    pub streaks: HashMap<u64, Streak>,
    /// Co-access clusters
    pub co_access: CoAccessMap,
    /// 14-day trend tracker
    pub trends: TrendTracker,
    /// file name lookup (ino → name) — persisted separately
    pub names: HashMap<u64, String>,
    /// Total sessions ever recorded (monotonic counter)
    pub total_sessions: u64,
}

const MAX_SESSIONS: usize = 500;

impl MemoryEngine {
    pub fn new() -> Self {
        let session_id = now_secs(); // use timestamp as unique id
        Self {
            sessions: Vec::new(),
            current_session: Session::new(session_id),
            temporal: HashMap::new(),
            streaks: HashMap::new(),
            co_access: CoAccessMap::new(),
            trends: TrendTracker::new(),
            names: HashMap::new(),
            total_sessions: 0,
        }
    }

    /// Record a file access in the current session.
    pub fn record_access(&mut self, ino: u64, name: &str) {
        let ts = now_secs();

        // Update name lookup
        self.names.insert(ino, name.to_string());

        // Add to current session's file list (avoid spam — once per file per session)
        if !self.current_session.files_touched.iter().any(|(i, _)| *i == ino) {
            self.current_session.files_touched.push((ino, name.to_string()));
        }

        // Temporal pattern
        self.temporal.entry(ino).or_default().record(ts);

        // Streak
        self.streaks.entry(ino).or_insert_with(Streak::new).touch(ts);

        // Trend
        self.trends.record(ino);
    }

    pub fn record_write(&mut self, _ino: u64) {
        self.current_session.total_writes += 1;
    }

    pub fn record_read(&mut self, _ino: u64) {
        self.current_session.total_reads += 1;
    }

    /// Close the current session and archive it.
    pub fn close_session(&mut self) {
        let ts = now_secs();
        self.current_session.end_ts = ts;

        // Compute focus file
        let mut freq: HashMap<u64, u32> = HashMap::new();
        for (ino, _) in &self.current_session.files_touched {
            *freq.entry(*ino).or_insert(0) += 1;
        }
        self.current_session.focus_file = freq
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .and_then(|(ino, _)| self.names.get(&ino).cloned());

        // Record co-access patterns
        let files = self.current_session.files_touched.clone();
        self.co_access.record_session(&files);

        // Archive session
        let archived = self.current_session.clone();
        self.sessions.push(archived);
        if self.sessions.len() > MAX_SESSIONS {
            self.sessions.remove(0);
        }

        self.total_sessions += 1;

        // Start a fresh session
        let new_id = ts + self.total_sessions;
        self.current_session = Session::new(new_id);
    }

    /// Context summary for the `.vexfs-context` virtual file.
    /// This is what people see in the demo.
    pub fn context_summary(&self, name_lookup: &HashMap<u64, String>) -> String {
        let mut out = String::new();

        // ── Current session ───────────────────────────────────────────────
        let dur = self.current_session.duration_secs();
        let dur_str = if dur < 60 {
            format!("{}s", dur)
        } else if dur < 3600 {
            format!("{}m", dur / 60)
        } else {
            format!("{}h {}m", dur / 3600, (dur % 3600) / 60)
        };

        out.push_str(&format!("## Current session ({})\n", dur_str));

        let touched = &self.current_session.files_touched;
        if touched.is_empty() {
            out.push_str("No files opened yet.\n");
        } else {
            out.push_str(&format!(
                "Files touched: {}\n",
                touched.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        out.push_str(&format!(
            "Writes: {}  Reads: {}\n",
            self.current_session.total_writes,
            self.current_session.total_reads,
        ));

        // ── Cross-session memory ──────────────────────────────────────────
        out.push_str(&format!(
            "\n## Memory ({} sessions total)\n",
            self.total_sessions + 1  // +1 for current
        ));

        // Trending files
        let trending = self.trends.trending_files();
        if !trending.is_empty() {
            out.push_str("Trending:\n");
            for (ino, trend, count) in trending.iter().take(3) {
                let name = name_lookup.get(ino)
                    .or_else(|| self.names.get(ino))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");
                out.push_str(&format!("  {} {} ({} accesses this week)\n",
                    trend.label(), name, count));
            }
        }

        // Streaks
        let mut streaks: Vec<(&u64, &Streak)> = self.streaks.iter()
            .filter(|(_, s)| s.current_days >= 2)
            .collect();
        streaks.sort_by(|a, b| b.1.current_days.cmp(&a.1.current_days));
        if !streaks.is_empty() {
            out.push_str("Streaks:\n");
            for (ino, streak) in streaks.iter().take(3) {
                let name = name_lookup.get(*ino)
                    .or_else(|| self.names.get(*ino))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");
                out.push_str(&format!(
                    "  🔥 {} — {} days in a row (best: {})\n",
                    name, streak.current_days, streak.longest_days
                ));
            }
        }

        // Recent sessions
        if !self.sessions.is_empty() {
            out.push_str("\nRecent sessions:\n");
            for session in self.sessions.iter().rev().take(3) {
                let dur = session.duration_secs();
                let dur_str = if dur < 60 { format!("{}s", dur) }
                    else if dur < 3600 { format!("{}m", dur / 60) }
                    else { format!("{}h", dur / 3600) };
                let focus = session.focus_file.as_deref().unwrap_or("—");
                let file_count = session.files_touched.len();
                let ago = format_age(now_secs().saturating_sub(session.start_ts));
                out.push_str(&format!(
                    "  {} ago — {}  focus: {}  ({} files)\n",
                    ago, dur_str, focus, file_count
                ));
            }
        }

        // Temporal insight for most-used file
        if let Some((ino, pat)) = self.temporal.iter()
            .filter(|(_, p)| p.total >= 5)
            .max_by_key(|(_, p)| p.total)
        {
            let name = name_lookup.get(ino)
                .or_else(|| self.names.get(ino))
                .map(|s| s.as_str())
                .unwrap_or("your most-used file");
            out.push_str(&format!(
                "\nInsight: you usually work on '{}' on {}s around {}\n",
                name,
                TemporalPattern::day_name(pat.peak_day()),
                TemporalPattern::hour_label(pat.peak_hour()),
            ));
        }

        out
    }

    /// Returns the top co-accessed file names for a given ino.
    pub fn top_cofiles(&self, ino: u64, limit: usize) -> Vec<String> {
        self.co_access.top_partners(ino, limit)
            .into_iter()
            .filter_map(|(partner_ino, _)| {
                self.names.get(&partner_ino).cloned()
            })
            .collect()
    }

    /// Current streak for a file.
    pub fn streak(&self, ino: u64) -> u32 {
        self.streaks.get(&ino).map(|s| s.current_days).unwrap_or(0)
    }

    /// Stats for status reporting.
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            total_sessions: self.total_sessions + 1,
            current_session_files: self.current_session.files_touched.len(),
            tracked_files: self.temporal.len(),
            active_streaks: self.streaks.values().filter(|s| s.current_days >= 2).count(),
            trending_count: self.trends.trending_files().len(),
            co_access_pairs: self.co_access.pairs.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_sessions: u64,
    pub current_session_files: usize,
    pub tracked_files: usize,
    pub active_streaks: usize,
    pub trending_count: usize,
    pub co_access_pairs: usize,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_age(secs: u64) -> String {
    if secs < 60 { format!("{}s", secs) }
    else if secs < 3600 { format!("{}m", secs / 60) }
    else if secs < 86400 { format!("{}h", secs / 3600) }
    else { format!("{}d", secs / 86400) }
}

// ── Serialization ─────────────────────────────────────────────────────────────
// Simple binary format — no serde dependency.
// Layout documented inline. All integers little-endian.

impl MemoryEngine {
    /// Serialize to bytes for persistence.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();

        // Magic + version
        out.extend_from_slice(b"VEXMEM01");
        push_u64(&mut out, self.total_sessions);

        // Sessions
        push_u32(&mut out, self.sessions.len().min(500) as u32);
        for s in self.sessions.iter().take(500) {
            push_u64(&mut out, s.id);
            push_u64(&mut out, s.start_ts);
            push_u64(&mut out, s.end_ts);
            push_u64(&mut out, s.total_writes);
            push_u64(&mut out, s.total_reads);
            push_u32(&mut out, s.files_touched.len().min(100) as u32);
            for (ino, name) in s.files_touched.iter().take(100) {
                push_u64(&mut out, *ino);
                push_str(&mut out, name);
            }
            push_str(&mut out, s.focus_file.as_deref().unwrap_or(""));
        }

        // Temporal patterns
        push_u32(&mut out, self.temporal.len().min(10_000) as u32);
        for (ino, pat) in self.temporal.iter().take(10_000) {
            push_u64(&mut out, *ino);
            for h in &pat.by_hour { push_u32(&mut out, *h); }
            for d in &pat.by_day  { push_u32(&mut out, *d); }
            push_u32(&mut out, pat.total);
        }

        // Streaks
        push_u32(&mut out, self.streaks.len().min(10_000) as u32);
        for (ino, streak) in self.streaks.iter().take(10_000) {
            push_u64(&mut out, *ino);
            push_u32(&mut out, streak.current_days);
            push_u32(&mut out, streak.longest_days);
            push_u64(&mut out, streak.last_touched_day);
        }

        // Co-access pairs
        push_u32(&mut out, self.co_access.pairs.len().min(50_000) as u32);
        for ((a, b), count) in self.co_access.pairs.iter().take(50_000) {
            push_u64(&mut out, *a);
            push_u64(&mut out, *b);
            push_u32(&mut out, *count);
        }

        // Trend tracker (daily arrays)
        push_u64(&mut out, self.trends.last_day);
        push_u32(&mut out, self.trends.daily.len().min(10_000) as u32);
        for (ino, counts) in self.trends.daily.iter().take(10_000) {
            push_u64(&mut out, *ino);
            for c in counts { push_u32(&mut out, *c); }
        }

        // Name lookup
        push_u32(&mut out, self.names.len().min(10_000) as u32);
        for (ino, name) in self.names.iter().take(10_000) {
            push_u64(&mut out, *ino);
            push_str(&mut out, name);
        }

        out
    }

    /// Deserialize from bytes. Returns None on any parse error — never crashes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut p = 0usize;

        if data.len() < 8 { return None; }
        if &data[..8] != b"VEXMEM01" { return None; }
        p += 8;

        let total_sessions = read_u64(data, &mut p)?;

        // Sessions
        let session_count = read_u32(data, &mut p)? as usize;
        let mut sessions = Vec::with_capacity(session_count.min(500));
        for _ in 0..session_count.min(500) {
            let id          = read_u64(data, &mut p)?;
            let start_ts    = read_u64(data, &mut p)?;
            let end_ts      = read_u64(data, &mut p)?;
            let total_writes = read_u64(data, &mut p)?;
            let total_reads  = read_u64(data, &mut p)?;
            let file_count  = read_u32(data, &mut p)? as usize;
            let mut files_touched = Vec::with_capacity(file_count.min(100));
            for _ in 0..file_count.min(100) {
                let ino  = read_u64(data, &mut p)?;
                let name = read_str(data, &mut p)?;
                files_touched.push((ino, name));
            }
            let focus_raw = read_str(data, &mut p)?;
            let focus_file = if focus_raw.is_empty() { None } else { Some(focus_raw) };
            sessions.push(Session {
                id, start_ts, end_ts, files_touched,
                total_writes, total_reads,
                peak_files: 0, focus_file,
            });
        }

        // Temporal patterns
        let tp_count = read_u32(data, &mut p)? as usize;
        let mut temporal = HashMap::new();
        for _ in 0..tp_count.min(10_000) {
            let ino = read_u64(data, &mut p)?;
            let mut by_hour = [0u32; 24];
            let mut by_day  = [0u32; 7];
            for h in &mut by_hour { *h = read_u32(data, &mut p)?; }
            for d in &mut by_day  { *d = read_u32(data, &mut p)?; }
            let total = read_u32(data, &mut p)?;
            temporal.insert(ino, TemporalPattern { by_hour, by_day, total });
        }

        // Streaks
        let streak_count = read_u32(data, &mut p)? as usize;
        let mut streaks = HashMap::new();
        for _ in 0..streak_count.min(10_000) {
            let ino              = read_u64(data, &mut p)?;
            let current_days     = read_u32(data, &mut p)?;
            let longest_days     = read_u32(data, &mut p)?;
            let last_touched_day = read_u64(data, &mut p)?;
            streaks.insert(ino, Streak { current_days, longest_days, last_touched_day });
        }

        // Co-access pairs
        let pair_count = read_u32(data, &mut p)? as usize;
        let mut pairs = HashMap::new();
        for _ in 0..pair_count.min(50_000) {
            let a     = read_u64(data, &mut p)?;
            let b     = read_u64(data, &mut p)?;
            let count = read_u32(data, &mut p)?;
            pairs.insert((a, b), count);
        }

        // Trend tracker
        let last_day = read_u64(data, &mut p)?;
        let trend_count = read_u32(data, &mut p)? as usize;
        let mut daily = HashMap::new();
        for _ in 0..trend_count.min(10_000) {
            let ino = read_u64(data, &mut p)?;
            let mut counts = [0u32; 14];
            for c in &mut counts { *c = read_u32(data, &mut p)?; }
            daily.insert(ino, counts);
        }

        // Name lookup
        let name_count = read_u32(data, &mut p)? as usize;
        let mut names = HashMap::new();
        for _ in 0..name_count.min(10_000) {
            let ino  = read_u64(data, &mut p)?;
            let name = read_str(data, &mut p)?;
            names.insert(ino, name);
        }

        let session_id = now_secs();
        Some(Self {
            sessions,
            current_session: Session::new(session_id),
            temporal,
            streaks,
            co_access: CoAccessMap { pairs },
            trends: TrendTracker { daily, last_day },
            names,
            total_sessions,
        })
    }
}

// ── Binary helpers ────────────────────────────────────────────────────────────

fn push_u64(out: &mut Vec<u8>, v: u64) { out.extend_from_slice(&v.to_le_bytes()); }
fn push_u32(out: &mut Vec<u8>, v: u32) { out.extend_from_slice(&v.to_le_bytes()); }
fn push_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let len = b.len().min(255) as u8;
    out.push(len);
    out.extend_from_slice(&b[..len as usize]);
}

fn read_u64(data: &[u8], p: &mut usize) -> Option<u64> {
    if *p + 8 > data.len() { return None; }
    let v = u64::from_le_bytes(data[*p..*p+8].try_into().ok()?);
    *p += 8;
    Some(v)
}
fn read_u32(data: &[u8], p: &mut usize) -> Option<u32> {
    if *p + 4 > data.len() { return None; }
    let v = u32::from_le_bytes(data[*p..*p+4].try_into().ok()?);
    *p += 4;
    Some(v)
}
fn read_str(data: &[u8], p: &mut usize) -> Option<String> {
    if *p >= data.len() { return None; }
    let len = data[*p] as usize;
    *p += 1;
    if *p + len > data.len() { return None; }
    let s = String::from_utf8_lossy(&data[*p..*p+len]).to_string();
    *p += len;
    Some(s)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_tracks_files() {
        let mut mem = MemoryEngine::new();
        mem.record_access(2, "main.rs");
        mem.record_access(3, "lib.rs");
        mem.record_access(2, "main.rs"); // duplicate — should not double-add

        assert_eq!(mem.current_session.files_touched.len(), 2);
    }
}
