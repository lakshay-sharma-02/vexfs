import re

with open('src/fuse/mod.rs', 'r', encoding='utf-8') as f:
    code = f.read()

# Replace VexFS::new
code = re.sub(
    r'pub fn new\(disk: DiskManager, image_path: &str, ai_tx: Sender<FsEvent>, ai_state: Arc<RwLock<SharedAIState>>\) -> Self \{.*?\}',
    '''pub fn new(disk: DiskManager, image_path: &str) -> Self {
        use crate::ai::engine::AIEngine;
        use crate::ai::neural::NeuralPrefetcher;
        use crate::ai::entropy::EntropyGuard;
        use crate::ai::logger::AccessLog;
        use crate::ai::markov::MarkovPrefetcher;
        use crate::ai::search::SearchIndex;
        use crate::ai::importance::ImportanceEngine;
        
        let engine = AIEngine::new(
            MarkovPrefetcher::new(50_000),
            NeuralPrefetcher::new(),
            ImportanceEngine::new(),
            EntropyGuard::new(),
            SearchIndex::new(),
            AccessLog::new(10_000),
        );
        let (ai_tx, ai_state) = engine.spawn();

        Self {
            index: BPlusTree::new(),
            files: HashMap::new(),
            next_inode: 2,
            disk,
            snapshots: SnapshotManager::new(10),
            last_opened_ino: None,
            write_buffer: WriteBuffer::new(32, 5),
            ai_persist: AIPersistence::new(image_path),
            cache: ArcCache::new(64 * 1024 * 1024),
            ai_tx,
            ai_state,
            search_query: String::new(),
            ask_query: String::new(),
        }
    }''',
    code,
    flags=re.DOTALL
)

# Replace VexFS::load signature
code = code.replace(
    'pub fn load(mut disk: DiskManager, image_path: &str, ai_tx: Sender<FsEvent>, ai_state: Arc<RwLock<SharedAIState>>) -> Self {',
    'pub fn load(mut disk: DiskManager, image_path: &str) -> Self {'
)

# Insert engine spawning right before returning Self in load
code = re.sub(
    r'        Self \{\n\s+index, files, next_inode, disk,\n\s+snapshots,',
    '''        use crate::ai::engine::AIEngine;
        use crate::ai::neural::NeuralPrefetcher;
        use crate::ai::entropy::EntropyGuard;
        use crate::ai::logger::AccessLog;
        
        let engine = AIEngine::new(
            markov,
            NeuralPrefetcher::new(),
            importance,
            EntropyGuard::new(),
            search,
            AccessLog::new(10_000),
        );
        let (ai_tx, ai_state) = engine.spawn();

        Self {
            index, files, next_inode, disk,
            snapshots,''',
    code
)

with open('src/fuse/mod.rs', 'w', encoding='utf-8') as f:
    f.write(code)
