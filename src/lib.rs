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
    update_command: Option<String>,
    extra_notice: Option<String>,
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
            update_command: None,
            extra_notice: None,
            receiver: Mutex::new(None),
        }
    }

    pub fn with_update_command(mut self, update_command: impl Into<String>) -> Self {
        self.update_command = Some(update_command.into());
        self
    }

    pub fn with_extra_notice(mut self, extra_notice: impl Into<String>) -> Self {
        self.extra_notice = Some(extra_notice.into());
        self
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
            eprint!("{}", self.notice(latest_version));
        }
    }

    /// See [`print_warning`](Self::print_warning) for clap integration notes.
    pub fn print_warning_sync(&self) {
        if let Some(ref latest_version) = self.recv_update(Duration::from_secs(6)) {
            eprint!("{}", self.notice(latest_version));
        }
    }

    fn notice(&self, latest_version: &str) -> String {
        format_notice(
            &self.tool_name,
            &self.current_version,
            latest_version,
            self.update_command.as_deref(),
            self.extra_notice.as_deref(),
        )
    }
}

fn format_notice(
    tool_name: &str,
    current_version: &str,
    latest_version: &str,
    update_command: Option<&str>,
    extra_notice: Option<&str>,
) -> String {
    let update_command =
        update_command.map_or_else(|| format!("cargo binstall {}", tool_name), str::to_string);
    let mut notice = format!(
        "Note: A newer version of {} is available (current: {}, latest: {})\n      Run: {}\n",
        tool_name, current_version, latest_version, update_command
    );
    if let Some(extra_notice) = extra_notice {
        notice.push_str(extra_notice);
        notice.push('\n');
    }
    notice
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
    parse_check_interval(std::env::var(UPDATE_CHECK_INTERVAL_ENV).ok().as_deref())
}

fn parse_check_interval(value: Option<&str>) -> u64 {
    value
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_names_tool_versions_and_binstall_command() {
        assert_eq!(
            format_notice("demo", "0.1.0", "99.0.0", None, None),
            "Note: A newer version of demo is available (current: 0.1.0, latest: 99.0.0)\n      Run: cargo binstall demo\n"
        );
    }

    #[test]
    fn notice_uses_custom_update_command() {
        assert_eq!(
            format_notice("demo", "0.1.0", "99.0.0", Some("demo update"), None),
            "Note: A newer version of demo is available (current: 0.1.0, latest: 99.0.0)\n      Run: demo update\n"
        );
    }

    #[test]
    fn notice_appends_extra_line() {
        assert_eq!(
            format_notice("demo", "0.1.0", "99.0.0", None, Some("ACTION REQUIRED: demo is out of date")),
            "Note: A newer version of demo is available (current: 0.1.0, latest: 99.0.0)\n      Run: cargo binstall demo\nACTION REQUIRED: demo is out of date\n"
        );
    }

    #[test]
    fn notice_combines_custom_command_and_extra_line() {
        assert_eq!(
            format_notice("demo", "0.1.0", "99.0.0", Some("demo update"), Some("ACTION REQUIRED: demo is out of date")),
            "Note: A newer version of demo is available (current: 0.1.0, latest: 99.0.0)\n      Run: demo update\nACTION REQUIRED: demo is out of date\n"
        );
    }

    #[test]
    fn builders_configure_the_notice() {
        let checker = VersionChecker::with_check_interval("demo", "0.1.0", Duration::from_secs(60))
            .with_update_command("demo update")
            .with_extra_notice("ACTION REQUIRED: demo is out of date");
        assert_eq!(
            checker.notice("99.0.0"),
            "Note: A newer version of demo is available (current: 0.1.0, latest: 99.0.0)\n      Run: demo update\nACTION REQUIRED: demo is out of date\n"
        );
    }

    #[test]
    fn higher_version_is_newer() {
        assert!(is_newer_version("0.2.4", "0.3.0"));
    }

    #[test]
    fn v_prefix_is_ignored() {
        assert!(is_newer_version("v0.2.4", "0.2.5"));
        assert!(!is_newer_version("0.2.4", "v0.2.4"));
    }

    #[test]
    fn longer_version_is_newer_than_its_prefix() {
        assert!(is_newer_version("0.2", "0.2.1"));
        assert!(!is_newer_version("0.2.1", "0.2"));
    }

    #[test]
    fn equal_or_older_version_is_not_newer() {
        assert!(!is_newer_version("0.2.4", "0.2.4"));
        assert!(!is_newer_version("0.3.0", "0.2.4"));
    }

    #[test]
    fn check_interval_defaults_when_unset() {
        assert_eq!(parse_check_interval(None), DEFAULT_CHECK_INTERVAL_SECONDS);
    }

    #[test]
    fn check_interval_accepts_positive_seconds() {
        assert_eq!(parse_check_interval(Some("3600")), 3600);
    }

    #[test]
    fn check_interval_falls_back_on_zero_or_garbage() {
        assert_eq!(
            parse_check_interval(Some("0")),
            DEFAULT_CHECK_INTERVAL_SECONDS
        );
        assert_eq!(
            parse_check_interval(Some("soon")),
            DEFAULT_CHECK_INTERVAL_SECONDS
        );
    }
}
