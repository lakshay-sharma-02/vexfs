//! vexfs_gui — egui desktop explorer for VexFS
//!
//! Usage:
//!   vexfs_gui <mountpoint> [image_path] [daemon_url]
//!
//! Examples:
//!   vexfs_gui ~/mnt/vexfs
//!   vexfs_gui ~/mnt/vexfs ~/vexfs.img
//!   vexfs_gui ~/mnt/vexfs ~/vexfs.img http://localhost:8080
//!
//! Panels:
//!   Files     — directory listing with tier badges, click to open
//!   Dashboard — live AI telemetry polled from daemon
//!   Search    — writes to .vexfs-search virtual file, reads result
//!   Ask       — writes to .vexfs-ask virtual file, reads LLM answer
//!   Snapshots — lists + restores snapshots via subprocess

use eframe::egui::{self, Color32, RichText, Stroke, Vec2, Ui, ScrollArea};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ── Telemetry ────────────────────────────────────────────────────────────────

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
                let rest = &json[i + pat.len()..].trim_start_matches(|c: char| c == ' ');
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

    // Parse ranked_files array: [{"name":"...","score":...,"tier":"..."}]
    if let Some(arr_start) = json.find("\"ranked_files\":[") {
        let arr = &json[arr_start + 16..];
        for obj in arr.split('{').skip(1) {
            let get_str = |key: &str| -> String {
                let pat = format!("\"{}\":\"", key);
                obj.find(&pat)
                    .map(|i| {
                        let rest = &obj[i + pat.len()..];
                        rest.split('"').next().unwrap_or("").to_string()
                    })
                    .unwrap_or_default()
            };
            let get_f32 = |key: &str| -> f32 {
                let pat = format!("\"{}\":", key);
                obj.find(&pat)
                    .and_then(|i| {
                        let rest = &obj[i + pat.len()..];
                        rest.split(|c: char| c == ',' || c == '}')
                            .next()
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0.0)
            };
            let name = get_str("name");
            if name.is_empty() { continue; }
            t.ranked_files.push(RankedFile {
                name,
                score: get_f32("score"),
                tier: get_str("tier"),
            });
        }
    }

    t.last_updated = Some(Instant::now());
    t
}

// ── File entry ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct FileEntry {
    name: String,
    size: u64,
    is_dir: bool,
    tier: String,   // derived from telemetry ranked_files
    score: f32,
}

impl FileEntry {
    fn tier_color(&self) -> Color32 {
        match self.tier.as_str() {
            t if t.contains("HOT")  => Color32::from_rgb(220, 80,  40),
            t if t.contains("WARM") => Color32::from_rgb(180, 130, 20),
            _                       => Color32::from_rgb(80,  140, 200),
        }
    }

    fn tier_label(&self) -> &str {
        if self.tier.contains("HOT")  { "HOT" }
        else if self.tier.contains("WARM") { "WARM" }
        else { "COLD" }
    }

    fn size_str(&self) -> String {
        if self.is_dir { return "dir".into(); }
        if self.size < 1024 { format!("{} B", self.size) }
        else if self.size < 1024 * 1024 { format!("{:.1} KB", self.size as f64 / 1024.0) }
        else { format!("{:.1} MB", self.size as f64 / (1024.0 * 1024.0)) }
    }
}

// ── Snapshot entry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SnapEntry {
    version: u32,
    name: String,
    size: u64,
    age: String,
}

// ── Tab ──────────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Tab { Files, Dashboard, Search, Ask, Snapshots }

// ── App ──────────────────────────────────────────────────────────────────────

struct VexApp {
    mountpoint: PathBuf,
    image_path: Option<String>,
    daemon_url: String,

    // Shared state polled by background threads
    telemetry: Arc<Mutex<Telemetry>>,
    files: Arc<Mutex<Vec<FileEntry>>>,

    // Active tab
    tab: Tab,

    // Search panel
    search_input: String,
    search_result: String,
    search_pending: bool,

    // Ask panel
    ask_input: String,
    ask_result: String,
    ask_pending: bool,

    // Snapshots panel
    snap_entries: Vec<SnapEntry>,
    snap_filter: String,
    snap_status: String,

    // Dashboard
    cache_history: VecDeque<f32>,   // rolling cache% history for sparkline

    // Status bar
    status: String,

