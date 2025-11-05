use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use regex::Regex;
use scraper::{Html, Selector};
use urlencoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
enum PlatformArch {
    WinX86,
    WinX64,
    WinArm64,
    MacOsIntel,
    MacOsArm64,
}

impl PlatformArch {
    fn path_template(&self) -> &str {
        match self {
            PlatformArch::WinX86 => "win32-x86/spotify_installer-{version}-{number}.exe",
            PlatformArch::WinX64 => "win32-x86_64/spotify_installer-{version}-{number}.exe",
            PlatformArch::WinArm64 => "win32-arm64/spotify_installer-{version}-{number}.exe",
            PlatformArch::MacOsIntel => "osx-x86_64/spotify-autoupdate-{version}-{number}.tbz",
            PlatformArch::MacOsArm64 => "osx-arm64/spotify-autoupdate-{version}-{number}.tbz",
        }
    }

    fn to_string(&self) -> &str {
        match self {
            PlatformArch::WinX86 => "WIN32",
            PlatformArch::WinX64 => "WIN64",
            PlatformArch::WinArm64 => "WIN-ARM64",
            PlatformArch::MacOsIntel => "OSX",
            PlatformArch::MacOsArm64 => "OSX-ARM64",
        }
    }
}

struct UrlGenerator;

impl UrlGenerator {
    const BASE_URL: &'static str = "https://upgrade.scdn.co/upgrade/client/";

    fn generate_url(platform: PlatformArch, version: &str, number: i32) -> String {
        let path = platform
            .path_template()
            .replace("{version}", version)
            .replace("{number}", &number.to_string());
        format!("{}{}", Self::BASE_URL, path)
    }

}

fn extract_base_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 3 {
        format!("{}.{}.{}", parts[0], parts[1], parts[2])
    } else {
        version.to_string()
    }
}

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

async fn check_url(client: &Client, url: String, platform: PlatformArch) -> Option<(String, PlatformArch)> {
    match client.head(&url).send().await {
        Ok(response) if response.status().is_success() => Some((url, platform)),
        _ => None,
    }
}

async fn search_installers(
    client: &Client,
    version: &str,
    start: i32,
    end: i32,
    platform: PlatformArch,
    max_connections: usize,
) -> Vec<(String, PlatformArch)> {
    let semaphore = Arc::new(Semaphore::new(max_connections));
    let mut tasks = Vec::new();
    let found_urls = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    for number in start..=end {
        let url = UrlGenerator::generate_url(platform, version, number);
        let client = client.clone();
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let found_urls_clone = found_urls.clone();

        let task = tokio::spawn(async move {
            let result = check_url(&client, url, platform).await;
            drop(permit);

            if let Some(found_data) = result {
                let mut guard = found_urls_clone.lock().await;
                guard.push(found_data);
            }
        });

        tasks.push(task);
    }

    for task in tasks {
        let _ = task.await;
    }

    let guard = found_urls.lock().await;
    guard.clone()
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None, disable_version_flag = true)]
struct Cli {
    /// Spotify version(s) to search for
    #[clap(long, required = true, use_value_delimiter = true, value_delimiter = ',')]
    version: Vec<String>,

    /// Range of build numbers to check (e.g., 0-5000)
    #[clap(long, default_value = "0-5000")]
    range: String,

    /// Architecture(s)
    #[clap(long, use_value_delimiter = true, value_delimiter = ',', value_parser = ["x86", "x64", "arm64", "intel", "all"], default_value = "all")]
    arch: Vec<String>,

    /// Platform(s)
    #[clap(long, name = "os", use_value_delimiter = true, value_delimiter = ',', value_parser = ["win", "mac", "all"], default_value = "all")]
    platform: Vec<String>,

    /// Number of concurrent connections
    #[clap(long, default_value_t = 100)]
    connections: usize,

    /// URL to send the found versions to (Google Apps Script)
    #[clap(long)]
    gas_url: Option<String>,

    /// Source of the version
    #[clap(long)]
    source: Option<String>,
}

