// Hide console window on Windows
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use reqwest::Client;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Platform {
    WinX86,
    WinX64,
    WinArm64,
    MacOsIntel,
    MacOsArm64,
}

impl Platform {
    fn name(&self) -> &str {
        match self {
            Platform::WinX86 => "Windows x86",
            Platform::WinX64 => "Windows x64",
            Platform::WinArm64 => "Windows arm64",
            Platform::MacOsIntel => "macOS intel",
            Platform::MacOsArm64 => "macOS arm64",
        }
    }

    fn path_template(&self) -> &str {
        match self {
            Platform::WinX86 => "win32-x86/spotify_installer-{version}-{number}.exe",
            Platform::WinX64 => "win32-x86_64/spotify_installer-{version}-{number}.exe",
            Platform::WinArm64 => "win32-arm64/spotify_installer-{version}-{number}.exe",
            Platform::MacOsIntel => "osx-x86_64/spotify-autoupdate-{version}-{number}.tbz",
            Platform::MacOsArm64 => "osx-arm64/spotify-autoupdate-{version}-{number}.tbz",
        }
    }

    fn all() -> Vec<Platform> {
        vec![
            Platform::WinX86,
            Platform::WinX64,
            Platform::WinArm64,
            Platform::MacOsIntel,
            Platform::MacOsArm64,
        ]
    }
}

#[derive(Deserialize)]
struct VersionData {
    fullversion: Option<String>,
}

struct UrlGenerator;

impl UrlGenerator {
    const BASE_URL: &'static str = "https://upgrade.scdn.co/upgrade/client/";

    fn generate_url(platform: Platform, version: &str, number: i32) -> String {
        let path = platform
            .path_template()
            .replace("{version}", version)
            .replace("{number}", &number.to_string());
        format!("{}{}", Self::BASE_URL, path)
    }
}

#[allow(dead_code)]
fn extract_base_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 3 {
        format!("{}.{}.{}", parts[0], parts[1], parts[2])
    } else {
        version.to_string()
    }
}

#[allow(dead_code)]
fn should_use_win_x86(version: &str) -> bool {
    let base_version = extract_base_version(version);
    let parts: Vec<&str> = base_version.split('.').collect();

    if parts.len() >= 3 {
        if let (Ok(major), Ok(minor), Ok(patch)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            return (major, minor, patch) <= (1, 2, 53);
        }
    }
    true
}

fn validate_version(version: &str) -> bool {
    let re = regex::Regex::new(r"^\d+\.\d+\.\d+\.\d+\.g[0-9a-f]{8}$").unwrap();
    re.is_match(version)
}

fn short_version(version: &str) -> String {
    if let Some(pos) = version.find(".g") {
        return version[..pos].to_string();
    }

    let parts: Vec<&str> = version.split('.').collect();
    let take = parts.len().min(4);
    parts[..take].join(".")
}

const MAX_CONNECTION_OPTIONS: [usize; 6] = [50, 100, 150, 200, 250, 300];

async fn check_url(client: &Client, url: String, platform: Platform) -> Option<(String, Platform)> {
    match client.head(&url).send().await {
        Ok(response) if response.status().is_success() => Some((url, platform)),
        _ => None,
    }
}

async fn fetch_versions_json(client: &Client) -> HashMap<String, VersionData> {
    let url =
        "https://raw.githubusercontent.com/LoaderSpot/LoaderSpot/refs/heads/main/versions.json";

    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => response
            .json::<HashMap<String, VersionData>>()
            .await
            .unwrap_or_default(),
        _ => HashMap::new(),
    }
}

async fn submit_to_google_form(client: &Client, version: &str) {
    let form_url = "https://docs.google.com/forms/u/0/d/e/1FAIpQLSdqIxSjqt2PcjBlQzhvwqc4QckfWuq5qqWsrdpoTidQHsPGpw/formResponse";

    let params = [
        ("entry.1104502920", version),
        ("entry.1319854718", "from LoaderSpot"),
    ];

    let _ = client.post(form_url).form(&params).send().await;
}

async fn check_version_and_submit(client: &Client, version: &str) {
    let versions_json = fetch_versions_json(client).await;

    let version_exists = versions_json
        .values()
        .any(|v| v.fullversion.as_ref().map_or(false, |fv| fv == version));

    if !version_exists {
        submit_to_google_form(client, version).await;
    }
}