    // Background poll timing
    last_file_scan: Instant,
    last_telemetry_poll: Instant,
}

impl VexApp {
    fn new(mountpoint: PathBuf, image_path: Option<String>, daemon_url: String) -> Self {
        let telemetry = Arc::new(Mutex::new(Telemetry::default()));
        let files     = Arc::new(Mutex::new(vec![]));

        Self {
            mountpoint,
            image_path,
            daemon_url,
            telemetry,
            files,
            tab: Tab::Files,
            search_input: String::new(),
            search_result: String::new(),
            search_pending: false,
            ask_input: String::new(),
            ask_result: String::new(),
            ask_pending: false,
            snap_entries: vec![],
            snap_filter: String::new(),
            snap_status: String::new(),
            cache_history: VecDeque::with_capacity(60),
            status: "Ready".into(),
            last_file_scan: Instant::now() - Duration::from_secs(10),
            last_telemetry_poll: Instant::now() - Duration::from_secs(10),
        }
    }

    // ── Background polling ────────────────────────────────────────────────

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

            let entries: Vec<FileEntry> = match fs::read_dir(&mountpoint) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        // Skip virtual dot-files in the listing
                        if name.starts_with(".vexfs-") { return None; }
                        let meta = e.metadata().ok()?;
                        let size = meta.len();
                        let is_dir = meta.is_dir();
                        let (tier, score) = ranked.iter()
                            .find(|(n, _, _)| n == &name)
                            .map(|(_, s, t)| (t.clone(), *s))
                            .unwrap_or_else(|| ("COLD".into(), 0.0));
                        Some(FileEntry { name, size, is_dir, tier, score })
                    })
                    .collect(),
                Err(_) => vec![],
            };

            // Sort: dirs first, then by score desc
            let mut entries = entries;
            entries.sort_by(|a, b| {
                b.is_dir.cmp(&a.is_dir)
                    .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            });

            *files_arc.lock().unwrap() = entries;
        });
    }

    fn maybe_poll_telemetry(&mut self) {
        if self.last_telemetry_poll.elapsed() < Duration::from_secs(2) { return; }
        self.last_telemetry_poll = Instant::now();

        let url     = format!("{}/api/telemetry", self.daemon_url);
        let tel_arc = Arc::clone(&self.telemetry);

        thread::spawn(move || {
            // Simple blocking HTTP GET — no reqwest dep, use std TcpStream
            if let Some(body) = simple_get(&url) {
                let t = parse_telemetry(&body);
                *tel_arc.lock().unwrap() = t;
            }
        });
    }

    // ── Search / Ask via virtual files ───────────────────────────────────

    fn do_search(&mut self) {
        let query = self.search_input.trim().to_string();
        if query.is_empty() { return; }
        self.search_pending = true;
        self.search_result = "Searching…".into();

        let search_path = self.mountpoint.join(".vexfs-search");
        let query_clone = query.clone();

        // Write query then read result back
        let result = (|| -> Option<String> {
            fs::write(&search_path, query_clone.as_bytes()).ok()?;
            thread::sleep(Duration::from_millis(250));
            let mut f = fs::File::open(&search_path).ok()?;
            let mut out = String::new();
            f.read_to_string(&mut out).ok()?;
            if out.trim().is_empty() {
                Some(format!("No results for \"{}\"", query_clone))
            } else {
                Some(out)
            }
        })().unwrap_or_else(|| "Error: mount not accessible".into());

        self.search_result = result;
        self.search_pending = false;
    }

    fn do_ask(&mut self) {
        let question = self.ask_input.trim().to_string();
        if question.is_empty() { return; }
        self.ask_pending = true;
        self.ask_result = "Thinking…".into();

        let ask_path = self.mountpoint.join(".vexfs-ask");
        let q = question.clone();

        let result = (|| -> Option<String> {
            fs::write(&ask_path, q.as_bytes()).ok()?;
            thread::sleep(Duration::from_millis(400));
            let mut f = fs::File::open(&ask_path).ok()?;
            let mut out = String::new();
            f.read_to_string(&mut out).ok()?;
            if out.trim().is_empty() {
                Some(format!("No answer found for: {}", q))
            } else {
                Some(out)
            }
        })().unwrap_or_else(|| "Error: mount not accessible".into());

        self.ask_result = result;
        self.ask_pending = false;
    }

    // ── Snapshots via subprocess ──────────────────────────────────────────

    fn load_snapshots(&mut self) {
        let Some(img) = &self.image_path else {
            self.snap_status = "No image path provided — pass it as 2nd argument".into();
            return;
        };

        let output = Command::new("vexfs_snapshot")
            .args(["all", img])
            .output();

        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                self.snap_entries = parse_snapshot_output(&text);
                self.snap_status = format!("{} snapshots", self.snap_entries.len());
            }
            Err(_) => {
                self.snap_status = "vexfs_snapshot not in PATH".into();
            }
        }
    }

    fn restore_snapshot(&mut self, name: &str, version: u32) {
        let Some(img) = &self.image_path else { return; };
        let out = Command::new("vexfs_snapshot")
            .args(["restore", img, name, &version.to_string()])
            .output();
        self.snap_status = match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(e) => format!("Error: {}", e),
        };
    }

    // ── UI panels ─────────────────────────────────────────────────────────

    fn ui_topbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for (t, label) in [
                (Tab::Files,     "📁  Files"),
                (Tab::Dashboard, "📊  Dashboard"),
                (Tab::Search,    "🔍  Search"),
                (Tab::Ask,       "💬  Ask"),
                (Tab::Snapshots, "📸  Snapshots"),
            ] {
                let selected = self.tab == t;
                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(13.0)
                        .color(if selected {
                            Color32::from_rgb(255, 255, 255)
                        } else {
                            Color32::from_gray(180)
                        })
                )
                .fill(if selected {
                    Color32::from_rgb(80, 70, 200)
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(if selected {
                    Stroke::new(1.0, Color32::from_rgb(100, 90, 220))
                } else {
                    Stroke::NONE
                })
                .rounding(6.0)
                .min_size(Vec2::new(120.0, 30.0));

                if ui.add(btn).clicked() {
                    self.tab = t;
                    if t == Tab::Snapshots { self.load_snapshots(); }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let tel = self.telemetry.lock().unwrap();
                let age = tel.last_updated
                    .map(|t| format!("{}s ago", t.elapsed().as_secs()))
                    .unwrap_or_else(|| "no data".into());
                ui.label(
                    RichText::new(format!("⬤  {}", age))
                        .size(11.0)
                        .color(if tel.last_updated.map(|t| t.elapsed().as_secs() < 5).unwrap_or(false) {
                            Color32::from_rgb(80, 200, 100)
                        } else {
                            Color32::from_gray(100)
                        })
                );
                ui.label(
                    RichText::new(format!("daemon: {}", self.daemon_url))
                        .size(11.0)
                        .color(Color32::from_gray(120))
                );
            });
        });
    }

    fn ui_files(&mut self, ui: &mut Ui) {
        let files = self.files.lock().unwrap().clone();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}  {} files", self.mountpoint.display(), files.len()))
                    .size(12.0)
                    .color(Color32::from_gray(150))
            );
        });
        ui.add_space(6.0);

        // Header
        egui::Grid::new("file_header")
            .num_columns(4)
            .min_col_width(80.0)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").size(12.0).color(Color32::from_gray(160)));
                ui.label(RichText::new("Size").size(12.0).color(Color32::from_gray(160)));
                ui.label(RichText::new("Tier").size(12.0).color(Color32::from_gray(160)));
                ui.label(RichText::new("Score").size(12.0).color(Color32::from_gray(160)));
                ui.end_row();
            });

        ui.separator();

        ScrollArea::vertical()
            .id_source("file_list")
            .max_height(ui.available_height() - 40.0)
            .show(ui, |ui| {
                if files.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No files — is the filesystem mounted?")
                                .size(14.0)
                                .color(Color32::from_gray(120))
                        );
                    });
                    return;
                }

                egui::Grid::new("file_grid")
                    .num_columns(4)
                    .min_col_width(80.0)
                    .spacing([12.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for f in &files {
                            // Name (clickable)
                            let icon = if f.is_dir { "📁" } else { "📄" };
                            let name_label = ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{}  {}", icon, f.name)).size(13.0)
                                )
                                .sense(egui::Sense::click())
                            );
                            if name_label.clicked() {
                                let full = self.mountpoint.join(&f.name);
                                #[cfg(target_os = "linux")]
                                { let _ = Command::new("xdg-open").arg(&full).spawn(); }
                                #[cfg(target_os = "macos")]
                                { let _ = Command::new("open").arg(&full).spawn(); }
                            }
                            if name_label.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            // Size
                            ui.label(
                                RichText::new(f.size_str())
                                    .size(12.0)
                                    .color(Color32::from_gray(170))
                            );

                            // Tier badge
                            let (bg, fg) = match f.tier_label() {
                                "HOT"  => (Color32::from_rgb(180, 50, 20), Color32::from_rgb(255, 200, 180)),
                                "WARM" => (Color32::from_rgb(140, 100, 10), Color32::from_rgb(255, 230, 150)),
                                _      => (Color32::from_rgb(30, 80, 130), Color32::from_rgb(160, 210, 255)),
                            };
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(48.0, 20.0),
                                egui::Sense::hover()
                            );
                            ui.painter().rect_filled(rect, 4.0, bg);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                f.tier_label(),
                                egui::FontId::proportional(10.0),
                                fg,
                            );

                            // Score bar
                            let bar_w = 80.0;
                            let (bar_rect, _) = ui.allocate_exact_size(
                                Vec2::new(bar_w, 16.0),
                                egui::Sense::hover()
                            );
                            ui.painter().rect_filled(
                                bar_rect,
                                3.0,
                                Color32::from_gray(40)
                            );
                            if f.score > 0.0 {
                                let fill = egui::Rect::from_min_size(
                                    bar_rect.min,
                                    Vec2::new(bar_w * f.score, bar_rect.height())
                                );
                                ui.painter().rect_filled(fill, 3.0, f.tier_color());
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn ui_dashboard(&mut self, ui: &mut Ui) {
        let tel = self.telemetry.lock().unwrap().clone();

        // Cache usage bar
        let cache_pct = if tel.cache_max > 0 {
            tel.cache_used as f32 / tel.cache_max as f32
        } else { 0.0 };

        // Update sparkline history
        drop(tel); // release lock before mutable borrow
        self.cache_history.push_back(cache_pct);
        if self.cache_history.len() > 60 { self.cache_history.pop_front(); }

        let tel = self.telemetry.lock().unwrap().clone();

        ui.add_space(8.0);

        // ── Stat cards row ────────────────────────────────────────────────
        ui.horizontal(|ui| {
            stat_card(ui, "Cache", &format!("{:.1} MB / {:.1} MB",
                tel.cache_used as f64 / 1_048_576.0,
                tel.cache_max  as f64 / 1_048_576.0),
                Color32::from_rgb(80, 130, 230));

            stat_card(ui, "Markov entries", &tel.markov_entries.to_string(),
                Color32::from_rgb(130, 80, 230));

            stat_card(ui, "Search index", &tel.search_indexed.to_string(),
                Color32::from_rgb(50, 160, 130));

            stat_card(ui, "Snapshots", &tel.snapshots_total.to_string(),
                Color32::from_rgb(200, 140, 40));

            stat_card(ui, "Files", &tel.total_files.to_string(),
                Color32::from_gray(130));

            let threat_color = if tel.entropy_threats > 0 {
                Color32::from_rgb(220, 60, 40)
            } else {
                Color32::from_gray(100)
            };
            stat_card(ui, "Entropy threats",
                &tel.entropy_threats.to_string(),
                threat_color);
        });

        ui.add_space(12.0);

        // ── Cache progress bar ────────────────────────────────────────────
        ui.label(RichText::new("Cache usage").size(12.0).color(Color32::from_gray(160)));
        let (bar_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 18.0),
            egui::Sense::hover()
        );
        ui.painter().rect_filled(bar_rect, 4.0, Color32::from_gray(35));
        if cache_pct > 0.0 {
            let fill = egui::Rect::from_min_size(
                bar_rect.min,
                Vec2::new(bar_rect.width() * cache_pct, bar_rect.height())
            );
            let bar_color = if cache_pct > 0.85 {
                Color32::from_rgb(220, 70, 40)
            } else if cache_pct > 0.60 {
                Color32::from_rgb(200, 140, 30)
            } else {
                Color32::from_rgb(60, 150, 220)
            };
            ui.painter().rect_filled(fill, 4.0, bar_color);
        }
        ui.painter().text(
            bar_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{:.1}%", cache_pct * 100.0),
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );

        ui.add_space(12.0);

        // ── Ranked files table ────────────────────────────────────────────
        ui.label(RichText::new("Top files by importance").size(12.0).color(Color32::from_gray(160)));
        ui.add_space(4.0);

        if tel.ranked_files.is_empty() {
            ui.label(
                RichText::new("No file scores yet — open some files to build the model")
                    .size(12.0)
                    .color(Color32::from_gray(120))
            );
        } else {
            ScrollArea::vertical()
                .id_source("ranked")
                .max_height(200.0)
                .show(ui, |ui| {
                    egui::Grid::new("ranked_grid")
                        .num_columns(3)
                        .spacing([16.0, 5.0])
                        .striped(true)
                        .show(ui, |ui| {
                            for r in &tel.ranked_files {
                                // Tier badge
                                let tier_str = if r.tier.contains("HOT") { "HOT" }
                                    else if r.tier.contains("WARM") { "WARM" }
                                    else { "COLD" };
                                let (bg, fg) = match tier_str {
                                    "HOT"  => (Color32::from_rgb(180,50,20), Color32::from_rgb(255,200,180)),
                                    "WARM" => (Color32::from_rgb(140,100,10), Color32::from_rgb(255,230,150)),
                                    _      => (Color32::from_rgb(30,80,130), Color32::from_rgb(160,210,255)),
                                };
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(44.0, 18.0),
                                    egui::Sense::hover()
                                );
                                ui.painter().rect_filled(rect, 3.0, bg);
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    tier_str,
                                    egui::FontId::proportional(10.0),
                                    fg,
                                );

                                ui.label(RichText::new(&r.name).size(13.0));

                                // Score bar
                                let bw = 120.0;
                                let (br, _) = ui.allocate_exact_size(
                                    Vec2::new(bw, 14.0),
                                    egui::Sense::hover()
                                );
                                ui.painter().rect_filled(br, 3.0, Color32::from_gray(40));
                                let fill = egui::Rect::from_min_size(
                                    br.min,
                                    Vec2::new(bw * r.score, br.height())
                                );
                                ui.painter().rect_filled(fill, 3.0,
                                    if r.tier.contains("HOT") { Color32::from_rgb(220,80,40) }
                                    else if r.tier.contains("WARM") { Color32::from_rgb(200,140,30) }
                                    else { Color32::from_rgb(60,140,210) }
                                );
                                ui.label(
                                    RichText::new(format!("{:.2}", r.score))
                                        .size(11.0)
                                        .color(Color32::from_gray(150))
                                );
                                ui.end_row();
                            }
                        });
                });
        }

        // ── Sparkline ─────────────────────────────────────────────────────
        ui.add_space(12.0);
        ui.label(RichText::new("Cache usage — last 60 polls").size(12.0).color(Color32::from_gray(160)));

        let spark_h = 50.0;
        let (spark_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), spark_h),
            egui::Sense::hover()
        );
        ui.painter().rect_filled(spark_rect, 4.0, Color32::from_gray(25));

        let pts: Vec<_> = self.cache_history.iter().enumerate()
            .map(|(i, &v)| {
                egui::pos2(
                    spark_rect.min.x + (i as f32 / 59.0) * spark_rect.width(),
                    spark_rect.min.y + (1.0 - v) * spark_h,
                )
            })
            .collect();

        if pts.len() >= 2 {
            for w in pts.windows(2) {
                ui.painter().line_segment(
                    [w[0], w[1]],
                    Stroke::new(1.5, Color32::from_rgb(80, 160, 255))
                );
            }
        }
    }

    fn ui_search(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.label(
            RichText::new("Search file contents and names using TF-IDF")
                .size(13.0)
                .color(Color32::from_gray(180))
        );
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.search_input)
                    .hint_text("e.g. authentication, database config, readme")
                    .desired_width(ui.available_width() - 90.0)
                    .font(egui::FontId::proportional(14.0))
            );

            let enter = edit.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if ui.add(
                egui::Button::new(
                    RichText::new(if self.search_pending { "…" } else { "Search" })
                        .size(13.0)
                )
                .min_size(Vec2::new(80.0, 32.0))
                .fill(Color32::from_rgb(60, 100, 200))
            ).clicked() || enter {
                self.do_search();
            }
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ScrollArea::vertical()
            .id_source("search_result")
            .show(ui, |ui| {
                if self.search_result.is_empty() {
                    ui.label(
                        RichText::new("Results appear here after searching")
                            .size(13.0)
                            .color(Color32::from_gray(100))
                    );
                } else {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.search_result.clone())
                            .desired_width(f32::INFINITY)
                            .font(egui::FontId::monospace(12.0))
                            .interactive(false)
                    );
                }
            });
    }

    fn ui_ask(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.label(
            RichText::new("Ask a natural-language question about your files")
                .size(13.0)
                .color(Color32::from_gray(180))
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("e.g.  \"what was I working on yesterday?\"  /  \"find config files\"")
                .size(12.0)
                .color(Color32::from_gray(120))
        );
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.ask_input)
                    .hint_text("Ask anything about your filesystem…")
                    .desired_width(ui.available_width() - 80.0)
                    .font(egui::FontId::proportional(14.0))
            );

            let enter = edit.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if ui.add(
                egui::Button::new(
                    RichText::new(if self.ask_pending { "…" } else { "Ask" })
                        .size(13.0)
                )
                .min_size(Vec2::new(70.0, 32.0))
                .fill(Color32::from_rgb(80, 50, 180))
            ).clicked() || enter {
                self.do_ask();
            }
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ScrollArea::vertical()
            .id_source("ask_result")
            .show(ui, |ui| {
                if self.ask_result.is_empty() {
                    ui.label(
                        RichText::new("Answers appear here after asking")
                            .size(13.0)
                            .color(Color32::from_gray(100))
                    );
                } else {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.ask_result.clone())
                            .desired_width(f32::INFINITY)
                            .font(egui::FontId::monospace(12.0))
                            .interactive(false)
                    );
                }
            });
    }

    fn ui_snapshots(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&self.snap_status)
                    .size(12.0)
                    .color(Color32::from_gray(160))
            );
            if ui.button("↺  Refresh").clicked() { self.load_snapshots(); }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter:").size(12.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.snap_filter)
                    .hint_text("filename…")
                    .desired_width(200.0)
            );
        });
        ui.add_space(8.0);
        ui.separator();

        let filter = self.snap_filter.to_lowercase();
        let snaps: Vec<SnapEntry> = self.snap_entries.iter()
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter))
            .cloned()
            .collect();

        if snaps.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if self.snap_entries.is_empty() {
                        "No snapshots yet — modify a file to create one"
                    } else {
                        "No snapshots match filter"
                    })
                    .size(13.0)
                    .color(Color32::from_gray(120))
                );
            });
            return;
        }

        ScrollArea::vertical()
            .id_source("snap_list")
            .show(ui, |ui| {
                egui::Grid::new("snap_grid")
                    .num_columns(4)
                    .spacing([16.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Version").size(12.0).color(Color32::from_gray(150)));
                        ui.label(RichText::new("File").size(12.0).color(Color32::from_gray(150)));
                        ui.label(RichText::new("Size").size(12.0).color(Color32::from_gray(150)));
                        ui.label(RichText::new("Age").size(12.0).color(Color32::from_gray(150)));
                        ui.label(RichText::new("").size(12.0));
                        ui.end_row();

                        let mut to_restore: Option<(String, u32)> = None;
                        for s in &snaps {
                            ui.label(
                                RichText::new(format!("v{}", s.version))
                                    .size(13.0)
                                    .color(Color32::from_rgb(140, 160, 230))
                            );
                            ui.label(RichText::new(&s.name).size(13.0));
                            ui.label(
                                RichText::new(format_bytes(s.size))
                                    .size(12.0)
                                    .color(Color32::from_gray(160))
                            );
                            ui.label(
                                RichText::new(&s.age)
                                    .size(12.0)
                                    .color(Color32::from_gray(160))
                            );
                            if ui.add(
                                egui::Button::new(
                                    RichText::new("Restore").size(11.0)
                                )
                                .fill(Color32::from_rgb(40, 90, 50))
                                .min_size(Vec2::new(60.0, 22.0))
                            ).clicked() {
                                to_restore = Some((s.name.clone(), s.version));
                            }
                            ui.end_row();
                        }
                        if let Some((name, version)) = to_restore {
                            self.restore_snapshot(&name, version);
                        }
                    });
            });
    }
}

