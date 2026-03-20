use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_CHECK_INTERVAL_SECONDS: u64 = 86400;
const CHECK_TIMEOUT_SECONDS: u64 = 5;
const UPDATE_CHECK_INTERVAL_ENV: &str = "MOZTOOLS_UPDATE_CHECK_INTERVAL_SECONDS";

#[derive(Debug, Serialize, Deserialize)]
struct ToolVersionInfo {
    last_check: u64,
    #[serde(default)]
    latest: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct VersionCache {
    #[serde(flatten)]
    tools: HashMap<String, ToolVersionInfo>,
}

#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    crate_info: CrateInfo,
}

#[derive(Debug, Deserialize)]
struct CrateInfo {
    max_version: String,
}

pub struct VersionChecker {
    tool_name: String,
    current_version: String,
    check_interval: Duration,
    receiver: Mutex<Option<Receiver<Option<String>>>>,
}

impl VersionChecker {
    pub fn new(tool_name: impl Into<String>, current_version: impl Into<String>) -> Self {
        Self::with_check_interval(
            tool_name,
            current_version,
            Duration::from_secs(get_check_interval_seconds()),
        )
    }

    pub fn with_check_interval(
        tool_name: impl Into<String>,
        current_version: impl Into<String>,
        check_interval: Duration,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            current_version: current_version.into(),
            check_interval,
            receiver: Mutex::new(None),
        }
    }

    pub fn check_async(&self) {
        if std::env::var("MOZTOOLS_UPDATE_CHECK").unwrap_or_default() == "0" {
            return;
        }

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if let Ok(mut guard) = self.receiver.lock() {
            *guard = Some(rx);
        }

        let tool_name = self.tool_name.clone();
        let current_version = self.current_version.clone();
        let check_interval = self.check_interval;

        thread::spawn(move || {
            let result = check_version(&tool_name, &current_version, check_interval);
            let _ = tx.send(result);
        });
    }

    fn recv_update(&self, timeout: Duration) -> Option<String> {
        let mut guard = self.receiver.lock().ok()?;
        let rx = guard.as_ref()?;
        match rx.recv_timeout(timeout) {
            Ok(result) => {
                *guard = None;
                result
            }
            Err(_) => None,
        }
    }

    /// # Clap integration
    ///
    /// If your CLI uses clap, you must use `Parser::try_parse()` instead of
    /// `parse()`. clap's `parse()` calls `std::process::exit()` on `--help`
    /// and `--version`, which will skip this call entirely. With `try_parse()`,
    /// call `e.print()` first in the `Err` branch, then `print_warning()`, then
    /// `std::process::exit(e.exit_code())`. See the README for a full example.
    pub fn print_warning(&self) {
        if let Some(ref latest_version) = self.recv_update(Duration::from_millis(500)) {
            self.print_update_message(latest_version);
        }
    }

    /// See [`print_warning`](Self::print_warning) for clap integration notes.
    pub fn print_warning_sync(&self) {
        if let Some(ref latest_version) = self.recv_update(Duration::from_secs(6)) {
            self.print_update_message(latest_version);
        }
    }

    fn print_update_message(&self, latest_version: &str) {
        eprintln!(
            "Note: A newer version of {} is available (current: {}, latest: {})",
            self.tool_name, self.current_version, latest_version
        );
        eprintln!("      Run: cargo binstall {}", self.tool_name);
    }
}

fn get_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".mozbuild").join("tool-versions.json"))
}

fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn get_check_interval_seconds() -> u64 {
    std::env::var(UPDATE_CHECK_INTERVAL_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_CHECK_INTERVAL_SECONDS)
}

fn load_cache() -> VersionCache {
    let cache_path = match get_cache_path() {
        Some(path) => path,
        None => return VersionCache::default(),
    };

    if !cache_path.exists() {
        return VersionCache::default();
    }

    fs::read_to_string(&cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &VersionCache) {
    let cache_path = match get_cache_path() {
        Some(path) => path,
        None => return,
    };

    if let Some(parent) = cache_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ =
            fs::File::create(&cache_path).and_then(|mut file| file.write_all(content.as_bytes()));
    }
}

fn fetch_latest_version(tool_name: &str) -> Option<String> {
    let url = format!("https://crates.io/api/v1/crates/{}", tool_name);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(CHECK_TIMEOUT_SECONDS))
        .user_agent(format!("{}/version-check", tool_name))
        .build()
        .ok()?;

    let response: CratesIoResponse = client.get(&url).send().ok()?.json().ok()?;

    Some(response.crate_info.max_version)
}

fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse_version = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect()
    };

    let current_parts = parse_version(current);
    let latest_parts = parse_version(latest);

    for (c, l) in current_parts.iter().zip(latest_parts.iter()) {
        if l > c {
            return true;
        } else if l < c {
            return false;
        }
    }

    latest_parts.len() > current_parts.len()
}

fn check_version(
    tool_name: &str,
    current_version: &str,
    check_interval: Duration,
) -> Option<String> {
    if let Ok(fake) = std::env::var("MOZTOOLS_FAKE_LATEST") {
        return if is_newer_version(current_version, &fake) {
            Some(fake)
        } else {
            None
        };
    }

    let mut cache = load_cache();
    let now = get_current_timestamp();
    let check_interval = check_interval.as_secs();

    if let Some(info) = cache.tools.get(tool_name) {
        if now.saturating_sub(info.last_check) < check_interval {
            if is_newer_version(current_version, &info.latest) {
                return Some(info.latest.clone());
            }
            if is_newer_version(&info.latest, current_version) {
                cache.tools.remove(tool_name);
                save_cache(&cache);
            }
            return None;
        }
    }

    let previous_latest = cache
        .tools
        .get(tool_name)
        .map(|info| info.latest.clone())
        .unwrap_or_default();

    cache.tools.insert(
        tool_name.to_string(),
        ToolVersionInfo {
            last_check: now,
            latest: previous_latest.clone(),
        },
    );
    save_cache(&cache);

    let latest_version = match fetch_latest_version(tool_name) {
        Some(version) => version,
        None => {
            if is_newer_version(current_version, &previous_latest) {
                return Some(previous_latest);
            }
            return None;
        }
    };

    cache.tools.insert(
        tool_name.to_string(),
        ToolVersionInfo {
            last_check: now,
            latest: latest_version.clone(),
        },
    );

    save_cache(&cache);

    if is_newer_version(current_version, &latest_version) {
        Some(latest_version)
    } else {
        None
    }
}
