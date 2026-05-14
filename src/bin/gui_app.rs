//! gui_app — premium egui desktop explorer for VexFS.
//!
//! Features:
//!   • Auto-launched by `vexfs gui <image>` — no manual terminal work
//!   • Files tab: Create / Delete buttons, Quick Edit (inline text editor)
//!   • "Ask AI about this file" context button on each file row
//!   • Search / Ask tabs integrated with the virtual file API
//!   • Glassmorphism dark aesthetic — feels like a native high-end app

use eframe::egui::{
    self, Color32, FontId, RichText, Rounding, Stroke, Vec2, Ui,
    ScrollArea, Sense, Rect, Align2, Align, Layout,
};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ══════════════════════════════════════════════════════════════════════════════
// Design tokens — tweak here, consistent everywhere
// ══════════════════════════════════════════════════════════════════════════════

const BG:         Color32 = Color32::from_rgb(10,  10,  18);   // near-black
const SURFACE:    Color32 = Color32::from_rgb(18,  18,  30);   // card bg
const SURFACE2:   Color32 = Color32::from_rgb(24,  24,  40);   // elevated
const BORDER:     Color32 = Color32::from_rgb(45,  45,  75);   // subtle divider
const ACCENT:     Color32 = Color32::from_rgb(99,  102, 241);  // indigo
const ACCENT_DIM: Color32 = Color32::from_rgb(55,  58,  160);  // pressed accent
const TEXT:       Color32 = Color32::from_rgb(220, 220, 240);  // primary text
const MUTED:      Color32 = Color32::from_rgb(110, 110, 145);  // secondary text
const HOT_BG:     Color32 = Color32::from_rgb(127, 29,  29);
const HOT_FG:     Color32 = Color32::from_rgb(252, 165, 165);
const WARM_BG:    Color32 = Color32::from_rgb(92,  65,  10);
const WARM_FG:    Color32 = Color32::from_rgb(253, 224, 132);
const COLD_BG:    Color32 = Color32::from_rgb(23,  52,  100);
const COLD_FG:    Color32 = Color32::from_rgb(147, 197, 253);
const SUCCESS:    Color32 = Color32::from_rgb(52,  211, 153);
const DANGER:     Color32 = Color32::from_rgb(248, 113, 113);
const WARN:       Color32 = Color32::from_rgb(251, 191, 36);

// ══════════════════════════════════════════════════════════════════════════════
// Telemetry
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
struct Telemetry {
    cache_used: u64,
    cache_max: u64,
    markov_entries: usize,
    search_indexed: usize,
    snapshots_total: usize,
    entropy_threats: usize,
    total_files: usize,
    ranked_files: Vec<RankedFile>,
    last_updated: Option<Instant>,
}

#[derive(Debug, Clone)]
struct RankedFile {
    name: String,
    score: f32,
    tier: String,
}

