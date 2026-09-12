use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::output::print_success;
use crate::config::Config;

const GITHUB_REPO: &str = "aymericcousaert/squads-cli";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    last_check: u64,
    latest_version: String,
}

fn get_asset_name() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "squads-cli-linux-amd64";

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "squads-cli-macos-amd64";

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "squads-cli-macos-arm64";

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "squads-cli-windows-amd64.exe";

    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    return "unsupported";
}

fn get_current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn cache_path() -> Result<PathBuf> {
    let cache_dir = Config::cache_dir()?;
    Ok(cache_dir.join("update_cache.json"))
}

fn load_cache() -> Option<UpdateCache> {
    let path = cache_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_cache(cache: &UpdateCache) -> Result<()> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string(cache)?;
    fs::write(path, content)?;
    Ok(())
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

async fn fetch_latest_release() -> Result<Release> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            GITHUB_REPO
        ))
        .header("User-Agent", "squads-cli")
        .send()
        .await
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("No releases found. The repository may not exist or has no published releases.");
    }

    if !response.status().is_success() {
        bail!(
            "GitHub API returned error: {} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("Unknown")
        );
    }

    response
        .json()
        .await
        .context("Failed to parse release info")
}

/// Split a version into its numbers and its prerelease. A leading `v` and any
/// build metadata are dropped: neither changes the order.
fn parse_version(version: &str) -> (Vec<u64>, Option<&str>) {
    let version = version.trim().trim_start_matches(['v', 'V']);
    let version = version.split('+').next().unwrap_or(version);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let numbers = core
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    (numbers, pre)
}

/// Order two prereleases. Identifiers are compared one dot-separated piece at a
/// time, numbers below text, and a shorter list below a longer one.
fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        let order = match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if order != Ordering::Equal {
            return order;
        }
    }
}

/// Order two versions the semver way, so a prerelease sits below the release it
/// leads to.
fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a_numbers, a_pre) = parse_version(a);
    let (b_numbers, b_pre) = parse_version(b);
    for i in 0..a_numbers.len().max(b_numbers.len()) {
        let a = a_numbers.get(i).copied().unwrap_or(0);
        let b = b_numbers.get(i).copied().unwrap_or(0);
        if a != b {
            return a.cmp(&b);
        }
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_prerelease(a, b),
    }
}

/// True when moving from `current` to `candidate` is a step forward. An older
/// release must never be offered as an update.
fn is_upgrade(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == Ordering::Greater
}

/// Check for updates automatically (called on startup)
/// Returns Some(version) if an update is available
pub async fn check_for_update(config: &Config) -> Option<String> {
    // Skip if auto-update is disabled
    if !config.update.auto_check {
        return None;
    }

    // Skip if env var disables updates
    if std::env::var("SQUADS_CLI_NO_UPDATE").is_ok() {
        return None;
    }

    let current_version = format!("v{}", get_current_version());
    let check_interval = config.update.check_interval_hours * 3600;

    // Check cache first
    if let Some(cache) = load_cache() {
        let elapsed = current_timestamp().saturating_sub(cache.last_check);
        if elapsed < check_interval {
            // Cache is fresh, use cached version
            if is_upgrade(&cache.latest_version, &current_version) {
                return Some(cache.latest_version);
            }
            return None;
        }
    }

    // Fetch latest version (silently fail if network issues)
    let release = fetch_latest_release().await.ok()?;

    // Update cache
    let cache = UpdateCache {
        last_check: current_timestamp(),
        latest_version: release.tag_name.clone(),
    };
    let _ = save_cache(&cache);

    if is_upgrade(&release.tag_name, &current_version) {
        Some(release.tag_name)
    } else {
        None
    }
}

/// Notify user about available update
pub fn notify_update_available(new_version: &str) {
    eprintln!(
        "\n\x1b[33m⚡ Update available: v{} → {}\x1b[0m",
        get_current_version(),
        new_version
    );
    eprintln!("\x1b[33m   Run `squads-cli update` to update\x1b[0m\n");
}

