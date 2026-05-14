//! B+ Tree — hierarchical metadata index for VexFS.
//!
//! Keys are now composite: `Key(parent_ino: u64, name: String)`.
//! Ordering is `parent_ino` first, then `name` lexicographically.
//! This lets `list_dir(parent_ino)` do an efficient O(log n + k) range scan
//! that returns only the direct children of a given directory inode —
//! no full-table scan, no filtering, just a tight B+ tree range.

const ORDER: usize = 8; // max children per internal node

// ─── Key ─────────────────────────────────────────────────────────────────────

/// Composite B+ tree key: (parent directory inode, entry name).
///
/// Sorted by `parent_ino` first, then name lexicographically.
/// This clustering property is what makes `list_dir` an O(log n + k) range
/// scan rather than a full O(n) table walk.
#[derive(Debug, Clone)]
pub struct Key(pub u64, pub String);

impl Key {
    pub fn new(parent_ino: u64, name: &str) -> Self {
        Key(parent_ino, name.to_string())
    }

    /// Sentinel key: the smallest possible key for a given parent directory.
    fn dir_start(parent_ino: u64) -> Self {
        Key(parent_ino, String::new())
    }

    /// Sentinel key: the largest possible key for a given parent directory.
    /// `\u{10FFFF}` is the highest valid Unicode scalar — sorts after every
    /// real filename, so this sentinel cleanly bounds the directory range.
    fn dir_end(parent_ino: u64) -> Self {
        Key(parent_ino, "\u{10FFFF}".to_string())
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // parent_ino first → all children of a directory cluster together.
        // Within the same parent, entries are alphabetical.
        self.0.cmp(&other.0).then_with(|| self.1.cmp(&other.1))
    }
}

// ─── Value ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Value {
    pub ino:        u64,
    pub size:       u64,
    pub is_dir:     bool,
    pub disk_index: usize,
}

// ─── Internal node types ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Node {
    Leaf {
        keys: Vec<Key>,
        vals: Vec<Value>,
    },
    Internal {
        keys:     Vec<Key>,   // separator keys
        children: Vec<Node>,  // len = keys.len() + 1
    },
}

/// Result of inserting into a node — may produce a split.
enum InsertResult {
    Ok,
    Split(Key, Node), // promoted key + new right node
}

/// Result of removing from a node.
enum RemoveResult {
    Ok(Option<Value>),
    NotFound,
}

#[allow(dead_code)]
impl Node {
    fn new_leaf() -> Self {
        Node::Leaf { keys: vec![], vals: vec![] }
    }

    fn new_internal(keys: Vec<Key>, children: Vec<Node>) -> Self {
        Node::Internal { keys, children }
    }

    fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf { .. })
    }

    fn key_count(&self) -> usize {
        match self {
            Node::Leaf { keys, .. }     => keys.len(),
            Node::Internal { keys, .. } => keys.len(),
        }
    }

    fn is_full(&self) -> bool {
        self.key_count() >= ORDER - 1
    }

    /// Search for a value by key.
    fn get(&self, key: &Key) -> Option<&Value> {
        match self {
            Node::Leaf { keys, vals } => {
                let pos = keys.partition_point(|k| k < key);
                if pos < keys.len() && &keys[pos] == key {
                    Some(&vals[pos])
                } else {
                    None
                }
            }
            Node::Internal { keys, children } => {
                let pos = keys.partition_point(|k| k <= key);
                children[pos].get(key)
            }
        }
    }

    /// Insert key-value, return split info if node overflows.
    fn insert(&mut self, key: Key, val: Value) -> InsertResult {
        match self {
            Node::Leaf { keys, vals } => {
                let pos = keys.partition_point(|k| k < &key);
                if pos < keys.len() && keys[pos] == key {
                    vals[pos] = val; // update existing
                    return InsertResult::Ok;
                }
                keys.insert(pos, key);
                vals.insert(pos, val);

                if keys.len() >= ORDER {
                    // Split leaf: right half goes into a new leaf node.
                    let mid        = keys.len() / 2;
                    let right_keys = keys.split_off(mid);
                    let right_vals = vals.split_off(mid);
                    let promoted   = right_keys[0].clone();
                    let right      = Node::Leaf { keys: right_keys, vals: right_vals };
                    InsertResult::Split(promoted, right)
                } else {
                    InsertResult::Ok
                }
            }
            Node::Internal { keys, children } => {
                let pos = keys.partition_point(|k| k <= &key);
                match children[pos].insert(key, val) {
                    InsertResult::Ok => InsertResult::Ok,
                    InsertResult::Split(promoted, right_child) => {
                        keys.insert(pos, promoted);
                        children.insert(pos + 1, right_child);

                        if keys.len() >= ORDER {
                            // Split internal node.
                            let mid            = keys.len() / 2;
                            let promoted       = keys[mid].clone();
                            let right_keys     = keys.split_off(mid + 1);
                            keys.pop(); // remove promoted key from left node
                            let right_children = children.split_off(mid + 1);
                            let right          = Node::new_internal(right_keys, right_children);
                            InsertResult::Split(promoted, right)
                        } else {
                            InsertResult::Ok
                        }
                    }
                }
            }
        }
    }

    /// Remove a key, return value if found.
    fn remove(&mut self, key: &Key) -> RemoveResult {
        match self {
            Node::Leaf { keys, vals } => {
                let pos = keys.partition_point(|k| k < key);
                if pos < keys.len() && &keys[pos] == key {
                    keys.remove(pos);
                    RemoveResult::Ok(Some(vals.remove(pos)))
                } else {
                    RemoveResult::NotFound
                }
            }
            Node::Internal { keys, children } => {
                let pos = keys.partition_point(|k| k <= key);
                children[pos].remove(key)
            }
        }
    }

    /// Collect all key-value pairs in sorted order.
    fn collect_all<'a>(&'a self, out: &mut Vec<(&'a Key, &'a Value)>) {
        match self {
            Node::Leaf { keys, vals } => {
                for (k, v) in keys.iter().zip(vals.iter()) {
                    out.push((k, v));
                }
            }
            Node::Internal { children, .. } => {
                for child in children {
                    child.collect_all(out);
                }
            }
        }
    }

    /// Collect pairs within key range [start, end] inclusive.
    fn collect_range<'a>(&'a self, start: &Key, end: &Key, out: &mut Vec<(&'a Key, &'a Value)>) {
        match self {
            Node::Leaf { keys, vals } => {
                for (k, v) in keys.iter().zip(vals.iter()) {
                    if k >= start && k <= end {
                        out.push((k, v));
                    }
                }
            }
            Node::Internal { keys, children } => {
                // Prune subtrees that cannot possibly overlap [start, end].
                // child[i] covers keys in:
                //   i == 0              → keys < separator[0]
                //   0 < i < n          → separator[i-1] <= keys < separator[i]
                //   i == children.len-1 → keys >= separator[last]
                for (i, child) in children.iter().enumerate() {
                    let above_min = if i == 0 {
                        true
                    } else {
                        keys.get(i - 1).map_or(true, |k| k <= end)
                    };
                    let below_max = keys.get(i).map_or(true, |k| k >= start);
                    if above_min && below_max {
                        child.collect_range(start, end, out);
                    }
                }
            }
        }
    }
}

// ─── Public tree ─────────────────────────────────────────────────────────────

pub struct BPlusTree {
    root: Node,
    size: usize,
}

impl BPlusTree {
    pub fn new() -> Self {
        Self { root: Node::new_leaf(), size: 0 }
    }

    pub fn len(&self) -> usize   { self.size }
    pub fn is_empty(&self) -> bool { self.size == 0 }

    // ── Mutation ─────────────────────────────────────────────────────────────

    /// Insert (or update) a file/directory entry under `parent_ino`.
    pub fn insert(&mut self, parent_ino: u64, name: &str, value: Value) {
        let key            = Key::new(parent_ino, name);
        let already_exists = self.root.get(&key).is_some();

        match self.root.insert(key, value) {
            InsertResult::Ok => {}
            InsertResult::Split(promoted, right) => {
                // Root split — create a new root one level above.
                let old_root = std::mem::replace(&mut self.root, Node::new_leaf());
                self.root = Node::new_internal(
                    vec![promoted],
                    vec![old_root, right],
                );
            }
        }

        if !already_exists {
            self.size += 1;
        }
    }

    /// Remove an entry by (parent_ino, name), returning its value if found.
    pub fn remove(&mut self, parent_ino: u64, name: &str) -> Option<Value> {
        match self.root.remove(&Key::new(parent_ino, name)) {
            RemoveResult::Ok(val) => {
                if val.is_some() { self.size -= 1; }
                val
            }
            RemoveResult::NotFound => None,
        }
    }

    // ── Lookup ───────────────────────────────────────────────────────────────

    /// Look up a single entry by (parent_ino, name).
    pub fn get(&self, parent_ino: u64, name: &str) -> Option<&Value> {
        self.root.get(&Key::new(parent_ino, name))
    }

    // ── Scans ────────────────────────────────────────────────────────────────

    /// List all direct children of a directory — O(log n + k) range scan.
    ///
    /// Returns entries sorted by name (the B+ tree ordering guarantee).
    /// Directories appear mixed with files; sort by `v.is_dir` after if needed.
    pub fn list_dir(&self, parent_ino: u64) -> Vec<(&Key, &Value)> {
        let mut out = vec![];
        self.root.collect_range(
            &Key::dir_start(parent_ino),
            &Key::dir_end(parent_ino),
            &mut out,
        );
        out
    }

