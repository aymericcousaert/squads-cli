use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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

async fn fetch_latest_version() -> Result<Release> {
    let client = reqwest::Client::new();
    client
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            GITHUB_REPO
        ))
        .header("User-Agent", "squads-cli")
        .send()
        .await
        .context("Failed to fetch release info")?
        .json()
        .await
        .context("Failed to parse release info")
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
            if cache.latest_version != current_version {
                return Some(cache.latest_version);
            }
            return None;
        }
    }

    // Fetch latest version (silently fail if network issues)
    let release = fetch_latest_version().await.ok()?;

    // Update cache
    let cache = UpdateCache {
        last_check: current_timestamp(),
        latest_version: release.tag_name.clone(),
    };
    let _ = save_cache(&cache);

    if release.tag_name != current_version {
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

    let release = fetch_latest_version().await?;

    let current_version = format!("v{}", get_current_version());
    println!("Current version: {}", current_version);
    println!("Latest version:  {}", release.tag_name);

    if release.tag_name == current_version {
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