fn parse_telemetry(json: &str) -> Telemetry {
    let mut t = Telemetry::default();

    let get_u64 = |key: &str| -> u64 {
        let pat = format!("\"{}\":", key);
        json.find(&pat)
            .and_then(|i| {
                let rest = json[i + pat.len()..].trim_start_matches(|c: char| c == ' ');
                rest.split(|c: char| c == ',' || c == '}' || c == '\n')
                    .next()
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(0)
    };

    t.cache_used      = get_u64("cache_used");
    t.cache_max       = get_u64("cache_max");
    t.markov_entries  = get_u64("markov_entries") as usize;
    t.search_indexed  = get_u64("search_indexed") as usize;
    t.snapshots_total = get_u64("snapshots_total") as usize;
    t.entropy_threats = get_u64("entropy_threats") as usize;
    t.total_files     = get_u64("total_files") as usize;

    if let Some(arr_start) = json.find("\"ranked_files\":[") {
        let arr = &json[arr_start + 16..];
        for obj in arr.split('{').skip(1) {
            let get_str = |key: &str| -> String {
                let pat = format!("\"{}\":\"", key);
                obj.find(&pat)
                    .map(|i| obj[i + pat.len()..].split('"').next().unwrap_or("").to_string())
                    .unwrap_or_default()
            };
            let get_f32 = |key: &str| -> f32 {
                let pat = format!("\"{}\":", key);
                obj.find(&pat)
                    .and_then(|i| {
                        obj[i + pat.len()..]
                            .split(|c: char| c == ',' || c == '}')
                            .next()
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0.0)
            };
            let name = get_str("name");
            if name.is_empty() { continue; }
            t.ranked_files.push(RankedFile { name, score: get_f32("score"), tier: get_str("tier") });
        }
    }

    t.last_updated = Some(Instant::now());
    t
}

// ══════════════════════════════════════════════════════════════════════════════
// File entry
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct FileEntry {
    name: String,
    size: u64,
    is_dir: bool,
    tier: String,
    score: f32,
}

impl FileEntry {
    fn tier_label(&self) -> &'static str {
        if self.tier.contains("HOT")       { "HOT" }
        else if self.tier.contains("WARM") { "WARM" }
        else                               { "COLD" }
    }

    fn tier_colors(&self) -> (Color32, Color32) {
        match self.tier_label() {
            "HOT"  => (HOT_BG,  HOT_FG),
            "WARM" => (WARM_BG, WARM_FG),
            _      => (COLD_BG, COLD_FG),
        }
    }

    fn tier_bar_color(&self) -> Color32 {
        match self.tier_label() {
            "HOT"  => Color32::from_rgb(239, 68, 68),
            "WARM" => Color32::from_rgb(245, 158, 11),
            _      => Color32::from_rgb(99, 102, 241),
        }
    }

    fn size_str(&self) -> String {
        if self.is_dir { return "folder".into(); }
        if self.size < 1024 { format!("{} B", self.size) }
        else if self.size < 1_048_576 { format!("{:.1} KB", self.size as f64 / 1024.0) }
        else { format!("{:.1} MB", self.size as f64 / 1_048_576.0) }
    }

    fn is_text(&self) -> bool {
        let ext = self.name.rsplit('.').next().unwrap_or("").to_lowercase();
        matches!(ext.as_str(),
            "txt" | "md" | "rs" | "toml" | "yaml" | "yml" | "json" |
            "sh" | "py" | "js" | "ts" | "html" | "css" | "c" | "h" |
            "cpp" | "go" | "java" | "log" | "conf" | "ini" | "env" | "xml"
        )
    }

    fn icon(&self) -> &'static str {
        if self.is_dir { return "📁"; }
        let ext = self.name.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "rs"   => "🦀",
            "toml" => "⚙",
            "md"   => "📝",
            "txt"  => "📄",
            "sh"   => "⌨",
            "py"   => "🐍",
            "json" => "{}",
            "log"  => "📋",
            _      => "📄",
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Snapshot entry
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct SnapEntry {
    version: u32,
    name: String,
    size: u64,
    age: String,
}

// ══════════════════════════════════════════════════════════════════════════════
// UI state machines
// ══════════════════════════════════════════════════════════════════════════════

#[derive(PartialEq, Clone, Copy)]
enum Tab { Files, Dashboard, Search, Ask, Snapshots }

/// What the Files panel is showing right now.
#[derive(Clone)]
enum FilesView {
    /// Normal grid listing
    List,
    /// Inline editor for one file
    Edit {
        filename: String,
        content:  String,
        dirty:    bool,
        status:   String,
    },
}

// ══════════════════════════════════════════════════════════════════════════════
// App
// ══════════════════════════════════════════════════════════════════════════════

struct VexApp {
    mountpoint: PathBuf,
    image_path: Option<String>,
    daemon_url: String,

    telemetry:  Arc<Mutex<Telemetry>>,
    files:      Arc<Mutex<Vec<FileEntry>>>,

    tab:        Tab,
    files_view: FilesView,

    // ── New-file dialog
    new_file_name:    String,
    new_file_content: String,
    new_is_dir:       bool,
    show_new_dialog:  bool,
    new_file_status:  String,

    // ── Delete confirm
    delete_confirm: Option<String>,

    // ── Search
    search_input:   String,
    search_result:  String,
    search_pending: bool,

    // ── Ask
    ask_input:   String,
    ask_result:  String,
    ask_pending: bool,

    // ── Snapshots
    snap_entries: Vec<SnapEntry>,
    snap_filter:  String,
    snap_status:  String,

    // ── Dashboard sparkline
    cache_history: VecDeque<f32>,

    // ── Status bar
    status: String,

    // ── Polling timers
    last_file_scan:      Instant,
    last_telemetry_poll: Instant,
}

impl VexApp {
    fn new(mountpoint: PathBuf, image_path: Option<String>, daemon_url: String) -> Self {
        Self {
            mountpoint,
            image_path,
            daemon_url,
            telemetry:  Arc::new(Mutex::new(Telemetry::default())),
            files:      Arc::new(Mutex::new(vec![])),
            tab:        Tab::Files,
            files_view: FilesView::List,
            new_file_name:    String::new(),
            new_file_content: String::new(),
            new_is_dir:       false,
            show_new_dialog:  false,
            new_file_status:  String::new(),
            delete_confirm:   None,
            search_input:  String::new(),
            search_result: String::new(),
            search_pending: false,
            ask_input:   String::new(),
            ask_result:  String::new(),
            ask_pending: false,
            snap_entries: vec![],
            snap_filter:  String::new(),
            snap_status:  String::new(),
            cache_history: VecDeque::with_capacity(60),
            status:       "Ready  ·  VexFS Explorer".into(),
            last_file_scan:      Instant::now() - Duration::from_secs(10),
            last_telemetry_poll: Instant::now() - Duration::from_secs(10),
        }
    }

    // ── Background polling ─────────────────────────────────────────────────

    fn maybe_refresh_files(&mut self) {
        if self.last_file_scan.elapsed() < Duration::from_secs(3) { return; }
        self.last_file_scan = Instant::now();

        let mountpoint = self.mountpoint.clone();
        let files_arc  = Arc::clone(&self.files);
        let tel_arc    = Arc::clone(&self.telemetry);

        thread::spawn(move || {
            let ranked: Vec<(String, f32, String)> = {
                let t = tel_arc.lock().unwrap();
                t.ranked_files.iter().map(|r| (r.name.clone(), r.score, r.tier.clone())).collect()
            };

            let mut entries: Vec<FileEntry> = fs::read_dir(&mountpoint)
                .map(|rd| rd
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if name.starts_with(".vexfs-") { return None; }
                        let meta = e.metadata().ok()?;
                        let (tier, score) = ranked.iter()
                            .find(|(n, _, _)| n == &name)
                            .map(|(_, s, t)| (t.clone(), *s))
                            .unwrap_or_else(|| ("COLD".into(), 0.0));
                        Some(FileEntry {
                            name,
                            size: meta.len(),
                            is_dir: meta.is_dir(),
                            tier, score,
                        })
                    })
                    .collect())
                .unwrap_or_default();

            entries.sort_by(|a, b|
                b.is_dir.cmp(&a.is_dir)
                    .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            );
            *files_arc.lock().unwrap() = entries;
        });
    }

    fn maybe_poll_telemetry(&mut self) {
        if self.last_telemetry_poll.elapsed() < Duration::from_secs(2) { return; }
        self.last_telemetry_poll = Instant::now();

        let url     = format!("{}/api/telemetry", self.daemon_url);
        let tel_arc = Arc::clone(&self.telemetry);

        thread::spawn(move || {
            if let Some(body) = simple_get(&url) {
                *tel_arc.lock().unwrap() = parse_telemetry(&body);
            }
        });
    }

    // ── File operations ────────────────────────────────────────────────────

    fn open_editor(&mut self, filename: &str) {
        let path    = self.mountpoint.join(filename);
        let content = fs::read_to_string(&path).unwrap_or_default();
        self.files_view = FilesView::Edit {
            filename: filename.to_string(),
            content,
            dirty: false,
            status: format!("Editing  {filename}"),
        };
        self.status = format!("Quick Edit  ·  {filename}");
    }

    fn save_editor(&mut self) {
        if let FilesView::Edit { filename, content, dirty, status } = &mut self.files_view {
            let path = self.mountpoint.join(filename.clone());
            match fs::write(&path, content.as_bytes()) {
                Ok(_) => {
                    *dirty  = false;
                    *status = format!("Saved  ✓  {}", filename);
                    self.status = format!("Saved  ·  {filename}");
                }
                Err(e) => {
                    *status = format!("Error: {e}");
                }
            }
            // Invalidate file scan cache
            self.last_file_scan -= Duration::from_secs(10);
        }
    }

    fn create_new_file(&mut self) {
        let name = self.new_file_name.trim().to_string();
        if name.is_empty() {
            self.new_file_status = "Name cannot be empty.".into();
            return;
        }
        let path = self.mountpoint.join(&name);
        let res = if self.new_is_dir {
            fs::create_dir(&path)
        } else {
            fs::write(&path, self.new_file_content.as_bytes())
        };

        match res {
            Ok(_) => {
                let kind = if self.new_is_dir { "Directory" } else { "File" };
                self.new_file_status = format!("Created  ✓  {name}");
                self.show_new_dialog  = false;
                self.new_file_name    = String::new();
                self.new_file_content = String::new();
                self.last_file_scan  -= Duration::from_secs(10);
                self.status = format!("Created {kind}  ·  {name}");
            }
            Err(e) => {
                self.new_file_status = format!("Error: {e}");
            }
        }
    }

    fn delete_file(&mut self, filename: &str) {
        let path = self.mountpoint.join(filename);
        let meta = fs::metadata(&path);
        let res = if let Ok(m) = meta {
            if m.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) }
        } else {
            fs::remove_file(&path)
        };
        match res {
            Ok(_) => {
                self.status = format!("Deleted  ·  {filename}");
                self.last_file_scan -= Duration::from_secs(10);
            }
            Err(e) => {
                self.status = format!("Delete failed: {e}");
            }
        }
        self.delete_confirm = None;
    }

    fn do_search(&mut self) {
        let query = self.search_input.trim().to_string();
        if query.is_empty() { return; }
        self.search_pending = true;
        self.search_result  = "Searching…".into();

        let search_path = self.mountpoint.join(".vexfs-search");
        let q = query.clone();

        let result = (|| -> Option<String> {
            fs::write(&search_path, q.as_bytes()).ok()?;
            thread::sleep(Duration::from_millis(250));
            let mut f = fs::File::open(&search_path).ok()?;
            let mut out = String::new();
            f.read_to_string(&mut out).ok()?;
            Some(if out.trim().is_empty() {
                format!("No results for \"{q}\"")
            } else { out })
        })().unwrap_or_else(|| "Error: mount not accessible".into());

        self.search_result  = result;
        self.search_pending = false;
    }

    fn do_ask(&mut self) {
        let question = self.ask_input.trim().to_string();
        if question.is_empty() { return; }
        self.ask_pending = true;
        self.ask_result  = "Thinking…".into();

        let ask_path = self.mountpoint.join(".vexfs-ask");
        let q = question.clone();

        let result = (|| -> Option<String> {
            fs::write(&ask_path, q.as_bytes()).ok()?;
            thread::sleep(Duration::from_millis(400));
            let mut f = fs::File::open(&ask_path).ok()?;
            let mut out = String::new();
            f.read_to_string(&mut out).ok()?;
            Some(if out.trim().is_empty() {
                format!("No answer found for: {q}")
            } else { out })
        })().unwrap_or_else(|| "Error: mount not accessible".into());

        self.ask_result  = result;
        self.ask_pending = false;
    }

    fn ask_about_file(&mut self, filename: &str) {
        self.tab       = Tab::Ask;
        self.ask_input = format!("Summarise and explain the contents of the file: {filename}");
        self.do_ask();
    }

    // ── Snapshots ──────────────────────────────────────────────────────────

    fn load_snapshots(&mut self) {
        let Some(img) = &self.image_path else {
            self.snap_status = "No image path — pass it via the GUI launcher".into();
            return;
        };
        match Command::new("vexfs").args(["snapshot", "all", img]).output() {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                self.snap_entries = parse_snapshot_output(&text);
                self.snap_status  = format!("{} snapshot(s)", self.snap_entries.len());
            }
            Err(_) => {
                self.snap_status = "Could not run `vexfs` — ensure it is in PATH".into();
            }
        }
    }

    fn restore_snapshot(&mut self, name: &str, version: u32) {
        let Some(img) = &self.image_path else { return; };
        self.snap_status = match Command::new("vexfs")
            .args(["snapshot", "restore", img, name, &version.to_string()])
            .output()
        {
            Ok(o)  => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(e) => format!("Error: {e}"),
        };
    }

    // ══════════════════════════════════════════════════════════════════════
    // UI panels
    // ══════════════════════════════════════════════════════════════════════

    fn ui_topbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            // Logo mark
            ui.add_space(12.0);
            ui.label(
                RichText::new("⬡  VexFS")
                    .size(15.0)
                    .color(ACCENT)
                    .strong()
            );
            ui.add_space(20.0);

            // Nav tabs
            for (t, label, icon) in [
                (Tab::Files,     "Files",      "󰉋"),
                (Tab::Dashboard, "Dashboard",  "󰕰"),
                (Tab::Search,    "Search",     "󰍉"),
                (Tab::Ask,       "Ask AI",     "󰚩"),
                (Tab::Snapshots, "Snapshots",  "󰣏"),
            ] {
                let selected = self.tab == t;
                let (bg, fg) = if selected {
                    (ACCENT, Color32::WHITE)
                } else {
                    (Color32::TRANSPARENT, MUTED)
                };

                let btn = egui::Button::new(
                    RichText::new(format!("{}  {}", icon, label)).size(13.0).color(fg)
                )
                .fill(bg)
                .stroke(if selected { Stroke::new(0.0, Color32::TRANSPARENT) } else { Stroke::NONE })
                .rounding(Rounding::same(6.0))
                .min_size(Vec2::new(110.0, 28.0));

                if ui.add(btn).clicked() {
                    self.tab = t;
                    if t != Tab::Files { self.files_view = FilesView::List; }
                    if t == Tab::Snapshots { self.load_snapshots(); }
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);

                let tel = self.telemetry.lock().unwrap();
                let fresh = tel.last_updated
                    .map(|t| t.elapsed().as_secs() < 5)
                    .unwrap_or(false);
                let dot_color = if fresh { SUCCESS } else { MUTED };
                let age = tel.last_updated
                    .map(|t| {
                        let s = t.elapsed().as_secs();
                        if s < 5 { "live".into() } else { format!("{s}s ago") }
                    })
                    .unwrap_or_else(|| "offline".into());

                ui.label(RichText::new(format!("⬤  {age}")).size(11.0).color(dot_color));
                ui.add_space(8.0);

                if tel.entropy_threats > 0 {
                    ui.label(
                        RichText::new(format!("⚠  {} threat(s)", tel.entropy_threats))
                            .size(11.0).color(WARN).strong()
                    );
                }
            });
        });
    }

    fn ui_files(&mut self, ui: &mut Ui) {
        let files_view = self.files_view.clone();

        match files_view {
            FilesView::Edit { .. } => self.ui_editor(ui),
            FilesView::List       => self.ui_file_list(ui),
        }
    }

    fn ui_file_list(&mut self, ui: &mut Ui) {
        let files = self.files.lock().unwrap().clone();

        // ── Toolbar ───────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}   {}  file(s)", self.mountpoint.display(), files.len()))
                    .size(12.0).color(MUTED)
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if accent_button(ui, "+ New Item", 90.0, false).clicked() {
                    self.show_new_dialog = true;
                    self.new_file_status = String::new();
                }
            });
        });

        ui.add_space(6.0);

        // ── New-file dialog ───────────────────────────────────────────────
        if self.show_new_dialog {
            glass_card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Create New").size(13.0).color(TEXT).strong());
                    ui.add_space(8.0);
                    ui.radio_value(&mut self.new_is_dir, false, "File");
                    ui.radio_value(&mut self.new_is_dir, true, "Directory");
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name:").size(12.0).color(MUTED));
                    ui.add(egui::TextEdit::singleline(&mut self.new_file_name)
                        .desired_width(200.0)
                        .hint_text(if self.new_is_dir { "src" } else { "readme.md" })
                        .font(FontId::proportional(13.0)));
                });
                ui.add_space(4.0);
                if !self.new_is_dir {
                    ui.label(RichText::new("Initial content (optional):").size(12.0).color(MUTED));
                    ui.add(egui::TextEdit::multiline(&mut self.new_file_content)
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .font(FontId::monospace(12.0)));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if accent_button(ui, "Create", 70.0, false).clicked() {
                        self.create_new_file();
                    }
                    ui.add_space(6.0);
                    if danger_button(ui, "Cancel", 70.0).clicked() {
                        self.show_new_dialog  = false;
                        self.new_file_name    = String::new();
                        self.new_file_content = String::new();
                        self.new_file_status  = String::new();
                    }
                    if !self.new_file_status.is_empty() {
                        ui.add_space(8.0);
                        let ok = self.new_file_status.contains('✓');
                        ui.label(RichText::new(&self.new_file_status).size(12.0)
                            .color(if ok { SUCCESS } else { DANGER }));
                    }
                });
            });
            ui.add_space(8.0);
        }

        // ── Delete confirmation ───────────────────────────────────────────
        let mut do_delete: Option<String> = None;
        let mut clear_confirm = false;

        if let Some(ref name) = self.delete_confirm.clone() {
            glass_card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Delete  {}  ?", name)).size(13.0).color(WARN).strong());
                    ui.add_space(16.0);
                    if danger_button(ui, "Yes, delete", 90.0).clicked() {
                        do_delete = Some(name.clone());
                    }
                    ui.add_space(6.0);
                    if accent_button(ui, "Cancel", 70.0, false).clicked() {
                        clear_confirm = true;
                    }
                });
            });
            ui.add_space(6.0);
        }

        if clear_confirm { self.delete_confirm = None; }
        if let Some(name) = do_delete { self.delete_file(&name); }

        // ── Column headers ────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.add_space(32.0);
            header_label(ui, "Name",  320.0);
            header_label(ui, "Size",   80.0);
            header_label(ui, "Tier",   56.0);
            header_label(ui, "Score", 100.0);
            header_label(ui, "",      140.0); // actions
        });
        ui.add(egui::Separator::default().shrink(0.0));

        // ── File rows ─────────────────────────────────────────────────────
        let mut open_editor: Option<String> = None;
        let mut ask_ai:      Option<String> = None;
        let mut del_req:     Option<String> = None;

        ScrollArea::vertical()
            .id_salt("file_list")
            .max_height(ui.available_height() - 20.0)
            .show(ui, |ui| {
                if files.is_empty() {
                    ui.add_space(50.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No files — filesystem may not be mounted yet")
                            .size(14.0).color(MUTED));
                    });
                    return;
                }

                for (row, f) in files.iter().enumerate() {
                    let row_bg = if row % 2 == 0 { SURFACE } else { SURFACE2 };

                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 36.0),
                        Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, Rounding::same(4.0), row_bg);

                    #[allow(deprecated)]
                    let mut child = ui.child_ui(rect, Layout::left_to_right(Align::Center), None);

                    child.add_space(8.0);

                    // Icon
                    child.label(RichText::new(f.icon()).size(14.0));
                    child.add_space(4.0);

                    // Name — clickable to open editor
                    let name_resp = child.add(
                        egui::Label::new(
                            RichText::new(&f.name)
                                .size(13.0)
                                .color(if f.is_text() { Color32::from_rgb(165, 180, 252) } else { TEXT })
                        )
                        .sense(Sense::click())
                        .truncate()
                    );
                    if name_resp.hovered() { child.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                    if name_resp.clicked() && f.is_text() { open_editor = Some(f.name.clone()); }

                    // Pad to fixed width
                    let used = name_resp.rect.width() + 32.0;
                    let gap  = (320.0_f32 - used).max(4.0);
                    child.add_space(gap);

                    // Size
                    sized_label(&mut child, &f.size_str(), 80.0, MUTED);

                    // Tier badge
                    let (tbg, tfg) = f.tier_colors();
                    let (badge_rect, _) = child.allocate_exact_size(Vec2::new(52.0, 22.0), Sense::hover());
                    child.painter().rect_filled(badge_rect, Rounding::same(4.0), tbg);
                    child.painter().text(badge_rect.center(), Align2::CENTER_CENTER,
                        f.tier_label(), FontId::proportional(10.5), tfg);

                    child.add_space(4.0);

                    // Score bar
                    let bw = 90.0;
                    let (br, _) = child.allocate_exact_size(Vec2::new(bw, 10.0), Sense::hover());
                    child.painter().rect_filled(br, Rounding::same(3.0), Color32::from_gray(35));
                    if f.score > 0.0 {
                        let fill = Rect::from_min_size(br.min, Vec2::new(bw * f.score.min(1.0), br.height()));
                        child.painter().rect_filled(fill, Rounding::same(3.0), f.tier_bar_color());
                    }
                    child.add_space(4.0);
                    child.label(
                        RichText::new(format!("{:.2}", f.score)).size(10.5).color(MUTED)
                    );

                    child.add_space(8.0);

                    // Action buttons
                    if f.is_text() {
                        if ghost_button(&mut child, "Edit", 44.0).clicked() {
                            open_editor = Some(f.name.clone());
                        }
                        child.add_space(4.0);
                    }

                    if ghost_button(&mut child, "Ask AI", 52.0).clicked() {
                        ask_ai = Some(f.name.clone());
                    }
                    child.add_space(4.0);

                    if small_danger_button(&mut child, "✕", 24.0).clicked() {
                        del_req = Some(f.name.clone());
                    }
                }
            });

        // Apply deferred actions outside the borrow
        if let Some(name) = open_editor { self.open_editor(&name); }
        if let Some(name) = ask_ai      { self.ask_about_file(&name); }
        if let Some(name) = del_req     { self.delete_confirm = Some(name); }
    }

    fn ui_editor(&mut self, ui: &mut Ui) {
        let FilesView::Edit { filename, content: _, dirty, status } =
            &self.files_view
        else { return; };

        // Extract display values before the mutable borrow
        let filename_str = filename.clone();
        let is_dirty     = *dirty;
        let status_str   = status.clone();

        let mut do_back = false;
        let mut do_save = false;

        // Toolbar
        ui.horizontal(|ui| {
            if ghost_button(ui, "← Back", 70.0).clicked() { do_back = true; }
            ui.add_space(8.0);
            ui.label(RichText::new(filename_str.as_str()).size(13.0).color(TEXT).strong());
            if is_dirty {
                ui.label(RichText::new("●").size(12.0).color(WARN));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if !status_str.is_empty() {
                    let ok = status_str.contains('✓');
                    ui.label(RichText::new(status_str.as_str()).size(11.0)
                        .color(if ok { SUCCESS } else { MUTED }));
                    ui.add_space(8.0);
                }
                if accent_button(ui, "Save", 60.0, is_dirty).clicked() {
                    do_save = true;
                }
            });
        });

        // Apply deferred toolbar actions
        if do_back {
            self.files_view = FilesView::List;
            self.status = "Files".into();
            return;
        }
        if do_save {
            self.save_editor();
        }

        ui.add_space(6.0);
        ui.add(egui::Separator::default());
        ui.add_space(4.0);

        // Now we can exclusively borrow the content for the editor widget
        let FilesView::Edit { content, dirty, .. } = &mut self.files_view
        else { return; };

        let response = ScrollArea::vertical()
            .id_salt("editor_scroll")
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(content)
                        .desired_width(f32::INFINITY)
                        .desired_rows(30)
                        .font(FontId::monospace(13.0))
                        .text_color(Color32::from_rgb(200, 220, 200))
                        .frame(false)
                )
            });

        if response.inner.changed() {
            *dirty = true;
            if let FilesView::Edit { status, .. } = &mut self.files_view {
                *status = "Unsaved changes".into();
            }
        }
    }

    fn ui_dashboard(&mut self, ui: &mut Ui) {
        let tel = self.telemetry.lock().unwrap().clone();
        let cache_pct = if tel.cache_max > 0 {
            tel.cache_used as f32 / tel.cache_max as f32
        } else { 0.0 };

        drop(tel);
        self.cache_history.push_back(cache_pct);
        if self.cache_history.len() > 60 { self.cache_history.pop_front(); }

        let tel = self.telemetry.lock().unwrap().clone();

        ui.add_space(10.0);

        // ── Stat cards row ────────────────────────────────────────────────
        ui.horizontal(|ui| {
            stat_card(ui, "ARC Cache",
                &format!("{:.1} / {:.1} MB",
                    tel.cache_used as f64 / 1_048_576.0,
                    tel.cache_max  as f64 / 1_048_576.0),
                ACCENT, "");

            stat_card(ui, "Markov Entries",
                &tel.markov_entries.to_string(),
                Color32::from_rgb(167, 139, 250), "");

            stat_card(ui, "Search Index",
                &tel.search_indexed.to_string(),
                Color32::from_rgb(52, 211, 153), "");

            stat_card(ui, "Snapshots",
                &tel.snapshots_total.to_string(),
                Color32::from_rgb(251, 191, 36), "");

            stat_card(ui, "Files",
                &tel.total_files.to_string(),
                MUTED, "");

            let threat_color = if tel.entropy_threats > 0 { DANGER } else { MUTED };
            stat_card(ui, "Entropy Threats",
                &tel.entropy_threats.to_string(),
                threat_color, if tel.entropy_threats > 0 { "⚠" } else { "" });
        });

        ui.add_space(16.0);

        // ── Cache usage bar ───────────────────────────────────────────────
        ui.label(RichText::new("ARC Cache Usage").size(12.0).color(MUTED));
        ui.add_space(4.0);

        let bar_w = ui.available_width();
        let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 22.0), Sense::hover());
        ui.painter().rect_filled(bar_rect, Rounding::same(5.0), Color32::from_gray(28));

        if cache_pct > 0.0 {
            let bar_color = if cache_pct > 0.85 { DANGER }
                            else if cache_pct > 0.6 { WARN }
                            else { ACCENT };
            let fill = Rect::from_min_size(
                bar_rect.min, Vec2::new(bar_w * cache_pct.min(1.0), bar_rect.height())
            );
            ui.painter().rect_filled(fill, Rounding::same(5.0), bar_color);
        }
        ui.painter().text(bar_rect.center(), Align2::CENTER_CENTER,
            format!("{:.1}%", cache_pct * 100.0),
            FontId::proportional(11.5), Color32::WHITE);

        ui.add_space(16.0);

        // ── Top files ─────────────────────────────────────────────────────
        ui.label(RichText::new("Top Files by Importance").size(12.0).color(MUTED));
        ui.add_space(6.0);

        if tel.ranked_files.is_empty() {
            ui.label(
                RichText::new("Open some files to build the AI model")
                    .size(12.0).color(Color32::from_gray(80))
            );
        } else {
            ScrollArea::vertical().id_salt("ranked").max_height(160.0).show(ui, |ui| {
                for r in &tel.ranked_files {
                    let tier = if r.tier.contains("HOT") { "HOT" }
                               else if r.tier.contains("WARM") { "WARM" }
                               else { "COLD" };
                    let (tbg, tfg) = tier_badge_colors(tier);

                    ui.horizontal(|ui| {
                        let (br, _) = ui.allocate_exact_size(Vec2::new(44.0, 20.0), Sense::hover());
                        ui.painter().rect_filled(br, Rounding::same(4.0), tbg);
                        ui.painter().text(br.center(), Align2::CENTER_CENTER, tier,
                            FontId::proportional(10.0), tfg);
                        ui.add_space(6.0);
                        ui.label(RichText::new(&r.name).size(13.0).color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(8.0);
                            ui.label(RichText::new(format!("{:.2}", r.score))
                                .size(11.0).color(MUTED));
                            // mini score bar
                            let bw = 80.0;
                            let (bar, _) = ui.allocate_exact_size(Vec2::new(bw, 8.0), Sense::hover());
                            ui.painter().rect_filled(bar, Rounding::same(2.0), Color32::from_gray(35));
                            let fill_r = Rect::from_min_size(bar.min,
                                Vec2::new(bw * r.score.min(1.0), bar.height()));
                            let c = match tier {
                                "HOT"  => Color32::from_rgb(239, 68, 68),
                                "WARM" => Color32::from_rgb(245, 158, 11),
                                _      => ACCENT,
                            };
                            ui.painter().rect_filled(fill_r, Rounding::same(2.0), c);
                        });
                    });
                    ui.add_space(2.0);
                }
            });
        }

        // ── Sparkline ─────────────────────────────────────────────────────
        ui.add_space(16.0);
        ui.label(RichText::new("Cache History — last 60 polls").size(12.0).color(MUTED));
        ui.add_space(4.0);

        let spark_h = 56.0;
        let (spark_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), spark_h), Sense::hover()
        );
        ui.painter().rect_filled(spark_rect, Rounding::same(6.0), Color32::from_gray(18));

        let pts: Vec<egui::Pos2> = self.cache_history.iter().enumerate()
            .map(|(i, &v)| egui::pos2(
                spark_rect.min.x + (i as f32 / 59.0_f32.max(1.0)) * spark_rect.width(),
                spark_rect.min.y + (1.0 - v) * spark_h,
            ))
            .collect();

        if pts.len() >= 2 {
            for w in pts.windows(2) {
                ui.painter().line_segment([w[0], w[1]], Stroke::new(1.5, ACCENT));
            }
        }
    }

    fn ui_search(&mut self, ui: &mut Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("Semantic Search").size(18.0).color(TEXT).strong());
        ui.add_space(2.0);
        ui.label(RichText::new("TF-IDF search over all file names and contents")
            .size(12.0).color(MUTED));
        ui.add_space(16.0);

        ui.horizontal(|ui| {
            let w = ui.available_width() - 100.0;
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.search_input)
                    .hint_text("e.g.  authentication  ·  database config  ·  readme")
                    .desired_width(w)
                    .font(FontId::proportional(14.0))
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if accent_button(ui, if self.search_pending { "…" } else { "Search" }, 90.0, true)
                .clicked() || enter
            {
                self.do_search();
            }
        });

        ui.add_space(12.0);
        ui.add(egui::Separator::default());
        ui.add_space(8.0);

        ScrollArea::vertical().id_salt("search_result").show(ui, |ui| {
            if self.search_result.is_empty() {
                ui.label(RichText::new("Results appear here after searching")
                    .size(13.0).color(Color32::from_gray(70)));
            } else {
                ui.add(egui::TextEdit::multiline(&mut self.search_result.clone())
                    .desired_width(f32::INFINITY)
                    .font(FontId::monospace(12.5))
                    .interactive(false)
                    .frame(false)
                    .text_color(TEXT));
            }
        });
    }

    fn ui_ask(&mut self, ui: &mut Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("Ask AI").size(18.0).color(TEXT).strong());
        ui.add_space(2.0);
        ui.label(
            RichText::new("Natural-language questions about your files  ·  powered by VexFS intelligence")
                .size(12.0).color(MUTED)
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("Examples:  \"what was I working on yesterday?\"  ·  \"find config files\"  ·  \"summarise readme.md\"")
                .size(11.5).color(Color32::from_gray(90))
                .italics()
        );
        ui.add_space(14.0);

        ui.horizontal(|ui| {
            let w = ui.available_width() - 88.0;
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.ask_input)
                    .hint_text("Ask anything about your filesystem…")
                    .desired_width(w)
                    .font(FontId::proportional(14.0))
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if accent_button(ui, if self.ask_pending { "…" } else { "Ask" }, 80.0, true)
                .clicked() || enter
            {
                self.do_ask();
            }
        });

        ui.add_space(12.0);
        ui.add(egui::Separator::default());
        ui.add_space(8.0);

        ScrollArea::vertical().id_salt("ask_result").show(ui, |ui| {
            if self.ask_result.is_empty() {
                ui.label(RichText::new("Answers appear here")
                    .size(13.0).color(Color32::from_gray(70)));
            } else {
                ui.add(egui::TextEdit::multiline(&mut self.ask_result.clone())
                    .desired_width(f32::INFINITY)
                    .font(FontId::monospace(12.5))
                    .interactive(false)
                    .frame(false)
                    .text_color(TEXT));
            }
        });
    }

    fn ui_snapshots(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Snapshot Manager").size(18.0).color(TEXT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ghost_button(ui, "↺  Refresh", 90.0).clicked() { self.load_snapshots(); }
            });
        });
        ui.add_space(4.0);
        ui.label(RichText::new(&self.snap_status).size(12.0).color(MUTED));
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter:").size(12.0).color(MUTED));
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut self.snap_filter)
                .hint_text("filename…")
                .desired_width(220.0)
                .font(FontId::proportional(13.0)));
        });

        ui.add_space(8.0);
        ui.add(egui::Separator::default());

        let filter = self.snap_filter.to_lowercase();
        let snaps: Vec<SnapEntry> = self.snap_entries.iter()
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter))
            .cloned()
            .collect();

        if snaps.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(
                    if self.snap_entries.is_empty() {
                        "No snapshots yet — modify any file to create one automatically"
                    } else {
                        "No snapshots match your filter"
                    })
                    .size(13.0).color(Color32::from_gray(80)));
            });
            return;
        }

        ScrollArea::vertical().id_salt("snap_list").show(ui, |ui| {
            let mut to_restore: Option<(String, u32)> = None;

            for s in &snaps {
                glass_card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("v{}", s.version))
                                .size(12.0)
                                .color(ACCENT)
                                .monospace()
                        );
                        ui.add_space(8.0);
                        ui.label(RichText::new(&s.name).size(13.0).color(TEXT));
                        ui.add_space(8.0);
                        ui.label(RichText::new(format_bytes(s.size)).size(12.0).color(MUTED));
                        ui.add_space(8.0);
                        ui.label(RichText::new(&s.age).size(12.0).color(MUTED));

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ghost_button(ui, "Restore", 70.0).clicked() {
                                to_restore = Some((s.name.clone(), s.version));
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }

            if let Some((name, version)) = to_restore {
                self.restore_snapshot(&name, version);
            }
        });
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// eframe App trait
// ══════════════════════════════════════════════════════════════════════════════

