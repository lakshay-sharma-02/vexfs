import re

# 1. Fix src/ai/engine.rs
with open('src/ai/engine.rs', 'r', encoding='utf-8') as f:
    engine_code = f.read()
engine_code = engine_code.replace(
    'w.entropy_threats = self.entropy_guard.threat_count;',
    'w.entropy_threats = self.entropy_guard.threat_count as usize;'
)
with open('src/ai/engine.rs', 'w', encoding='utf-8') as f:
    f.write(engine_code)


# 2. Fix src/fuse/mod.rs
with open('src/fuse/mod.rs', 'r', encoding='utf-8') as f:
    fuse_code = f.read()

# Fix SEARCH_INO inside write()
bad_search_ino_block = '''        // Virtual .vexfs-search: interpret write as a search query
        if ino == SEARCH_INO {
            let state = self.ai_state.read().unwrap();
            let start = offset as usize;
            let end = (start + size as usize).min(state.search_result.len());
            if start < state.search_result.len() {
                reply.data(&state.search_result[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }'''
good_search_ino_block = '''        // Virtual .vexfs-search: interpret write as a search query
        if ino == SEARCH_INO {
            let query = String::from_utf8_lossy(data).trim().to_string();
            if !query.is_empty() {
                self.search_query = query.clone();
                let _ = self.ai_tx.send(FsEvent::SearchQuery { query });
            }
            reply.written(data.len() as u32);
            return;
        }'''
fuse_code = fuse_code.replace(bad_search_ino_block, good_search_ino_block)

# Remove unused variables warnings in read()
fuse_code = fuse_code.replace(
    'let fname = file.name.clone();\n            let fsize = file.attr.size;',
    '// removed unused fname/fsize'
)

# Fix unlink() -> self.search.remove(ino); -> send FsEvent::Delete
fuse_code = fuse_code.replace(
    'self.search.remove(ino);',
    'let _ = self.ai_tx.send(FsEvent::Delete { ino, name: name_str.clone() });'
)

# Fix rename() -> self.search.remove(dst_ino) and search.index
fuse_code = re.sub(
    r'self\.search\.remove\(dst_ino\);',
    'let _ = self.ai_tx.send(FsEvent::Delete { ino: dst_ino, name: dst.clone() });',
    fuse_code
)
fuse_code = re.sub(
    r'self\.search\.index\(src_ino, &dst, &cached_data, Self::now_secs\(\)\);',
    'let _ = self.ai_tx.send(FsEvent::Write { ino: src_ino, name: dst.clone(), data: cached_data });',
    fuse_code
)

with open('src/fuse/mod.rs', 'w', encoding='utf-8') as f:
    f.write(fuse_code)