// ── eframe App trait ─────────────────────────────────────────────────────────

impl eframe::App for VexApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.maybe_refresh_files();
        self.maybe_poll_telemetry();

        // Keep refreshing for live updates
        ctx.request_repaint_after(Duration::from_secs(2));

        // ── Title bar ──────────────────────────────────────────────────────
        egui::TopBottomPanel::top("topbar")
            .exact_height(44.0)
            .show(ctx, |ui| {
                ui.add_space(7.0);
                self.ui_topbar(ui);
            });

        // ── Status bar ────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(24.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(&self.status)
                            .size(11.0)
                            .color(Color32::from_gray(130))
                    );
                });
            });

        // ── Main panel ────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);

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

// ── Helper widgets ────────────────────────────────────────────────────────────

fn stat_card(ui: &mut Ui, label: &str, value: &str, color: Color32) {
    egui::Frame::none()
        .fill(Color32::from_gray(28))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .stroke(Stroke::new(0.5, color.linear_multiply(0.4)))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(value)
                        .size(18.0)
                        .color(color)
                );
                ui.label(
                    RichText::new(label)
                        .size(11.0)
                        .color(Color32::from_gray(130))
                );
            });
        });
}

fn format_bytes(b: u64) -> String {
    if b < 1024 { format!("{} B", b) }
    else if b < 1024 * 1024 { format!("{:.1} KB", b as f64 / 1024.0) }
    else { format!("{:.1} MB", b as f64 / 1_048_576.0) }
}

