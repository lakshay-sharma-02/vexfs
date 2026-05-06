# Integration patches
# Apply these changes to the existing files

## 1. src/ai/mod.rs
# Add these two lines alongside the other pub mod declarations:
pub mod workspace;
pub mod jarvis;

## 2. src/ai/engine.rs

### 2a. Add imports at top (after existing use statements):
use super::workspace::WorkspaceModel;
use super::jarvis::JarvisEngine;
use std::collections::HashSet;

### 2b. Add to SharedAIState struct:
    /// Rendered .vexfs-jarvis content — updated at EndSession
    pub jarvis_result: Vec<u8>,

### 2c. Add to AIEngine struct:
    workspace: WorkspaceModel,
    /// Inodes touched in the current session (for WorkspaceModel)
    session_files: Vec<u64>,
    /// Per-inode write counts this session
    session_writes: HashMap<u64, u32>,
    /// Per-inode open counts this session
    session_opens: HashMap<u64, u32>,

### 2d. In AIEngine::new(), add to the Self { ... } initializer:
    workspace: WorkspaceModel::new(),
    session_files: Vec::new(),
    session_writes: HashMap::new(),
    session_opens: HashMap::new(),

### 2e. In handle_event, FsEvent::Open arm, add after existing logic:
    // Track for WorkspaceModel
    if !self.session_files.contains(&ino) {
        self.session_files.push(ino);
    }
    *self.session_opens.entry(ino).or_insert(0) += 1;

### 2f. In handle_event, FsEvent::Write arm, add after existing logic:
    // Track for WorkspaceModel
    *self.session_writes.entry(ino).or_insert(0) += 1;

### 2g. Replace the FsEvent::EndSession arm entirely:
    FsEvent::EndSession => {
        self.memory.close_session();
        let stats = self.memory.stats();
        println!(
            "VexFS Memory: session closed. Total: {} sessions, {} files tracked",
            stats.total_sessions, stats.tracked_files
        );

        // Recompute WorkspaceModel
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Build co_access map from memory co-access pairs
        let co_access: HashMap<(u64, u64), u32> = self.memory.co_access
            .iter()
            .map(|(&k, &v)| (k, v))
            .collect();

        self.workspace.recompute(
            &co_access,
            &self.memory.stats_map(),   // HashMap<u64, (String, u32, u64, u64)>
            &self.session_writes,
            &self.session_opens,
            &self.memory.names,
            &self.session_files.clone(),
            now_ts,
        );

        // Generate Jarvis suggestions
        let suggestions = JarvisEngine::analyse(&self.workspace);
        let rendered    = JarvisEngine::render(&suggestions, &self.workspace);

        println!(
            "VexFS Jarvis: {} suggestion(s) ready — read .vexfs-jarvis",
            suggestions.len()
        );
        if !suggestions.is_empty() {
            // Print top suggestion to console so user sees it on unmount
            let top = &suggestions[0];
            println!(
                "  {} {} — {}",
                top.priority.label(), top.kind.label(), top.message
            );
        }

        // Store rendered output for the virtual file
        // sync_state will pick this up
        self.workspace_rendered = rendered.into_bytes();

        // Reset session tracking for next session
        self.session_files.clear();
        self.session_writes.clear();
        self.session_opens.clear();
    }

### 2h. Add field to AIEngine struct:
    workspace_rendered: Vec<u8>,

### 2i. In AIEngine::new(), add to Self { ... }:
    workspace_rendered: Vec::new(),

### 2j. In sync_state(), add before the closing brace:
    w.jarvis_result = self.workspace_rendered.clone();

### 2k. Add stats_map() to MemoryEngine (see section 4 below)


## 3. src/fuse/mod.rs

### 3a. Add constants near other virtual file consts:
const JARVIS_INO:      u64  = 0xFFFFFFFA;
const JARVIS_FILENAME: &str = ".vexfs-jarvis";

### 3b. Add attr method to VexFS impl:
fn jarvis_file_attr(&self) -> FileAttr {
    let size = self.ai_state.read().unwrap().jarvis_result.len() as u64;
    FileAttr {
        ino: JARVIS_INO, size: size.max(1), blocks: 1,
        atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
        ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
        kind: FileType::RegularFile,
        perm: 0o444, nlink: 1, uid: 1000, gid: 1000,
        rdev: 0, blksize: 4096, flags: 0,
    }
}

### 3c. In lookup(), add to the match on name_str:
JARVIS_FILENAME => { reply.entry(&TTL, &self.jarvis_file_attr(), 0); return; }

### 3d. In getattr(), add to the match on ino:
JARVIS_INO => { reply.attr(&TTL, &self.jarvis_file_attr()); return; }

### 3e. In read(), add before the regular file read block:
if ino == JARVIS_INO {
    let data = self.ai_state.read().unwrap().jarvis_result.clone();
    let start = offset as usize;
    let end   = (start + size as usize).min(data.len());
    reply.data(if start < data.len() { &data[start..end] } else { &[] });
    return;
}

### 3f. In readdir(), add to the entries vec:
(JARVIS_INO, FileType::RegularFile, JARVIS_FILENAME),


## 4. src/ai/memory.rs
# Add this method to MemoryEngine impl so engine.rs can call stats_map():

pub fn stats_map(&self) -> HashMap<u64, (String, u32, u64, u64)> {
    // Returns same structure as ImportanceEngine::stats:
    // ino -> (name, access_count, last_access_ts, total_engagement_secs)
    self.names.iter().map(|(&ino, name)| {
        let opens  = self.open_counts.get(&ino).copied().unwrap_or(0);
        let last   = self.last_access.get(&ino).copied().unwrap_or(0);
        let eng    = self.engagement.get(&ino).copied().unwrap_or(0);
        (ino, (name.clone(), opens, last, eng))
    }).collect()
}

# Note: field names (open_counts, last_access, engagement) may differ
# in your actual MemoryEngine — adapt to match whatever fields track
# per-inode access frequency, recency, and engagement time.
# The key contract is: returns HashMap<u64, (String, u32, u64, u64)>
# matching the shape ImportanceEngine uses.
