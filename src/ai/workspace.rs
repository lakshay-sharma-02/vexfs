//! WorkspaceModel — infers project clusters, session shapes, and stalls
//! from raw MemoryEngine data. No external deps, no LLM. Pure pattern
//! recognition over data the filesystem already collects.
//!
//! Designed to feed JarvisEngine with structured, high-signal observations
//! rather than raw events. Updated at EndSession, not per-event.

use std::collections::{HashMap, HashSet};

// ── Project cluster ───────────────────────────────────────────────────────────

/// A group of files that co-occur frequently enough to constitute a "project".
/// Discovered automatically from co-access pairs — never manually labeled.
#[derive(Debug, Clone)]
pub struct Project {
    /// Auto-derived: name of the most-written file in the cluster.
    pub name: String,
    /// Inodes belonging to this cluster.
    pub files: Vec<u64>,
    /// Display names for those inodes.
    pub file_names: Vec<String>,
    /// Unix timestamp of last access to any file in cluster.
    pub last_active_ts: u64,
    /// Days since last active (computed at snapshot time).
    pub days_inactive: u64,
    /// Average writes/session across cluster files, last 5 sessions.
    pub velocity: f32,
    /// Trending up (true) or cooling down (false).
    pub velocity_rising: bool,
}

// ── Session shape ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SessionKind {
    /// Few files, many writes — deep work on one thing.
    Focus,
    /// Many files, few writes each — broad exploration or refactoring survey.
    Exploration,
    /// Same files reopened 3+ times with few writes — something is blocking.
    Debugging,
    /// Doesn't fit cleanly into one category.
    Mixed,
}

impl SessionKind {
    pub fn label(&self) -> &'static str {
        match self {
            SessionKind::Focus       => "Focus",
            SessionKind::Exploration => "Exploration",
            SessionKind::Debugging   => "Debugging",
            SessionKind::Mixed       => "Mixed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionProfile {
    pub kind: SessionKind,
    /// Name of the project cluster most active in this session.
    pub primary_project: String,
    pub files_touched: usize,
    pub writes_made: usize,
    /// Files opened 3+ times during the session without a write.
    pub reopen_without_write: Vec<String>,
}

// ── Stall record ──────────────────────────────────────────────────────────────

/// A file that shows a stall pattern: opened repeatedly across sessions
/// but rarely written to. Strong signal that something is blocking the user.
#[derive(Debug, Clone)]
pub struct StallRecord {
    pub ino: u64,
    pub name: String,
    /// How many sessions this file was opened without a meaningful write.
    pub stall_session_count: usize,
    /// Total opens with no corresponding write across those sessions.
    pub open_without_write: usize,
}

// ── WorkspaceModel ────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct WorkspaceModel {
    pub projects: Vec<Project>,
    pub last_session: Option<SessionProfile>,
    pub stalls: Vec<StallRecord>,
    /// Files that were dirty/mid-edit at last EndSession.
    pub unfinished_files: Vec<String>,
    /// Snapshot timestamp (when this model was last computed).
    pub computed_at: u64,
}

impl WorkspaceModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute the full workspace model from raw MemoryEngine data.
    /// Call this at EndSession — not per-event.
    ///
    /// Parameters:
    ///   `co_access`   — HashMap<(ino_a, ino_b), count> of co-occurrence pairs
    ///   `access_counts` — HashMap<ino, (name, total_opens, last_ts, total_write_secs)>
    ///   `write_counts`  — HashMap<ino, u32> writes per inode this session
    ///   `open_counts`   — HashMap<ino, u32> opens per inode this session
    ///   `names`         — HashMap<ino, String>
    ///   `session_files` — inodes touched in the just-closed session
    ///   `now_ts`        — current unix timestamp
    pub fn recompute(
        &mut self,
        co_access:      &HashMap<(u64, u64), u32>,
        access_counts:  &HashMap<u64, (String, u32, u64, u64)>,
        write_counts:   &HashMap<u64, u32>,
        open_counts:    &HashMap<u64, u32>,
        names:          &HashMap<u64, String>,
        session_files:  &[u64],
        now_ts:         u64,
    ) {
        self.computed_at = now_ts;

        // 1. Discover project clusters from co-access graph
        self.projects = discover_projects(co_access, access_counts, names, now_ts);

        // 2. Profile the session that just ended
        self.last_session = Some(profile_session(
            session_files,
            write_counts,
            open_counts,
            names,
            &self.projects,
        ));

        // 3. Detect stalls
        self.stalls = detect_stalls(access_counts, write_counts, open_counts, names);

        // 4. Unfinished files: opened this session, written to, not "cleanly closed"
        //    Heuristic: written to but write count is odd (interrupted mid-thought)
        self.unfinished_files = session_files.iter()
            .filter(|&&ino| {
                write_counts.get(&ino).copied().unwrap_or(0) > 0
                    && open_counts.get(&ino).copied().unwrap_or(0) >= 2
            })
            .filter_map(|ino| names.get(ino).cloned())
            .collect();
    }
}

// ── Cluster discovery ─────────────────────────────────────────────────────────