enum SearchMessage {
    Progress(u64, u64),
    Result(String, Platform),
    Complete(String),
    VersionStart(String, usize, usize),
    CompleteAll,
}

async fn search_installers(
    client: &Client,
    version: &str,
    start: i32,
    end: i32,
    platforms: Vec<Platform>,
    max_connections: usize,
    tx: Sender<SearchMessage>,
    pause_flag: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
) {
    use tokio::sync::Semaphore;

    let total = ((end - start + 1) * platforms.len() as i32) as u64;
    let semaphore = Arc::new(Semaphore::new(max_connections));
    let mut tasks = Vec::new();
    let processed = Arc::new(Mutex::new(0u64));

    for platform in platforms {
        for number in start..=end {
            let url = UrlGenerator::generate_url(platform, version, number);
            let client = client.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let tx_clone = tx.clone();
            let processed_clone = processed.clone();
            let pause_local = pause_flag.clone();
            let cancel_local = cancel_flag.clone();

            let task = tokio::spawn(async move {
                if cancel_local.load(Ordering::Relaxed) {
                    return;
                }

                while pause_local.load(Ordering::Relaxed) {
                    if cancel_local.load(Ordering::Relaxed) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }

                let result = check_url(&client, url, platform).await;
                drop(permit);

                let mut p = processed_clone.lock().unwrap();
                *p += 1;
                let current = *p;
                drop(p);

                let _ = tx_clone.send(SearchMessage::Progress(current, total));

                if let Some((url, platform)) = result {
                    let _ = tx_clone.send(SearchMessage::Result(url, platform));
                }
            });

            tasks.push(task);
        }
    }

    for task in tasks {
        let _ = task.await;
    }

    let _ = tx.send(SearchMessage::Complete(version.to_string()));
}

struct SpotifyFinderApp {
    runtime: Runtime,
    versions_input: String,
    range_from: String,
    range_to: String,
    max_connections_index: usize,

    platform_win_x86: bool,
    platform_win_x64: bool,
    platform_win_arm64: bool,
    platform_macos_intel: bool,
    platform_macos_arm64: bool,

    report_unknown: bool,

    search_results: String,
    is_searching: bool,
    is_paused: bool,
    displayed_results: String,
    current_version: Option<String>,
    current_version_index: usize,
    total_versions: usize,
    reveal_queue: VecDeque<String>,
    current_reveal: Option<String>,
    reveal_pos: usize,
    last_reveal: Instant,
    reveal_speed_ms: u64,
    progress: f32,
    progress_text: String,
    total_work: u64,
    processed_global: u64,

    rx: Option<Receiver<SearchMessage>>,
    found_urls: HashMap<Platform, Vec<String>>,
    pause_flag: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
}

impl Default for SpotifyFinderApp {
    fn default() -> Self {
        Self {
            runtime: Runtime::new().unwrap(),
            versions_input: String::new(),
            range_from: "0".to_string(),
            range_to: "5000".to_string(),
            max_connections_index: 1,
            platform_win_x86: false,
            platform_win_x64: false,
            platform_win_arm64: false,
            platform_macos_intel: false,
            platform_macos_arm64: false,
            report_unknown: false,
            search_results: String::new(),
            is_searching: false,
            is_paused: false,
            displayed_results: String::new(),
            reveal_queue: VecDeque::new(),
            current_reveal: None,
            reveal_pos: 0,
            last_reveal: Instant::now(),
            reveal_speed_ms: 8,
            progress: 0.0,
            progress_text: String::new(),
            total_work: 0,
            processed_global: 0,
            rx: None,
            found_urls: HashMap::new(),
            pause_flag: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            current_version: None,
            current_version_index: 0,
            total_versions: 0,
        }
    }
}