impl eframe::App for VexApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply our theme
        apply_theme(ctx);

        self.maybe_refresh_files();
        self.maybe_poll_telemetry();
        ctx.request_repaint_after(Duration::from_secs(2));

        // ── Top bar ───────────────────────────────────────────────────────
        egui::TopBottomPanel::top("topbar")
            .exact_height(48.0)
            .frame(egui::Frame {
                fill: SURFACE,
                inner_margin: egui::Margin::symmetric(0.0, 0.0),
                stroke: Stroke::new(1.0, BORDER),
                ..Default::default()
            })
            .show(ctx, |ui| {
                ui.add_space(10.0);
                self.ui_topbar(ui);
            });

        // ── Status bar ────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(26.0)
            .frame(egui::Frame {
                fill: SURFACE,
                inner_margin: egui::Margin::symmetric(12.0, 0.0),
                stroke: Stroke::new(1.0, BORDER),
                ..Default::default()
            })
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(&self.status).size(11.0).color(MUTED));
                });
            });

        // ── Central panel ─────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame {
                fill: BG,
                inner_margin: egui::Margin::symmetric(16.0, 10.0),
                ..Default::default()
            })
            .show(ctx, |ui| {
                match self.tab {
                    Tab::Files     => self.ui_files(ui),
                    Tab::Dashboard => self.ui_dashboard(ui),
                    Tab::Search    => self.ui_search(ui),
                    Tab::Ask       => self.ui_ask(ui),
                    Tab::Snapshots => self.ui_snapshots(ui),
                }
            });
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Theme
// ══════════════════════════════════════════════════════════════════════════════

fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.spacing.item_spacing     = Vec2::new(6.0, 4.0);
    style.spacing.button_padding   = Vec2::new(10.0, 5.0);
    style.spacing.text_edit_width  = 200.0;
    style.spacing.scroll.bar_width = 6.0;

    // Window / panel backgrounds
    style.visuals.window_fill   = SURFACE;
    style.visuals.panel_fill    = BG;
    style.visuals.extreme_bg_color = Color32::from_gray(12);
    style.visuals.faint_bg_color   = SURFACE;

    // Default widget colours
    style.visuals.widgets.noninteractive.bg_fill = SURFACE;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    style.visuals.widgets.noninteractive.rounding  = Rounding::same(6.0);

    style.visuals.widgets.inactive.bg_fill  = SURFACE2;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.rounding  = Rounding::same(6.0);

    style.visuals.widgets.hovered.bg_fill   = Color32::from_rgb(35, 35, 55);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.hovered.rounding   = Rounding::same(6.0);

    style.visuals.widgets.active.bg_fill   = ACCENT_DIM;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.active.rounding   = Rounding::same(6.0);

    style.visuals.selection.bg_fill  = Color32::from_rgba_premultiplied(99, 102, 241, 80);
    style.visuals.selection.stroke   = Stroke::new(1.0, ACCENT);
    style.visuals.window_rounding    = Rounding::same(10.0);
    style.visuals.window_stroke      = Stroke::new(1.0, BORDER);

    // Text colours
    style.visuals.override_text_color = Some(TEXT);

    ctx.set_style(style);
}

