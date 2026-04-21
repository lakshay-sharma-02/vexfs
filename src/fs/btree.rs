//! B+ Tree — metadata index for VexFS
//! Powers all file lookups, directory listings, and range scans.

const ORDER: usize = 8; // max children per internal node

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(pub String);

impl Key {
    pub fn new(s: &str) -> Self { Key(s.to_string()) }
}

#[derive(Debug, Clone)]
pub struct Value {
    pub ino: u64,
    pub size: u64,
    pub is_dir: bool,
    pub disk_index: usize,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf {
        keys: Vec<Key>,
        vals: Vec<Value>,
    },
    Internal {
        keys: Vec<Key>,       // separator keys
        children: Vec<Node>,  // len = keys.len() + 1
    },
}

/// Result of inserting into a node — may produce a split
enum InsertResult {
    Ok,
    Split(Key, Node), // promoted key + new right node
}

/// Result of removing from a node
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
            Node::Leaf { keys, .. } => keys.len(),
            Node::Internal { keys, .. } => keys.len(),
        }
    }

    fn is_full(&self) -> bool {
        self.key_count() >= ORDER - 1
    }

    /// Search for a value by key
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

    /// Insert key-value, return split info if node overflows
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
                    // Split leaf
                    let mid = keys.len() / 2;
                    let right_keys = keys.split_off(mid);
                    let right_vals = vals.split_off(mid);
                    let promoted = right_keys[0].clone();
                    let right = Node::Leaf { keys: right_keys, vals: right_vals };
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
                            // Split internal node
                            let mid = keys.len() / 2;
                            let promoted = keys[mid].clone();
                            let right_keys = keys.split_off(mid + 1);
                            keys.pop(); // remove promoted key
                            let right_children = children.split_off(mid + 1);
                            let right = Node::new_internal(right_keys, right_children);
                            InsertResult::Split(promoted, right)
                        } else {
                            InsertResult::Ok
                        }
                    }
                }
            }
        }
    }

    /// Remove a key, return value if found
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

    /// Collect all key-value pairs in sorted order
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

    /// Collect pairs within key range [start, end]
    fn collect_range<'a>(&'a self, start: &Key, end: &Key, out: &mut Vec<(&'a Key, &'a Value)>) {
        match self {
            Node::Leaf { keys, vals } => {
                for (k, v) in keys.iter().zip(vals.iter()) {
                    if k >= start && k <= end {
                        out.push((k, v));
                    }
                }
            }
            Node::Internal { children, .. } => {
                for child in children {
                    child.collect_range(start, end, out);
                }
            }
        }
    }
}

pub struct BPlusTree {
    root: Node,
    size: usize,
}

impl BPlusTree {
    pub fn new() -> Self {
        Self { root: Node::new_leaf(), size: 0 }
    }

    pub fn len(&self) -> usize { self.size }
    pub fn is_empty(&self) -> bool { self.size == 0 }

    pub fn insert(&mut self, name: &str, value: Value) {
        let key = Key::new(name);
        let already_exists = self.root.get(&key).is_some();

        match self.root.insert(key, value) {
            InsertResult::Ok => {}
            InsertResult::Split(promoted, right) => {
                // Root split — create new root
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

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.root.get(&Key::new(name))
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        match self.root.remove(&Key::new(name)) {
            RemoveResult::Ok(val) => {
                if val.is_some() { self.size -= 1; }
                val
            }
            RemoveResult::NotFound => None,
        }
    }

    pub fn list_all(&self) -> Vec<(&Key, &Value)> {
        let mut out = vec![];
        self.root.collect_all(&mut out);
        out
    }

    pub fn range(&self, start: &str, end: &str) -> Vec<(&Key, &Value)> {
        let mut out = vec![];
        self.root.collect_range(&Key::new(start), &Key::new(end), &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(ino: u64) -> Value {
        Value { ino, size: 0, is_dir: false, disk_index: 0 }
    }

    #[test]
    fn test_insert_and_lookup() {
        let mut tree = BPlusTree::new();
        tree.insert("hello.txt", val(2));
        tree.insert("world.txt", val(3));
        tree.insert("readme.md", val(4));
        assert_eq!(tree.get("hello.txt").unwrap().ino, 2);
        assert_eq!(tree.get("world.txt").unwrap().ino, 3);
        assert_eq!(tree.get("readme.md").unwrap().ino, 4);
        assert!(tree.get("missing.txt").is_none());
    }

    #[test]
    fn test_sorted_listing() {
        let mut tree = BPlusTree::new();
        tree.insert("zebra.txt", val(2));
        tree.insert("apple.txt", val(3));
        tree.insert("mango.txt", val(4));
        let all = tree.list_all();
        assert_eq!(all[0].0.0, "apple.txt");
        assert_eq!(all[1].0.0, "mango.txt");
        assert_eq!(all[2].0.0, "zebra.txt");
    }

    #[test]
    fn test_delete() {
        let mut tree = BPlusTree::new();
        tree.insert("hello.txt", val(2));
        tree.insert("world.txt", val(3));
        assert!(tree.remove("hello.txt").is_some());
        assert!(tree.get("hello.txt").is_none());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_range_scan() {
        let mut tree = BPlusTree::new();
        tree.insert("a_file.txt", val(2));
        tree.insert("b_file.txt", val(3));
        tree.insert("c_file.txt", val(4));
        tree.insert("z_file.txt", val(5));
        let range = tree.range("a_file.txt", "c_file.txt");
        assert_eq!(range.len(), 3);
    }

    #[test]
    fn test_large_insert() {
        let mut tree = BPlusTree::new();
        for i in 0..500 {
            tree.insert(&format!("file_{:04}.txt", i), val(i as u64 + 2));
        }
        assert_eq!(tree.len(), 500);
        assert_eq!(tree.get("file_0250.txt").unwrap().ino, 252);
    }

    #[test]
    fn test_update_existing() {
        let mut tree = BPlusTree::new();
        tree.insert("file.txt", val(2));
        tree.insert("file.txt", val(99));
        assert_eq!(tree.get("file.txt").unwrap().ino, 99);
        assert_eq!(tree.len(), 1);
    }
}
