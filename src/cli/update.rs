use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::output::print_success;
use crate::config::Config;

const GITHUB_REPO: &str = "aymericcousaert/squads-cli";

#[derive(Debug, Deserialize)]
struct GhRelease {
    #[serde(rename = "tagName")]
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
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

fn fetch_latest_version() -> Result<GhRelease> {
    let output = Command::new("gh")
        .args([
            "release",
            "view",
            "--repo",
            GITHUB_REPO,
            "--json",
            "tagName,assets",
        ])
        .output()
        .context("Failed to run gh CLI. Is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("release not found") || stderr.contains("no releases") {
            bail!("No releases found for {}", GITHUB_REPO);
        }
        if stderr.contains("Could not resolve") || stderr.contains("not found") {
            bail!("Repository {} not found or not accessible", GITHUB_REPO);
        }
        bail!("gh release view failed: {}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("Failed to parse release info from gh")
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

    // Fetch latest version (silently fail if gh not available or network issues)
    let release = fetch_latest_version().ok()?;

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

    let release = fetch_latest_version()?;

    let current_version = format!("v{}", get_current_version());
    println!("Current version: {}", current_version);
    println!("Latest version:  {}", release.tag_name);

    if release.tag_name == current_version {
        print_success("Already up to date!");
        return Ok(());
    }

    // Find the right asset for this platform
    let _asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .context(format!(
            "No binary found for this platform ({})",
            asset_name
        ))?;

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

    println!("\n📥 Downloading {}...", asset_name);

    // Download using gh CLI (handles authentication for private repos)
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let download_output = Command::new("gh")
        .args([
            "release",
            "download",
            &release.tag_name,
            "--repo",
            GITHUB_REPO,
            "--pattern",
            asset_name,
            "--dir",
            temp_dir.path().to_str().unwrap(),
        ])
        .output()
        .context("Failed to run gh release download")?;

    if !download_output.status.success() {
        let stderr = String::from_utf8_lossy(&download_output.stderr);
        bail!("Failed to download release: {}", stderr.trim());
    }

    let downloaded_file = temp_dir.path().join(asset_name);
    if !downloaded_file.exists() {
        bail!("Downloaded file not found");
    }

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&downloaded_file)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&downloaded_file, perms)?;
    }

    // Remove old binary and move new one
    if dest.exists() {
        fs::remove_file(&dest).context("Failed to remove old binary")?;
    }
    fs::copy(&downloaded_file, &dest).context("Failed to install binary")?;

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