// ══════════════════════════════════════════════════════════════════════════════
// Widget helpers
// ══════════════════════════════════════════════════════════════════════════════

fn accent_button(ui: &mut Ui, label: &str, width: f32, enabled: bool) -> egui::Response {
    let (bg, fg) = if enabled || true {
        (ACCENT, Color32::WHITE)
    } else {
        (SURFACE2, MUTED)
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(fg).strong())
            .fill(bg)
            .rounding(Rounding::same(6.0))
            .min_size(Vec2::new(width, 28.0))
    )
}

fn ghost_button(ui: &mut Ui, label: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(MUTED))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(5.0))
            .min_size(Vec2::new(width, 24.0))
    )
}

fn danger_button(ui: &mut Ui, label: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(DANGER).strong())
            .fill(Color32::from_rgb(40, 15, 15))
            .stroke(Stroke::new(1.0, Color32::from_rgb(120, 30, 30)))
            .rounding(Rounding::same(6.0))
            .min_size(Vec2::new(width, 28.0))
    )
}

fn small_danger_button(ui: &mut Ui, label: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).color(Color32::from_gray(100)))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, Color32::from_rgb(80, 30, 30)))
            .rounding(Rounding::same(4.0))
            .min_size(Vec2::new(width, 22.0))
    )
}