    /// Collect every entry in the tree in sorted order.
    /// Used by the search / status CLI commands which need all files.
    pub fn list_all(&self) -> Vec<(&Key, &Value)> {
        let mut out = vec![];
        self.root.collect_all(&mut out);
        out
    }

    /// Range scan by raw Key objects (retained for legacy callers).
    pub fn range_keys(&self, start: &Key, end: &Key) -> Vec<(&Key, &Value)> {
        let mut out = vec![];
        self.root.collect_range(start, end, &mut out);
        out
    }
}

// ─── Default impl ────────────────────────────────────────────────────────────

impl Default for BPlusTree {
    fn default() -> Self { Self::new() }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn val(ino: u64) -> Value {
        Value { ino, size: 0, is_dir: false, disk_index: 0 }
    }

    fn dir_val(ino: u64) -> Value {
        Value { ino, size: 0, is_dir: true, disk_index: 0 }
    }

    // ── Key ordering ─────────────────────────────────────────────────────────

    #[test]
    fn test_key_ordering_by_parent_first() {
        let k1 = Key::new(1, "zzz.txt");
        let k2 = Key::new(2, "aaa.txt");
        // Even though "zzz" > "aaa" lexicographically, parent_ino wins.
        assert!(k1 < k2);
    }

    #[test]
    fn test_key_ordering_same_parent_alphabetical() {
        let k1 = Key::new(1, "apple.txt");
        let k2 = Key::new(1, "zebra.txt");
        assert!(k1 < k2);
    }

    #[test]
    fn test_key_equality() {
        assert_eq!(Key::new(3, "file.rs"), Key::new(3, "file.rs"));
        assert_ne!(Key::new(3, "file.rs"), Key::new(4, "file.rs"));
        assert_ne!(Key::new(3, "file.rs"), Key::new(3, "file.txt"));
    }

    // ── Basic insert / lookup ─────────────────────────────────────────────────

