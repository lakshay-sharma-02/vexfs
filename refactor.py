import re

with open('src/fuse/mod.rs', 'r', encoding='utf-8') as f:
    code = f.read()

# Replace flush_all's AI persist 
code = re.sub(
    r'let _ = self\.ai_persist\.save\(.*?println!\("VexFS AI: state saved to disk.*?\);',
    '// AI State is persisted implicitly via engine, but we could add an explicit flush event if needed.',
    code,
    flags=re.DOTALL
)

# Replace ai_on_open
code = re.sub(
    r'fn ai_on_open\(&mut self, ino: u64, name: &str, size: u64\) \{.*?\}',
    '''fn ai_on_open(&mut self, ino: u64, name: &str, size: u64) {
        let _ = self.ai_tx.send(FsEvent::Open { ino, name: name.to_string(), size });
    }''',
    code,
    flags=re.DOTALL
)

# Replace ai_on_close
code = re.sub(
    r'fn ai_on_close\(&mut self, ino: u64, name: &str\) \{.*?\}',
    '''fn ai_on_close(&mut self, ino: u64, name: &str) {
        let duration = self.files.get(&ino)
            .and_then(|f| f.open_since)
            .map(|t| Self::now_secs().saturating_sub(t))
            .unwrap_or(0);
        let _ = self.ai_tx.send(FsEvent::Close { ino, name: name.to_string(), duration });
    }''',
    code,
    flags=re.DOTALL
)

# Remove search
code = re.sub(
    r'pub fn search\(&self, query: &str\) -> Vec<\(String, f32, Vec<String>\)> \{.*?\}',
    '',
    code,
    flags=re.DOTALL
)

# Replace ai_status
code = re.sub(
    r'pub fn ai_status\(&self\) \{.*?println!.*"=======================\\n"\);\n\s+\}',
    '''pub fn ai_status(&self) {
        let state = self.ai_state.read().unwrap();
        println!("\\n=== VexFS AI Status ===");
        println!("Markov entries:  {}", state.markov_entries);
        println!("Search indexed:  {}", state.search_indexed);
        println!("Snapshots total: {}", self.snapshots.total_snapshots());
        println!("Cache used:      {:.1} MB / {:.1} MB",
            self.cache.used_bytes() as f64 / 1_048_576.0,
            self.cache.max_bytes() as f64 / 1_048_576.0);
        
        if !state.ranked_files.is_empty() {
            println!("\\nTop files:");
            for (name, score, tier) in state.ranked_files.iter().take(5) {
                println!("  [{}] {} score={:.2}", tier, name, score);
            }
        }
        println!("=======================\\n");
    }''',
    code,
    flags=re.DOTALL
)

# Replace run_ask_query
code = re.sub(
    r'fn run_ask_query\(&mut self, question: &str\) \{.*?\}\n\}\nimpl Filesystem for VexFS \{',
    r'}\nimpl Filesystem for VexFS {',
    code,
    flags=re.DOTALL
)

# Replace the write() AI logic
code = re.sub(
    r'// --- Entropy / ransomware check ---.*?match threat \{.*?\n\s+\}\n\s+\}',
    '''// --- Entropy / ransomware check ---
        let mut data_vec = data.to_vec();
        let _ = self.ai_tx.send(FsEvent::Write { ino, name: name.clone(), data: data_vec });''',
    code,
    flags=re.DOTALL
)

# Remove sync search index call
code = re.sub(
    r'self\.search\.index\(ino, &name, &new_data, Self::now_secs\(\)\);',
    '',
    code,
)

# Remove sync log record in write
code = re.sub(
    r'self\.log\.record\(AccessEvent::now\(ino, &name, AccessKind::Write, data\.len\(\) as u64\)\);',
    '',
    code,
)

# Remove sync delete calls in unlink
code = re.sub(
    r'self\.search\.remove\(ino\);\n\s+self\.snapshots\.remove_file\(ino\);\n\s+self\.cache\.remove\(ino\);\n\s+self\.write_buffer\.take\(ino\);\n\s+self\.log\.record\(AccessEvent::now\(ino, &name_str, AccessKind::Delete, 0\)\);',
    '''self.snapshots.remove_file(ino);
            self.cache.remove(ino);
            self.write_buffer.take(ino);
            let _ = self.ai_tx.send(FsEvent::Delete { ino, name: name_str.clone() });''',
    code
)