fn header_label(ui: &mut Ui, text: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text,
        FontId::proportional(11.0), MUTED);
}

fn sized_label(ui: &mut Ui, text: &str, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text,
        FontId::proportional(12.0), color);
}

/// Draw a glass-card frame around inner content.
fn glass_card(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    egui::Frame::default()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| content(ui));
}

fn stat_card(ui: &mut Ui, label: &str, value: &str, color: Color32, prefix: &str) {
    egui::Frame::default()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(if prefix.is_empty() { value.to_string() }
                                  else { format!("{prefix} {value}") })
                        .size(20.0).color(color).strong()
                );
                ui.label(RichText::new(label).size(10.5).color(MUTED));
            });
        });
}

fn tier_badge_colors(tier: &str) -> (Color32, Color32) {
    match tier {
        "HOT"  => (HOT_BG,  HOT_FG),
        "WARM" => (WARM_BG, WARM_FG),
        _      => (COLD_BG, COLD_FG),
    }
}

fn format_bytes(b: u64) -> String {
    if b < 1024           { format!("{b} B") }
    else if b < 1_048_576 { format!("{:.1} KB", b as f64 / 1024.0) }
    else                  { format!("{:.1} MB", b as f64 / 1_048_576.0) }
}

// ══════════════════════════════════════════════════════════════════════════════
// Minimal HTTP GET (no external dep)
// ══════════════════════════════════════════════════════════════════════════════