fn parse_range(range_str: &str) -> (i32, i32) {
    let parts: Vec<&str> = range_str.split('-').collect();
    if parts.len() == 2 {
        let start = parts[0].parse().unwrap_or(0);
        let end = parts[1].parse().unwrap_or(5000);
        (start, end)
    } else {
        (0, 5000)
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let platforms = if cli.platform.contains(&"all".to_string()) {
        vec!["win", "mac"]
    } else {
        cli.platform.iter().map(|s| s.as_str()).collect()
    };

    let arches = if cli.arch.contains(&"all".to_string()) {
        if platforms.contains(&"win") && platforms.contains(&"mac") {
             vec!["x86", "x64", "arm64", "intel", "arm64"]
        } else if platforms.contains(&"win") {
            vec!["x86", "x64", "arm64"]
        } else if platforms.contains(&"mac") {
            vec!["intel", "arm64"]
        } else {
            vec![]
        }
    } else {
        cli.arch.iter().map(|s| s.as_str()).collect()
    };

    let mut platform_arches = Vec::new();
    for platform in &platforms {
        for arch in &arches {
            let platform_arch = match (*platform, *arch) {
                ("win", "x86") => Some(PlatformArch::WinX86),
                ("win", "x64") => Some(PlatformArch::WinX64),
                ("win", "arm64") => Some(PlatformArch::WinArm64),
                ("mac", "intel") => Some(PlatformArch::MacOsIntel),
                ("mac", "arm64") => Some(PlatformArch::MacOsArm64),
                _ => None,
            };
            if let Some(pa) = platform_arch {
                if !platform_arches.contains(&pa) {
                    platform_arches.push(pa);
                }
            }
        }
    }

    if platform_arches.is_empty() {
        eprintln!("Error: No valid platform and architecture combinations provided.");
        std::process::exit(1);
    }

    let connections = cli.connections.clamp(50, 300);
    let range = cli.range.clone();
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    let spinner_style = ProgressStyle::with_template("{spinner}")
        .unwrap()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"); // A classic rotating spinner
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style);
    pb.enable_steady_tick(Duration::from_millis(80));

    let version_clone = cli.version.clone();
    let client_clone = client.clone();
    let gas_url_clone = cli.gas_url.clone();
    let source_clone = cli.source.clone();
    let search_task = tokio::spawn(async move {
        let mut all_found_urls = Vec::new();

        for version in &version_clone {
            let mut arches_to_search = platform_arches.clone();

            if !should_use_win_x86(version) && arches_to_search.contains(&PlatformArch::WinX86) {
                if version_clone.len() == 1 && arches_to_search.len() == 1 {
                    eprintln!("Warning: x86 architecture for Windows is no longer supported for versions newer than 1.2.53.");
                    continue;
                }
                arches_to_search.retain(|&p| p != PlatformArch::WinX86);
            }

            if gas_url_clone.is_some() && source_clone.is_some() {
                // "Лесенка"
                let mut start_number = 0;
                let mut before_enter = 1000;
                let additional_searches = 15;
                let increment = 1000;

                // Первый проход
                for &platform_arch in &arches_to_search {
                    let found = search_installers(&client_clone, version, start_number, before_enter, platform_arch, connections).await;
                    all_found_urls.extend(found);
                }

                // Дополнительные поиски
                for _ in 0..additional_searches {
                    let latest_urls = get_latest_urls(&all_found_urls, &[version.to_string()], source_clone.as_deref().unwrap_or(""));
                    let target_len = arches_to_search.iter().filter(|&&p| p != PlatformArch::WinX86 || should_use_win_x86(version)).count();

                    if latest_urls.len() >= target_len + 2 { // +2 for version and source
                        break;
                    }

                    start_number = before_enter + 1;
                    before_enter += increment;

                    let mut missing_arches = Vec::new();
                    for &platform_arch in &arches_to_search {
                        if !latest_urls.contains_key(platform_arch.to_string()) {
                            missing_arches.push(platform_arch);
                        }
                    }
                    
                    for &platform_arch in &missing_arches {
                        let found = search_installers(&client_clone, version, start_number, before_enter, platform_arch, connections).await;
                        all_found_urls.extend(found);
                    }
                }
            } else {
                // Стандартный поиск
                let (start, end) = parse_range(&range);
                for &platform_arch in &arches_to_search {
                    let found = search_installers(&client_clone, version, start, end, platform_arch, connections).await;
                    all_found_urls.extend(found);
                }
            }
        }
        all_found_urls
    });

    let all_found_urls = search_task.await.unwrap();

    pb.finish_and_clear();

    match (&cli.gas_url, &cli.source) {
        (Some(gas_url), Some(source)) => {
            let latest_urls = get_latest_urls(&all_found_urls, &cli.version, source);
            send_to_gas(client, gas_url, latest_urls).await;
        }
        (Some(_), None) => {
            eprintln!("Warning: --gas-url is provided, but --source is missing. Skipping sending to GAS.");
            let json_output = serde_json::to_string_pretty(&all_found_urls).unwrap();
            println!("{}", json_output);
        }
        (None, Some(_)) => {
            eprintln!("Warning: --source is provided, but --gas-url is missing. Skipping sending to GAS.");
            let json_output = serde_json::to_string_pretty(&all_found_urls).unwrap();
            println!("{}", json_output);
        }
        (None, None) => {
            let json_output = serde_json::to_string_pretty(&all_found_urls).unwrap();
            println!("{}", json_output);
        }
    }
}