/// Perform the update
pub async fn execute() -> Result<()> {
    let asset_name = get_asset_name();
    if asset_name == "unsupported" {
        bail!("Unsupported platform. Please build from source.");
    }

    println!("🔍 Checking for updates...");

    let release = fetch_latest_release().await?;

    let current_version = format!("v{}", get_current_version());
    println!("Current version: {}", current_version);
    println!("Latest version:  {}", release.tag_name);

    if !is_upgrade(&release.tag_name, &current_version) {
        print_success("Already up to date!");
        return Ok(());
    }

    // Find the right asset for this platform
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .context(format!(
            "No binary found for this platform ({})",
            asset_name
        ))?;

    println!("\n📥 Downloading {}...", asset.name);

    // Download the binary
    let client = reqwest::Client::new();
    let response = client
        .get(&asset.browser_download_url)
        .header("User-Agent", "squads-cli")
        .send()
        .await
        .context("Failed to download binary")?;

    let bytes = response.bytes().await.context("Failed to read binary")?;

    // Determine destination
    let home = directories::BaseDirs::new()
        .context("Could not find home directory")?
        .home_dir()
        .to_path_buf();
    let bin_dir = home.join(".local").join("bin");

    #[cfg(windows)]
    let dest = bin_dir.join("squads-cli.exe");
    #[cfg(not(windows))]
    let dest = bin_dir.join("squads-cli");

    // Create directory if needed
    if !bin_dir.exists() {
        fs::create_dir_all(&bin_dir).context("Failed to create ~/.local/bin directory")?;
    }

    // Write to temp file first, then rename (atomic on most systems)
    let temp_dest = dest.with_extension("tmp");
    {
        let mut file = fs::File::create(&temp_dest).context("Failed to create temp file")?;
        file.write_all(&bytes).context("Failed to write binary")?;
    }

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_dest, perms)?;
    }

    // Remove old binary and rename temp to final
    if dest.exists() {
        fs::remove_file(&dest).context("Failed to remove old binary")?;
    }
    fs::rename(&temp_dest, &dest).context("Failed to install binary")?;

    // Update cache
    let cache = UpdateCache {
        last_check: current_timestamp(),
        latest_version: release.tag_name.clone(),
    };
    let _ = save_cache(&cache);

    print_success(&format!(
        "Updated to {} (installed at {:?})",
        release.tag_name, dest
    ));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_version_is_not_an_upgrade() {
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert!(!is_upgrade("v0.4.0", "v0.4.0"));
    }

    #[test]
    fn an_older_release_is_not_an_upgrade() {
        assert!(!is_upgrade("v0.3.2", "v0.4.0"));
        assert!(!is_upgrade("v0.9.9", "v1.0.0"));
        assert!(!is_upgrade("v1.2.3", "v1.10.0"));
    }

    #[test]
    fn a_newer_release_is_an_upgrade() {
        assert!(is_upgrade("v0.4.1", "v0.4.0"));
        assert!(is_upgrade("v0.10.0", "v0.9.9"));
        assert!(is_upgrade("v2.0.0", "v1.99.99"));
    }

    #[test]
    fn a_prerelease_sits_below_its_release() {
        assert!(!is_upgrade("v1.0.0-rc.1", "v1.0.0"));
        assert!(is_upgrade("v1.0.0", "v1.0.0-rc.1"));
        assert!(is_upgrade("v1.0.0-rc.2", "v1.0.0-rc.1"));
        assert!(is_upgrade("v1.0.0-rc.1", "v1.0.0-alpha.1"));
        assert!(is_upgrade("v1.0.0-alpha.1", "v1.0.0-alpha"));
    }

    #[test]
    fn build_metadata_and_short_versions_still_order() {
        assert_eq!(
            compare_versions("v1.2.3+build.5", "v1.2.3"),
            Ordering::Equal
        );
        assert_eq!(compare_versions("v1.2", "v1.2.0"), Ordering::Equal);
        assert!(is_upgrade("v1.3", "v1.2.9"));
    }
}