# Remove sync search calls in rename
code = re.sub(
    r'self\.search\.remove\(dst_ino\);',
    '',
    code,
)
code = re.sub(
    r'// Re-index in search under new name.*?self\.search\.index\(src_ino, &dst, &cached_data, Self::now_secs\(\)\);',
    '',
    code,
    flags=re.DOTALL
)

# Replace ASK_INO in read
code = re.sub(
    r'// Virtual \.vexfs-ask: return last LLM/semantic answer.*?// Virtual \.vexfs-search: return last search results',
    '''// Virtual .vexfs-ask: return last LLM/semantic answer
        if ino == ASK_INO {
            let state = self.ai_state.read().unwrap();
            let start = offset as usize;
            let end = (start + size as usize).min(state.ask_result.len());
            if start < state.ask_result.len() {
                reply.data(&state.ask_result[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }

        // Virtual .vexfs-search: return last search results''',
    code,
    flags=re.DOTALL
)

# Replace SEARCH_INO in read
code = re.sub(
    r'if ino == SEARCH_INO \{.*?return;\n\s+\}',
    '''if ino == SEARCH_INO {
            let state = self.ai_state.read().unwrap();
            let start = offset as usize;
            let end = (start + size as usize).min(state.search_result.len());
            if start < state.search_result.len() {
                reply.data(&state.search_result[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }''',
    code,
    flags=re.DOTALL
)

# Replace TELEMETRY_INO in read
code = re.sub(
    r'// Collect ranked files.*?let ranked = self\.importance\.ranked_files.*?\)\.collect::<Vec<_>>\(\)\.join\(","\);',
    '''// Collect ranked files
            let state = self.ai_state.read().unwrap();
            let ranked = state.ranked_files.iter().take(10).map(|(name, score, tier)| {
                format!(r#"{{"name":"{}","score":{},"tier":"{}"}}"#, name, score, tier)
            }).collect::<Vec<_>>().join(",");''',
    code,
    flags=re.DOTALL
)
code = code.replace(
    '''self.cache.used_bytes(),
                self.cache.max_bytes(),
                self.markov.entry_count(),
                self.search.indexed_count(),
                self.snapshots.total_snapshots(),
                self.entropy_guard.threat_count,''',
    '''self.cache.used_bytes(),
                self.cache.max_bytes(),
                state.markov_entries,
                state.search_indexed,
                self.snapshots.total_snapshots(),
                state.entropy_threats,'''
)

# Replace ASK_INO in write
code = re.sub(
    r'// Virtual \.vexfs-ask: interpret write as a natural-language question.*?// Virtual \.vexfs-search: interpret write as a search query',
    '''// Virtual .vexfs-ask: interpret write as a natural-language question
        if ino == ASK_INO {
            let question = String::from_utf8_lossy(data).trim().to_string();
            if !question.is_empty() {
                self.ask_query = question.clone();
                let file_list: Vec<String> = self.files.values().take(50).map(|f| f.name.clone()).collect();
                let _ = self.ai_tx.send(FsEvent::AskQuery { query: question, file_list });
            }
            reply.written(data.len() as u32);
            return;
        }

        // Virtual .vexfs-search: interpret write as a search query''',
    code,
    flags=re.DOTALL
)

# Replace SEARCH_INO in write
code = re.sub(
    r'// Virtual \.vexfs-search: interpret write as a search query.*?self\.search_result = out\.into_bytes\(\);\n\s+println!.*?\n\s+\}\n\s+reply\.written\(data\.len\(\) as u32\);\n\s+return;\n\s+\}',
    '''// Virtual .vexfs-search: interpret write as a search query
        if ino == SEARCH_INO {
            let query = String::from_utf8_lossy(data).trim().to_string();
            if !query.is_empty() {
                self.search_query = query.clone();
                let _ = self.ai_tx.send(FsEvent::SearchQuery { query });
            }
            reply.written(data.len() as u32);
            return;
        }''',
    code,
    flags=re.DOTALL
)

# Sync search/auth inside getattr search_file_attr, ask_file_attr calls
# But we made sure in read() it goes through state, for getattr size it might be slightly off until syncd, but it's fine for virtual files

with open('src/fuse/mod.rs', 'w', encoding='utf-8') as f:
    f.write(code)