// ── Simple HTTP GET (no external dep) ────────────────────────────────────────

fn simple_get(url: &str) -> Option<String> {
    use std::io::{BufRead, BufReader};
    use std::net::TcpStream;

    // Parse http://host:port/path
    let url = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{}", path);

    let stream = TcpStream::connect(hostport)
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();

    let mut stream_write = stream.try_clone().ok()?;
    let host = hostport.split(':').next().unwrap_or(hostport);
    write!(stream_write,
        "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    ).ok()?;

    let reader = BufReader::new(stream);
    let mut body = String::new();
    let mut in_body = false;
    for line in reader.lines() {
        let line = line.ok()?;
        if in_body {
            body.push_str(&line);
            body.push('\n');
        } else if line.is_empty() {
            in_body = true;
        }
    }
    Some(body)
}

// ── Parse snapshot CLI output ─────────────────────────────────────────────────

fn parse_snapshot_output(text: &str) -> Vec<SnapEntry> {
    // Parses lines like: "  [v3] filename.txt — 1024 bytes — 5m ago"
    fn parse_line(line: &str) -> Option<SnapEntry> {
        let line = line.trim();
        if !line.starts_with("[v") { return None; }
        let inner = &line[2..];
        let version_end = inner.find(']')?;
        let version: u32 = inner[..version_end].parse().ok()?;
        let rest = inner[version_end + 2..].trim();
        let mut parts = rest.splitn(2, " \u{2014} "); // em dash
        let name = parts.next()?.trim().to_string();
        let rest2 = parts.next().unwrap_or("");
        let mut parts2 = rest2.splitn(2, " \u{2014} ");
        let size_str = parts2.next().unwrap_or("").trim();
        let size: u64 = size_str.split_whitespace().next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let age = parts2.next().unwrap_or("").trim().to_string();
        Some(SnapEntry { version, name, size, age })
    }

    text.lines().filter_map(parse_line).collect()
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vexfs_gui <mountpoint> [image_path] [daemon_url]");
        eprintln!("  e.g. vexfs_gui ~/mnt/vexfs ~/vexfs.img http://localhost:8080");
        std::process::exit(1);
    }

    let mountpoint   = PathBuf::from(&args[1]);
    let image_path   = args.get(2).cloned();
    let daemon_url   = args.get(3)
        .cloned()
        .unwrap_or_else(|| "http://localhost:8080".into());

    if !mountpoint.exists() {
        eprintln!("Mountpoint '{}' does not exist", mountpoint.display());
        std::process::exit(1);
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VexFS Explorer")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "VexFS Explorer",
        options,
        Box::new(move |_cc| {
            Box::new(VexApp::new(mountpoint, image_path, daemon_url)) as Box<dyn eframe::App>
        }),
    ).unwrap();
}