fn get_latest_urls(
    found_urls: &[(String, PlatformArch)],
    versions: &[String],
    source: &str,
) -> HashMap<String, String> {
    let mut platform_urls = HashMap::new();
    let version_pattern = Regex::new(r"-(\d+)\.(exe|tbz)$").unwrap();

    for (url, platform) in found_urls {
        if let Some(captures) = version_pattern.captures(url) {
            if let Some(version_number_match) = captures.get(1) {
                let version_number = version_number_match.as_str().parse::<u32>().unwrap();
                let platform_key = platform.to_string().to_string();

                let entry = platform_urls.entry(platform_key).or_insert_with(|| (url.clone(), version_number));

                if version_number > entry.1 {
                    *entry = (url.clone(), version_number);
                }
            }
        }
    }

    let mut latest_urls: HashMap<String, String> = platform_urls
        .into_iter()
        .map(|(k, (v, _))| (k, v))
        .collect();

    if !versions.is_empty() {
        latest_urls.insert("version".to_string(), versions.join(", "));
    }
    
    latest_urls.insert("source".to_string(), source.to_string());


    if latest_urls.is_empty() {
        let mut empty_map = HashMap::new();
        empty_map.insert("unknown".to_string(), "unknown".to_string());
        if !versions.is_empty() {
            empty_map.insert("version".to_string(), versions.join(", "));
        }
        empty_map.insert("source".to_string(), source.to_string());
        return empty_map;
    }

    latest_urls
}

async fn send_to_gas(
    client: Client,
    gas_url: &str,
    data: HashMap<String, String>,
) {
    let json_data = serde_json::to_string(&data).unwrap();
    let encoded_json = urlencoding::encode(&json_data);
    let url = format!("{}{}", gas_url, encoded_json);

    match client.get(&url).send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.text().await {
                    Ok(text) => {
                        if text.contains("<div") {
                            let soup = Html::parse_document(&text);
                            let selector = Selector::parse("div[style*='text-align:center']").unwrap();
                            if let Some(div) = soup.select(&selector).next() {
                                println!("Ответ от GAS: {}", div.text().collect::<String>().trim());
                            } else {
                                println!("Не удалось извлечь ответ из HTML");
                            }
                        } else {
                             println!("Ответ от GAS: {}", text.trim());
                        }
                    }
                    Err(e) => eprintln!("Ошибка чтения ответа от GAS: {}", e),
                }
            } else {
                eprintln!("Ошибка при отправке в GAS: {}", response.status());
            }
        }
        Err(e) => eprintln!("Ошибка при отправке запроса в GAS: {}", e),
    }
}
