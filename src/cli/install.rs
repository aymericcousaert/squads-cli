use anyhow::{Context, Result};
use std::env;
use std::fs;

use super::output::{print_success, print_warning};

pub fn execute() -> Result<()> {
    // 1. Get current executable path
    let current_exe = env::current_exe().context("Failed to get current executable path")?;

    // 2. Determine destination (~/.local/bin/squads-cli)
    let home = directories::BaseDirs::new()
        .context("Could not find home directory")?
        .home_dir()
        .to_path_buf();
    let bin_dir = home.join(".local").join("bin");

    let dest = bin_dir.join("squads-cli");

    // 3. Create directory if needed
    if !bin_dir.exists() {
        fs::create_dir_all(&bin_dir).context("Failed to create ~/.local/bin directory")?;
    }

    if current_exe == dest {
        print_success(&format!("Already installed at {:?}", dest));
        return Ok(());
    }

    if dest.exists() {
        fs::remove_file(&dest).context("Failed to remove existing install")?;
    }

    // 4. Copy binary
    fs::copy(&current_exe, &dest).context(format!(
        "Failed to copy binary from {:?} to {:?}",
        current_exe, dest
    ))?;

    // 5. Ensure it's executable (on Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest, perms)?;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // Clear quarantine/provenance attributes if present
        let _ = Command::new("/usr/bin/xattr").arg("-c").arg(&dest).status();

        let status = Command::new(&dest).arg("--version").status();
        if !status.as_ref().map(|s| s.success()).unwrap_or(false) {
            print_warning("Installed binary failed to run. Try running from target/release or rebuild/install again.");
        }
    }

    print_success(&format!("Successfully installed to {:?}", dest));
    println!("\nMake sure {:?} is in your PATH.", bin_dir);
    println!("You can add it by adding this to your shell profile (~/.zshrc or ~/.bashrc):");
    println!("  export PATH=\"$HOME/.local/bin:$PATH\"");

    Ok(())
}