fn simple_get(url: &str) -> Option<String> {
    use std::io::{BufRead, BufReader};
    use std::net::TcpStream;

    let url = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");

    let stream = TcpStream::connect(hostport).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut w = stream.try_clone().ok()?;
    let host = hostport.split(':').next().unwrap_or(hostport);
    write!(w, "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").ok()?;

    let mut body = String::new();
    let mut in_body = false;
    for line in BufReader::new(stream).lines() {
        let line = line.ok()?;
        if in_body { body.push_str(&line); body.push('\n'); }
        else if line.is_empty() { in_body = true; }
    }
    Some(body)
}

// ══════════════════════════════════════════════════════════════════════════════
// Snapshot CLI output parser
// ══════════════════════════════════════════════════════════════════════════════

fn parse_snapshot_output(text: &str) -> Vec<SnapEntry> {
    fn parse_line(line: &str) -> Option<SnapEntry> {
        let line = line.trim();
        if !line.starts_with("[v") { return None; }
        let inner       = &line[2..];
        let version_end = inner.find(']')?;
        let version: u32 = inner[..version_end].parse().ok()?;
        let rest = inner[version_end + 2..].trim();
        let mut parts = rest.splitn(2, " \u{2014} ");
        let name = parts.next()?.trim().to_string();
        let rest2 = parts.next().unwrap_or("");
        let mut parts2 = rest2.splitn(2, " \u{2014} ");
        let size_str = parts2.next().unwrap_or("").trim();
        let size: u64 = size_str.split_whitespace().next()
            .and_then(|s| s.parse().ok()).unwrap_or(0);
        let age = parts2.next().unwrap_or("").trim().to_string();
        Some(SnapEntry { version, name, size, age })
    }
    text.lines().filter_map(parse_line).collect()
}

// ══════════════════════════════════════════════════════════════════════════════
// Public entry point — called by `vexfs gui` (and the auto-launcher)
// ══════════════════════════════════════════════════════════════════════════════

pub fn run(mountpoint: PathBuf, image_path: Option<String>, daemon_url: String) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VexFS Explorer")
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([760.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "VexFS Explorer",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(VexApp::new(mountpoint, image_path, daemon_url)))
        }),
    ).unwrap_or_else(|e| {
        eprintln!("error: GUI failed to start: {e}");
        std::process::exit(1);
    });
}
