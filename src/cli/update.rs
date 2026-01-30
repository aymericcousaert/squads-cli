use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Write;

use super::output::print_success;

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

pub async fn execute() -> Result<()> {
    let asset_name = get_asset_name();
    if asset_name == "unsupported" {
        bail!("Unsupported platform. Please build from source.");
    }

    println!("🔍 Checking for updates...");

    // Fetch latest release from GitHub
    let client = reqwest::Client::new();
    let release: Release = client
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
        .context("Failed to parse release info")?;

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

    print_success(&format!(
        "Updated to {} (installed at {:?})",
        release.tag_name, dest
    ));

    Ok(())
}