impl SpotifyFinderApp {
    // Improved reveal logic: finish current item immediately and fast-forward the queue
    fn advance_reveal(&mut self) {
        if self.current_reveal.is_some() && !self.reveal_queue.is_empty() {
            if let Some(cur) = self.current_reveal.take() {
                if self.reveal_pos < cur.len() {
                    self.displayed_results.push_str(&cur[self.reveal_pos..]);
                }
            }

            while self.reveal_queue.len() > 1 {
                if let Some(line) = self.reveal_queue.pop_front() {
                    self.displayed_results.push_str(&line);
                }
            }

            self.current_reveal = None;
            self.reveal_pos = 0;
        }

        if self.current_reveal.is_none() {
            if let Some(next) = self.reveal_queue.pop_front() {
                self.current_reveal = Some(next);
                self.reveal_pos = 0;
            }
        }

        if let Some(cur) = &self.current_reveal {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last_reveal);

            let mut chars_to_show = (elapsed.as_millis() as u64) / (self.reveal_speed_ms.max(1));

            if chars_to_show > 0 {
                while chars_to_show > 0 && self.reveal_pos < cur.len() {
                    let next_char = cur[self.reveal_pos..].chars().next().unwrap();
                    self.displayed_results.push(next_char);
                    self.reveal_pos += next_char.len_utf8();
                    chars_to_show -= 1;
                }
                self.last_reveal = now;
            }

            if self.reveal_pos >= cur.len() {
                self.current_reveal = None;
            }
        }
    }

    fn get_selected_platforms(&self) -> Vec<Platform> {
        let mut platforms = Vec::new();
        if self.platform_win_x86 {
            platforms.push(Platform::WinX86);
        }
        if self.platform_win_x64 {
            platforms.push(Platform::WinX64);
        }
        if self.platform_win_arm64 {
            platforms.push(Platform::WinArm64);
        }
        if self.platform_macos_intel {
            platforms.push(Platform::MacOsIntel);
        }
        if self.platform_macos_arm64 {
            platforms.push(Platform::MacOsArm64);
        }
        platforms
    }

    fn select_all_platforms(&mut self) {
        self.platform_win_x86 = true;
        self.platform_win_x64 = true;
        self.platform_win_arm64 = true;
        self.platform_macos_intel = true;
        self.platform_macos_arm64 = true;
    }

    fn select_no_platforms(&mut self) {
        self.platform_win_x86 = false;
        self.platform_win_x64 = false;
        self.platform_win_arm64 = false;
        self.platform_macos_intel = false;
        self.platform_macos_arm64 = false;
    }

    fn start_search(&mut self) {
        let versions: Vec<String> = self
            .versions_input
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && validate_version(s))
            .collect();

        if versions.is_empty() {
            self.displayed_results = "Error: No valid versions provided".to_string();
            return;
        }

        let start = self.range_from.parse::<i32>().unwrap_or(0);
        let end = self.range_to.parse::<i32>().unwrap_or(5000);

        if end < start {
            self.displayed_results = "Error: End range must be >= start range".to_string();
            return;
        }

        let base_platforms = self.get_selected_platforms();
        if base_platforms.is_empty() {
            self.displayed_results = "Error: No platforms selected".to_string();
            return;
        }

        let max_conn = MAX_CONNECTION_OPTIONS[self
            .max_connections_index
            .min(MAX_CONNECTION_OPTIONS.len() - 1)];

        if base_platforms.len() == 1 && base_platforms[0] == Platform::WinX86 && versions.len() == 1
        {
            let user_version = &versions[0];
            if !should_use_win_x86(user_version) {
                self.displayed_results = "Warning: x86 architecture for Windows is no longer supported for versions newer than 1.2.53".to_string();
                return;
            }
        }

        self.is_searching = true;
        self.progress = 0.0;
        self.progress_text = "Starting...".to_string();
        self.search_results.clear();
        self.displayed_results.clear();
        self.reveal_queue.clear();
        self.current_reveal = None;
        self.found_urls.clear();

        let mut total_work_calc: u64 = 0;
        for v in &versions {
            let mut cnt = base_platforms.len();
            if base_platforms.contains(&Platform::WinX86) && cnt > 1 && !should_use_win_x86(v) {
                cnt -= 1;
            }
            let per_version = ((end - start + 1) * cnt as i32) as u64;
            total_work_calc = total_work_calc.saturating_add(per_version);
        }
        self.total_work = total_work_calc;
        self.processed_global = 0;
        self.current_version = None;
        self.current_version_index = 0;
        self.total_versions = versions.len();

        let (tx, rx) = channel();
        self.rx = Some(rx);

        self.pause_flag.store(false, Ordering::Relaxed);
        self.cancel_flag.store(false, Ordering::Relaxed);
        self.is_paused = false;

        let versions_to_search = versions.clone();
        let report_unknown = self.report_unknown;
        let pause = self.pause_flag.clone();
        let cancel = self.cancel_flag.clone();
        let base_platforms_for_spawn = base_platforms.clone();

        self.runtime.spawn(async move {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap();

            let total_versions = versions_to_search.len();
            for (i, version) in versions_to_search.into_iter().enumerate() {
                let _ = tx.clone().send(SearchMessage::VersionStart(
                    version.clone(),
                    i + 1,
                    total_versions,
                ));
                if report_unknown {
                    check_version_and_submit(&client, &version).await;
                }

                let mut platforms_for_version = base_platforms_for_spawn.clone();
                if platforms_for_version.contains(&Platform::WinX86)
                    && platforms_for_version.len() > 1
                    && !should_use_win_x86(&version)
                {
                    platforms_for_version.retain(|p| *p != Platform::WinX86);
                }

                if platforms_for_version.is_empty() {
                    let _ = tx
                        .clone()
                        .send(SearchMessage::Complete(version.to_string()));
                    continue;
                }

                search_installers(
                    &client,
                    &version,
                    start,
                    end,
                    platforms_for_version,
                    max_conn,
                    tx.clone(),
                    pause.clone(),
                    cancel.clone(),
                )
                .await;

                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }

            let _ = tx.send(SearchMessage::CompleteAll);
        });
    }

    fn stop_search(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.pause_flag.store(false, Ordering::Relaxed);
        self.is_paused = false;
        self.is_searching = false;
        self.rx = None;
        self.progress_text = "Search stopped".to_string();
    }

    fn clear_results(&mut self) {
        self.search_results.clear();
        self.displayed_results.clear();
        self.reveal_queue.clear();
        self.current_reveal = None;
        self.found_urls.clear();
        self.progress = 0.0;
        self.progress_text.clear();
        self.total_work = 0;
        self.processed_global = 0;
    }

    fn update_search_progress(&mut self) {
        if let Some(rx_owned) = self.rx.take() {
            let rx = rx_owned;
            let mut completed = false;

            while let Ok(msg) = rx.try_recv() {
                match msg {
                    SearchMessage::Progress(_current, _total) => {
                        self.processed_global = self.processed_global.saturating_add(1);
                        let denom = if self.total_work == 0 {
                            1
                        } else {
                            self.total_work
                        };
                        self.progress = self.processed_global as f32 / denom as f32;

                        if let Some(v) = &self.current_version {
                            let short = short_version(v);
                            if self.total_versions > 1 {
                                self.progress_text = format!(
                                    "Checking: {}/{}, Version: {}, No. {}/{}",
                                    self.processed_global,
                                    denom,
                                    short,
                                    self.current_version_index,
                                    self.total_versions
                                );
                            } else {
                                self.progress_text = format!(
                                    "Checking: {}/{}, Version: {}",
                                    self.processed_global, denom, short
                                );
                            }
                        } else {
                            self.progress_text =
                                format!("Checking: {}/{}", self.processed_global, denom);
                        }
                    }
                    SearchMessage::Result(url, platform) => {
                        let entry = self.found_urls.entry(platform).or_insert_with(Vec::new);
                        let first = entry.is_empty();
                        entry.push(url.clone());

                        if first {
                            self.reveal_queue
                                .push_back(format!("\n{}:\n", platform.name()));
                        }
                        self.reveal_queue.push_back(format!("{}\n", url));
                    }
                    SearchMessage::VersionStart(version, idx, total) => {
                        self.current_version = Some(version);
                        self.current_version_index = idx;
                        self.total_versions = total;
                    }
                    SearchMessage::Complete(_version) => {}
                    SearchMessage::CompleteAll => {
                        self.is_searching = false;
                        self.progress = 1.0;
                        self.progress_text = "Search complete".to_string();

                        let mut found_any = false;

                        for platform in Platform::all() {
                            if let Some(urls) = self.found_urls.get(&platform) {
                                if !urls.is_empty() {
                                    found_any = true;
                                }
                            }
                        }

                        self.pause_flag.store(false, Ordering::Relaxed);
                        self.cancel_flag.store(false, Ordering::Relaxed);
                        self.is_paused = false;
                        if !found_any {
                            self.displayed_results =
                                "Nothing found, consider increasing the search range".to_string();
                        }

                        completed = true;
                    }
                }
            }

            if !completed {
                self.rx = Some(rx);
            }
        }
    }
}

