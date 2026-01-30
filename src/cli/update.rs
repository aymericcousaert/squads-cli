use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::output::print_success;

const DEFAULT_REPO_PATH: &str = "workspace/squads-cli-repo";

fn find_repo_path() -> Result<PathBuf> {
    // Check SQUADS_CLI_REPO env var first
    if let Ok(path) = env::var("SQUADS_CLI_REPO") {
        let path = PathBuf::from(path);
        if path.exists() && path.join(".git").exists() {
            return Ok(path);
        }
    }

    // Try default path relative to home
    let home = directories::BaseDirs::new()
        .context("Could not find home directory")?
        .home_dir()
        .to_path_buf();

    let default_path = home.join(DEFAULT_REPO_PATH);
    if default_path.exists() && default_path.join(".git").exists() {
        return Ok(default_path);
    }

    bail!(
        "Could not find squads-cli repo. Set SQUADS_CLI_REPO environment variable or clone to ~/{}",
        DEFAULT_REPO_PATH
    )
}

pub fn execute() -> Result<()> {
    let repo_path = find_repo_path()?;
    println!("Found repo at: {:?}", repo_path);

    // 1. Git pull
    println!("\n📥 Pulling latest changes...");
    let pull_status = Command::new("git")
        .current_dir(&repo_path)
        .args(["pull"])
        .status()
        .context("Failed to run git pull")?;

    if !pull_status.success() {
        bail!("git pull failed");
    }

    // 2. Build with cargo
    println!("\n🔨 Building release...");
    let build_status = Command::new("cargo")
        .current_dir(&repo_path)
        .args(["build", "--release"])
        .status()
        .context("Failed to run cargo build")?;

    if !build_status.success() {
        bail!("cargo build failed");
    }

    // 3. Copy to ~/.local/bin
    let home = directories::BaseDirs::new()
        .context("Could not find home directory")?
        .home_dir()
        .to_path_buf();
    let bin_dir = home.join(".local").join("bin");
    let dest = bin_dir.join("squads-cli");
    let source = repo_path.join("target").join("release").join("squads-cli");

    if !bin_dir.exists() {
        fs::create_dir_all(&bin_dir).context("Failed to create ~/.local/bin directory")?;
    }

    if dest.exists() {
        fs::remove_file(&dest).context("Failed to remove existing binary")?;
    }

    println!("\n📦 Installing to {:?}...", dest);
    fs::copy(&source, &dest).context("Failed to copy binary")?;

    // Ensure executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest, perms)?;
    }

    // Get new version
    let version_output = Command::new(&dest)
        .arg("--version")
        .output()
        .context("Failed to get version")?;
    let version = String::from_utf8_lossy(&version_output.stdout);

    print_success(&format!("Updated to {}", version.trim()));

    Ok(())
}