    #[test]
    fn test_insert_and_lookup() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "hello.txt", val(2));
        tree.insert(1, "world.txt", val(3));
        tree.insert(1, "readme.md", val(4));
        assert_eq!(tree.get(1, "hello.txt").unwrap().ino, 2);
        assert_eq!(tree.get(1, "world.txt").unwrap().ino, 3);
        assert_eq!(tree.get(1, "readme.md").unwrap().ino, 4);
        assert!(tree.get(1, "missing.txt").is_none());
    }

    #[test]
    fn test_insert_different_parents_same_name() {
        // Same filename under three different parent directories.
        let mut tree = BPlusTree::new();
        tree.insert(1, "main.rs", val(10));
        tree.insert(2, "main.rs", val(20));
        tree.insert(3, "main.rs", val(30));
        assert_eq!(tree.get(1, "main.rs").unwrap().ino, 10);
        assert_eq!(tree.get(2, "main.rs").unwrap().ino, 20);
        assert_eq!(tree.get(3, "main.rs").unwrap().ino, 30);
    }

    // ── Sorted listing ────────────────────────────────────────────────────────

    #[test]
    fn test_sorted_listing() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "zebra.txt", val(2));
        tree.insert(1, "apple.txt", val(3));
        tree.insert(1, "mango.txt", val(4));
        let all = tree.list_all();
        // Within parent_ino=1 the order must be alphabetical.
        assert_eq!(all[0].0.1, "apple.txt");
        assert_eq!(all[1].0.1, "mango.txt");
        assert_eq!(all[2].0.1, "zebra.txt");
    }

    // ── Delete ────────────────────────────────────────────────────────────────

    #[test]
    fn test_delete() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "hello.txt", val(2));
        tree.insert(1, "world.txt", val(3));
        assert!(tree.remove(1, "hello.txt").is_some());
        assert!(tree.get(1, "hello.txt").is_none());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_delete_only_correct_parent() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "doc.txt", val(5));
        tree.insert(2, "doc.txt", val(6));
        tree.remove(1, "doc.txt");
        assert!(tree.get(1, "doc.txt").is_none());
        // The same name under parent 2 must be unaffected.
        assert_eq!(tree.get(2, "doc.txt").unwrap().ino, 6);
    }

    // ── list_dir ──────────────────────────────────────────────────────────────

    #[test]
    fn test_list_dir_returns_only_children() {
        let mut tree = BPlusTree::new();
        // Root directory (ino=1) contains three files.
        tree.insert(1, "alpha.txt", val(10));
        tree.insert(1, "beta.txt",  val(11));
        tree.insert(1, "gamma.txt", val(12));
        // A subdirectory (ino=2) contains two files.
        tree.insert(2, "deep_a.txt", val(20));
        tree.insert(2, "deep_b.txt", val(21));

        let root_children = tree.list_dir(1);
        assert_eq!(root_children.len(), 3);
        let names: Vec<&str> = root_children.iter().map(|(k, _)| k.1.as_str()).collect();
        assert!(names.contains(&"alpha.txt"));
        assert!(names.contains(&"beta.txt"));
        assert!(names.contains(&"gamma.txt"));

        let sub_children = tree.list_dir(2);
        assert_eq!(sub_children.len(), 2);
        assert!(sub_children.iter().any(|(k, _)| k.1 == "deep_a.txt"));
    }

    #[test]
    fn test_list_dir_empty() {
        let tree = BPlusTree::new();
        assert!(tree.list_dir(99).is_empty());
    }

    #[test]
    fn test_list_dir_sorted() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "z_last.txt",  val(30));
        tree.insert(1, "a_first.txt", val(10));
        tree.insert(1, "m_mid.txt",   val(20));
        let dir = tree.list_dir(1);
        assert_eq!(dir[0].0.1, "a_first.txt");
        assert_eq!(dir[1].0.1, "m_mid.txt");
        assert_eq!(dir[2].0.1, "z_last.txt");
    }

    // ── Large insert / stress ─────────────────────────────────────────────────

    #[test]
    fn test_large_insert() {
        let mut tree = BPlusTree::new();
        for i in 0..500 {
            tree.insert(1, &format!("file_{:04}.txt", i), val(i as u64 + 2));
        }
        assert_eq!(tree.len(), 500);
        assert_eq!(tree.get(1, "file_0250.txt").unwrap().ino, 252);
    }

    #[test]
    fn test_multi_dir_large() {
        let mut tree = BPlusTree::new();
        // 5 directories, 100 files each.
        for dir in 2..7u64 {
            for file in 0..100u64 {
                let ino = dir * 1000 + file;
                tree.insert(dir, &format!("f{:03}.txt", file), val(ino));
            }
        }
        assert_eq!(tree.len(), 500);
        for dir in 2..7u64 {
            let children = tree.list_dir(dir);
            assert_eq!(children.len(), 100, "dir {dir} should have 100 children");
        }
    }

    // ── Update existing ───────────────────────────────────────────────────────

    #[test]
    fn test_update_existing() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "file.txt", val(2));
        tree.insert(1, "file.txt", val(99));
        assert_eq!(tree.get(1, "file.txt").unwrap().ino, 99);
        assert_eq!(tree.len(), 1); // count must not double-count an update
    }

    // ── Hierarchical directory structure ──────────────────────────────────────

    #[test]
    fn test_nested_hierarchy() {
        let mut tree = BPlusTree::new();
        // /src  (ino=2, parent=1)
        tree.insert(1, "src", dir_val(2));
        // /src/lib.rs  (ino=10, parent=2)
        tree.insert(2, "lib.rs", val(10));
        // /src/main.rs (ino=11, parent=2)
        tree.insert(2, "main.rs", val(11));
        // /docs  (ino=3, parent=1)
        tree.insert(1, "docs", dir_val(3));
        // /docs/readme.md (ino=20, parent=3)
        tree.insert(3, "readme.md", val(20));

        // Root should have exactly two children: src and docs.
        assert_eq!(tree.list_dir(1).len(), 2);
        // /src should have exactly two files.
        assert_eq!(tree.list_dir(2).len(), 2);
        // /docs should have exactly one file.
        assert_eq!(tree.list_dir(3).len(), 1);

        // Cross-tree lookup.
        assert_eq!(tree.get(2, "lib.rs").unwrap().ino, 10);
        assert_eq!(tree.get(3, "readme.md").unwrap().ino, 20);
    }

    // ── is_dir flag propagation ───────────────────────────────────────────────

    #[test]
    fn test_is_dir_flag() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "mydir",    dir_val(5));
        tree.insert(1, "myfile.txt", val(6));
        assert!( tree.get(1, "mydir").unwrap().is_dir);
        assert!(!tree.get(1, "myfile.txt").unwrap().is_dir);
    }

    // ── range_keys (legacy API) ───────────────────────────────────────────────

    #[test]
    fn test_range_keys_within_dir() {
        let mut tree = BPlusTree::new();
        tree.insert(1, "a.txt", val(1));
        tree.insert(1, "b.txt", val(2));
        tree.insert(1, "c.txt", val(3));
        tree.insert(1, "z.txt", val(26));
        let r = tree.range_keys(&Key::new(1, "a.txt"), &Key::new(1, "c.txt"));
        assert_eq!(r.len(), 3);
    }
}