/// Union-Find based clustering over co-access pairs.
/// Two files go in the same project if they co-occur with count >= threshold.
fn discover_projects(
    co_access:     &HashMap<(u64, u64), u32>,
    access_counts: &HashMap<u64, (String, u32, u64, u64)>,
    names:         &HashMap<u64, String>,
    now_ts:        u64,
) -> Vec<Project> {
    const CO_THRESHOLD: u32 = 3; // must co-occur at least 3 times

    // Collect all inodes
    let all_inos: HashSet<u64> = access_counts.keys().copied().collect();
    if all_inos.is_empty() {
        return vec![];
    }

    // Union-Find
    let mut parent: HashMap<u64, u64> = all_inos.iter().map(|&i| (i, i)).collect();

    fn find(parent: &mut HashMap<u64, u64>, x: u64) -> u64 {
        if parent[&x] == x { return x; }
        let p = find(parent, parent[&x]);
        parent.insert(x, p);
        p
    }

    fn union(parent: &mut HashMap<u64, u64>, a: u64, b: u64) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb { parent.insert(ra, rb); }
    }

    for (&(a, b), &count) in co_access {
        if count >= CO_THRESHOLD {
            union(&mut parent, a, b);
        }
    }

    // Group inodes by root
    let mut clusters: HashMap<u64, Vec<u64>> = HashMap::new();
    for &ino in &all_inos {
        let root = find(&mut parent, ino);
        clusters.entry(root).or_default().push(ino);
    }

    // Build Project structs — skip singletons (no co-access peers)
    clusters.into_values()
        .filter(|c| c.len() >= 2)
        .map(|file_inos| {
            let file_names: Vec<String> = file_inos.iter()
                .filter_map(|ino| names.get(ino).cloned())
                .collect();

            // Primary name: most-written file in cluster
            let primary_ino = file_inos.iter()
                .max_by_key(|&&ino| {
                    access_counts.get(&ino).map(|s| s.3).unwrap_or(0)
                })
                .copied()
                .unwrap_or(file_inos[0]);

            let name = names.get(&primary_ino)
                .cloned()
                .unwrap_or_else(|| format!("project_{}", primary_ino));

            // Last active: max last_ts across cluster
            let last_active_ts = file_inos.iter()
                .filter_map(|ino| access_counts.get(ino).map(|s| s.2))
                .max()
                .unwrap_or(0);

            let days_inactive = if last_active_ts > 0 && now_ts > last_active_ts {
                (now_ts - last_active_ts) / 86_400
            } else {
                0
            };

            // Velocity: average access_count across cluster (proxy for write activity)
            let velocity = file_inos.iter()
                .filter_map(|ino| access_counts.get(ino).map(|s| s.1 as f32))
                .sum::<f32>()
                / file_inos.len() as f32;

            // Rising if last_active is recent (< 2 days)
            let velocity_rising = days_inactive < 2;

            Project {
                name,
                files: file_inos,
                file_names,
                last_active_ts,
                days_inactive,
                velocity,
                velocity_rising,
            }
        })
        .collect()
}

// ── Session profiling ─────────────────────────────────────────────────────────

fn profile_session(
    session_files: &[u64],
    write_counts:  &HashMap<u64, u32>,
    open_counts:   &HashMap<u64, u32>,
    names:         &HashMap<u64, String>,
    projects:      &[Project],
) -> SessionProfile {
    let files_touched = session_files.len();
    let writes_made: u32 = write_counts.values().sum();

    // Files reopened 3+ times without a write = debugging signal
    let reopen_without_write: Vec<String> = session_files.iter()
        .filter(|&&ino| {
            open_counts.get(&ino).copied().unwrap_or(0) >= 3
                && write_counts.get(&ino).copied().unwrap_or(0) == 0
        })
        .filter_map(|ino| names.get(ino).cloned())
        .collect();

    let kind = if !reopen_without_write.is_empty() {
        SessionKind::Debugging
    } else if files_touched <= 3 && writes_made >= 5 {
        SessionKind::Focus
    } else if files_touched >= 6 && writes_made <= files_touched as u32 {
        SessionKind::Exploration
    } else {
        SessionKind::Mixed
    };

    // Primary project: which cluster had the most files touched this session
    let session_set: HashSet<u64> = session_files.iter().copied().collect();
    let primary_project = projects.iter()
        .max_by_key(|p| {
            p.files.iter().filter(|f| session_set.contains(f)).count()
        })
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    SessionProfile {
        kind,
        primary_project,
        files_touched,
        writes_made: writes_made as usize,
        reopen_without_write,
    }
}

// ── Stall detection ───────────────────────────────────────────────────────────

fn detect_stalls(
    access_counts: &HashMap<u64, (String, u32, u64, u64)>,
    write_counts:  &HashMap<u64, u32>,
    open_counts:   &HashMap<u64, u32>,
    names:         &HashMap<u64, String>,
) -> Vec<StallRecord> {
    const MIN_OPENS_FOR_STALL: u32 = 4;
    const MAX_WRITES_FOR_STALL: u32 = 1;

    access_counts.iter()
        .filter_map(|(&ino, (_, total_opens, _, _))| {
            let session_opens  = open_counts.get(&ino).copied().unwrap_or(0);
            let session_writes = write_counts.get(&ino).copied().unwrap_or(0);

            // Stall: many opens, almost no writes, across multiple access occasions
            if *total_opens >= MIN_OPENS_FOR_STALL as u32
                && session_opens >= 2
                && session_writes <= MAX_WRITES_FOR_STALL
            {
                let name = names.get(&ino)
                    .cloned()
                    .unwrap_or_else(|| format!("ino_{}", ino));

                Some(StallRecord {
                    ino,
                    name,
                    stall_session_count: (session_opens / 2) as usize,
                    open_without_write: session_opens as usize,
                })
            } else {
                None
            }
        })
        .collect()
}