impl eframe::App for SpotifyFinderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_search_progress();
        self.advance_reveal();

        if self.is_searching || !self.reveal_queue.is_empty() || self.current_reveal.is_some() {
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spotify Installer Finder");
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_gray(30))
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Spotify Versions:").strong());
                            ui.label(
                                egui::RichText::new("One per line. Example: 1.1.68.632.g2b11de83")
                                    .size(12.0)
                                    .color(egui::Color32::GRAY),
                            );

                            egui::Frame::group(ui.style())
                                .fill(egui::Color32::from_gray(20))
                                .show(ui, |ui| {
                                    let desired = egui::Vec2::new(348.0, 106.0);
                                    let (rect, _resp) =
                                        ui.allocate_exact_size(desired, egui::Sense::click());
                                    ui.painter().rect_filled(
                                        rect,
                                        4.0,
                                        egui::Color32::from_gray(20),
                                    );
                                    #[allow(deprecated)]
                                    ui.allocate_ui_at_rect(rect, |ui| {
                                        #[allow(deprecated)]
                                        egui::ScrollArea::vertical()
                                            .id_salt("versions_scroll")
                                            .show(ui, |ui| {
                                                ui.add(
                                                    egui::TextEdit::multiline(
                                                        &mut self.versions_input,
                                                    )
                                                    .desired_width(rect.width())
                                                    .font(egui::TextStyle::Monospace)
                                                    .frame(false),
                                                );
                                            });
                                    });
                                });

                            ui.add_space(5.0);

                            ui.label(egui::RichText::new("Build Number Range:").strong());
                            ui.horizontal(|ui| {
                                ui.label("From:");
                                let desired = egui::Vec2::new(120.0, ui.spacing().interact_size.y);
                                let (rect, _resp) =
                                    ui.allocate_exact_size(desired, egui::Sense::click());
                                ui.painter()
                                    .rect_filled(rect, 4.0, egui::Color32::from_gray(20));
                                ui.painter().rect_stroke(
                                    rect,
                                    4.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                                );
                                ui.put(
                                    rect,
                                    egui::TextEdit::singleline(&mut self.range_from).frame(false),
                                );

                                ui.label("To:");
                                let desired = egui::Vec2::new(120.0, ui.spacing().interact_size.y);
                                let (rect, _resp) =
                                    ui.allocate_exact_size(desired, egui::Sense::click());
                                ui.painter()
                                    .rect_filled(rect, 4.0, egui::Color32::from_gray(20));
                                ui.painter().rect_stroke(
                                    rect,
                                    4.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                                );
                                ui.put(
                                    rect,
                                    egui::TextEdit::singleline(&mut self.range_to).frame(false),
                                );
                            });
                        });
                    });

                ui.add_space(5.0);

                ui.vertical(|ui| {
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_gray(30))
                        .show(ui, |ui| {
                            ui.set_width(340.0);
                            ui.label(egui::RichText::new("Target Platforms:").strong());

                            ui.horizontal(|ui| {
                                let all_selected = self.platform_win_x86
                                    && self.platform_win_x64
                                    && self.platform_win_arm64
                                    && self.platform_macos_intel
                                    && self.platform_macos_arm64;
                                let toggle_size = egui::Vec2::new(35.0, 18.0);
                                let (rect, resp) =
                                    ui.allocate_exact_size(toggle_size, egui::Sense::click());
                                let bg_off = egui::Color32::from_rgb(48, 48, 48);
                                let bg_on = egui::Color32::from_rgb(110, 110, 110);
                                let knob_color = egui::Color32::from_rgb(230, 230, 230);

                                let radius = toggle_size.y / 2.0;
                                ui.painter().rect_filled(
                                    rect,
                                    radius,
                                    if all_selected { bg_on } else { bg_off },
                                );

                                if !all_selected {
                                    let inset = rect.shrink(1.0);
                                    ui.painter().rect_stroke(
                                        inset,
                                        radius,
                                        egui::Stroke::new(1.0, egui::Color32::from_gray(40)),
                                    );
                                }

                                let knob_radius = radius - 4.0;
                                let knob_x = if all_selected {
                                    rect.right() - radius
                                } else {
                                    rect.left() + radius
                                };
                                let knob_center = egui::pos2(knob_x, rect.center().y);
                                ui.painter()
                                    .circle_filled(knob_center, knob_radius, knob_color);
                                ui.painter().circle_stroke(
                                    knob_center,
                                    knob_radius,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
                                );

                                if resp.clicked() {
                                    if all_selected {
                                        self.select_no_platforms();
                                    } else {
                                        self.select_all_platforms();
                                    }
                                }

                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("All Platforms").size(12.0));
                            });

                            ui.add_space(5.0);

                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.checkbox(&mut self.platform_win_x86, "Windows x86");
                                    ui.checkbox(&mut self.platform_win_x64, "Windows x64");
                                    ui.checkbox(&mut self.platform_win_arm64, "Windows arm64");
                                });

                                ui.add_space(10.0);

                                ui.vertical(|ui| {
                                    ui.checkbox(&mut self.platform_macos_intel, "macOS intel");
                                    ui.checkbox(&mut self.platform_macos_arm64, "macOS arm64");
                                });
                            });
                        });

                    ui.add_space(5.0);

                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_gray(30))
                        .show(ui, |ui| {
                            ui.set_width(340.0);
                            ui.label(egui::RichText::new("Advanced:").strong());

                            ui.add_space(5.0);

                            ui.horizontal(|ui| {
                                ui.label("Max Connections:");

                                let value_label_w = 34.0;
                                let mut slider_w = ui.available_width() - value_label_w - 80.0;
                                if slider_w < 80.0 {
                                    slider_w = 80.0;
                                }
                                let desired = egui::Vec2::new(
                                    slider_w,
                                    ui.spacing().interact_size.y.max(24.0),
                                );
                                let (rect, resp) =
                                    ui.allocate_exact_size(desired, egui::Sense::click_and_drag());

                                let radius = 4.0;
                                let track_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.left(), rect.center().y - 4.0),
                                    egui::pos2(rect.right(), rect.center().y + 4.0),
                                );
                                ui.painter().rect_filled(
                                    track_rect,
                                    radius,
                                    egui::Color32::from_gray(60),
                                );

                                let max_idx = (MAX_CONNECTION_OPTIONS.len() - 1) as f32;
                                let frac = if max_idx > 0.0 {
                                    (self.max_connections_index as f32) / max_idx
                                } else {
                                    0.0
                                };
                                let filled_w = (track_rect.width() * frac).max(0.0);
                                if filled_w > 0.0 {
                                    let filled_rect = egui::Rect::from_min_max(
                                        track_rect.min,
                                        egui::pos2(track_rect.min.x + filled_w, track_rect.max.y),
                                    );
                                    let base_r = 255.0_f32;
                                    let base_g = 200.0_f32;
                                    let base_b = 0.0_f32;
                                    let red_r = 255.0_f32;
                                    let red_g = 0.0_f32;
                                    let red_b = 0.0_f32;
                                    let blend = frac.clamp(0.0, 1.0);
                                    let r = (base_r * (1.0 - blend) + red_r * blend).round() as u8;
                                    let g = (base_g * (1.0 - blend) + red_g * blend).round() as u8;
                                    let b = (base_b * (1.0 - blend) + red_b * blend).round() as u8;
                                    ui.painter().rect_filled(
                                        filled_rect,
                                        radius,
                                        egui::Color32::from_rgb(r, g, b),
                                    );
                                }

                                let max_idx_usize = max_idx as usize;
                                if max_idx_usize >= 2 {
                                    for i in 1..max_idx_usize {
                                        let fx = track_rect.left()
                                            + (i as f32 / max_idx) * track_rect.width();
                                        let tick_min = egui::pos2(fx, track_rect.top() - 3.0);
                                        let tick_max = egui::pos2(fx, track_rect.bottom() + 3.0);
                                        ui.painter().line_segment(
                                            [tick_min, tick_max],
                                            egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
                                        );
                                    }
                                }

                                let knob_x = track_rect.left() + frac * track_rect.width();
                                let knob_center = egui::pos2(knob_x, track_rect.center().y);
                                let knob_radius = 8.0;
                                ui.painter().circle_filled(
                                    knob_center,
                                    knob_radius,
                                    egui::Color32::from_rgb(240, 240, 240),
                                );
                                ui.painter().circle_stroke(
                                    knob_center,
                                    knob_radius,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
                                );

                                if let Some(pointer_pos) = resp.interact_pointer_pos() {
                                    if resp.dragged() || resp.clicked() {
                                        let local_x = (pointer_pos.x - track_rect.left())
                                            .clamp(0.0, track_rect.width());
                                        let new_frac = if track_rect.width() > 0.0 {
                                            local_x / track_rect.width()
                                        } else {
                                            0.0
                                        };
                                        let max_idx_usize =
                                            (MAX_CONNECTION_OPTIONS.len() - 1) as f32;
                                        let idx_f = (new_frac * max_idx_usize).round();
                                        self.max_connections_index = idx_f as usize;
                                    }
                                }

                                ui.add_space(6.0);

                                let val_text = format!(
                                    "{}",
                                    MAX_CONNECTION_OPTIONS[self.max_connections_index]
                                );
                                ui.add_sized(
                                    egui::Vec2::new(value_label_w, ui.spacing().interact_size.y),
                                    egui::Label::new(val_text),
                                );
                            });

                            ui.add_space(5.0);

                            ui.checkbox(&mut self.report_unknown, "Report unknown versions");
                        });
                });
            });

            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let btn_size = egui::Vec2::new(120.0, 28.0);

                if self.is_searching {
                    if self.is_paused {
                        if ui
                            .add_sized(btn_size, egui::Button::new("▶ Resume"))
                            .clicked()
                        {
                            self.is_paused = false;
                            self.pause_flag.store(false, Ordering::Relaxed);
                        }
                    } else {
                        if ui
                            .add_sized(btn_size, egui::Button::new("⏸ Pause"))
                            .clicked()
                        {
                            self.is_paused = true;
                            self.pause_flag.store(true, Ordering::Relaxed);
                            self.progress_text = "Paused".to_string();
                        }
                    }

                    if ui
                        .add_sized(btn_size, egui::Button::new("⏹ Stop"))
                        .clicked()
                    {
                        self.stop_search();
                    }
                } else {
                    if ui
                        .add_sized(btn_size, egui::Button::new("▶ Start Search"))
                        .clicked()
                    {
                        self.start_search();
                    }
                }

                ui.add_enabled_ui(!self.is_searching, |ui| {
                    if ui
                        .add_sized(btn_size, egui::Button::new("🗑 Clear Results"))
                        .clicked()
                    {
                        self.clear_results();
                    }
                });

                if self.is_searching {
                    ui.separator();
                    if self.is_paused {
                        ui.label(
                            egui::RichText::new("● Paused")
                                .color(egui::Color32::from_rgb(255, 200, 0)),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("● Searching...")
                                .color(egui::Color32::from_rgb(0, 255, 0)),
                        );
                    }
                }
            });

            if self.is_searching || self.progress > 0.0 {
                ui.add_space(5.0);
                ui.add(egui::ProgressBar::new(self.progress).text(&self.progress_text));
            }

            ui.add_space(8.0);

            ui.label(egui::RichText::new("Search Results").strong());

            let available_height = ui.available_height() - 10.0;

            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_gray(20))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(available_height)
                        .show(ui, |ui| {
                            let mut read_only: &str = &self.displayed_results;
                            ui.add(
                                egui::TextEdit::multiline(&mut read_only)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(20)
                                    .font(egui::TextStyle::Monospace)
                                    .frame(false),
                            );
                        });
                });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([750.0, 650.0])
            .with_min_inner_size([750.0, 650.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_title("LoaderSpot"),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "LoaderSpot",
        options,
        Box::new(|_cc| Ok(Box::new(SpotifyFinderApp::default()))),
    )
}
