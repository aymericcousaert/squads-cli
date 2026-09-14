use std::io::Write;
use std::time::Duration;

use anyhow::{anyhow, Result};
use arboard::Clipboard;
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::api::{gen_device_code, gen_refresh_token_from_device_code, TeamsClient};
use crate::config::Config;

use super::output::{print_error, print_info, print_success, print_warning};
use super::OutputFormat;

/// How long the device code is polled for: 60 tries, 5 seconds apart.
const POLL_EVERY: Duration = Duration::from_secs(5);
const POLL_ATTEMPTS: u32 = 60;

#[derive(Args, Debug)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AuthSubcommand {
    /// Login using device code flow
    Login {
        /// Specific tenant ID (default: organizations for multi-tenant)
        #[arg(short, long)]
        tenant: Option<String>,

        /// Copy authentication code to clipboard
        #[arg(short, long)]
        copy_code: bool,

        /// Don't automatically open the browser
        #[arg(long)]
        no_browser: bool,
    },

    /// Check authentication status
    Status,

    /// Logout and clear tokens
    Logout,

    /// Refresh authentication tokens
    Refresh,
}

pub async fn execute(cmd: AuthCommand, config: &Config, format: OutputFormat) -> Result<()> {
    match cmd.command {
        AuthSubcommand::Login {
            tenant,
            copy_code,
            no_browser,
        } => login(config, tenant, copy_code, no_browser, format).await,
        AuthSubcommand::Status => status(config).await,
        AuthSubcommand::Logout => logout(config).await,
        AuthSubcommand::Refresh => refresh(config).await,
    }
}

async fn login(
    config: &Config,
    tenant: Option<String>,
    copy_code: bool,
    no_browser: bool,
    format: OutputFormat,
) -> Result<()> {
    let tenant = tenant.as_ref().unwrap_or(&config.auth.tenant);

    if matches!(format, OutputFormat::Json) {
        return login_json(config, tenant).await;
    }

    print_info(&format!("Generating device code for tenant: {}", tenant));

    // Generate device code
    let device_code_info = gen_device_code(tenant).await?;

    println!();

    // Copy code to clipboard if requested
    if copy_code {
        match Clipboard::new() {
            Ok(mut clipboard) => {
                if clipboard.set_text(&device_code_info.user_code).is_ok() {
                    print_success(&format!(
                        "Code copied to clipboard: {}",
                        device_code_info.user_code
                    ));
                } else {
                    print_warning("Failed to copy code to clipboard");
                    println!("Enter this code when prompted:");
                    println!("  {}", device_code_info.user_code);
                }
            }
            Err(_) => {
                print_warning("Clipboard not available");
                println!("Enter this code when prompted:");
                println!("  {}", device_code_info.user_code);
            }
        }
    } else {
        println!("Enter this code when prompted:");
        println!("  {}", device_code_info.user_code);
    }

    println!();

    // Open browser automatically unless disabled
    if !no_browser {
        print_info(&format!(
            "Opening browser: {}",
            device_code_info.verification_url
        ));
        if let Err(e) = open::that(&device_code_info.verification_url) {
            print_warning(&format!("Failed to open browser: {}", e));
            println!("Please open this URL manually:");
            println!("  {}", device_code_info.verification_url);
        }
    } else {
        println!("To sign in, open a browser and go to:");
        println!("  {}", device_code_info.verification_url);
    }

    println!();
    print_info("Waiting for authorization...");

    // Poll for authorization
    let mut attempts = 0;

    loop {
        sleep(POLL_EVERY).await;
        attempts += 1;

        match gen_refresh_token_from_device_code(&device_code_info.device_code, tenant).await {
            Ok(refresh_token) => {
                // Store the token
                let client = TeamsClient::new(config)?;
                client.store_refresh_token(refresh_token)?;

                println!();
                print_success("Successfully authenticated!");
                print_info("You can now use squads-cli commands.");
                return Ok(());
            }
            Err(_) => {
                if attempts >= POLL_ATTEMPTS {
                    print_error("Authentication timed out. Please try again.");
                    return Ok(());
                }
                // Continue polling
            }
        }
    }
}

/// The login a program drives: one JSON object per line, and nothing else.
///
/// No browser and no clipboard here. The caller owns the window the user is
/// looking at, so it decides when the code is copied and the browser opens.
async fn login_json(config: &Config, tenant: &str) -> Result<()> {
    let info = gen_device_code(tenant).await?;
    emit(&device_code_line(
        &info.user_code,
        &info.verification_url,
        &info.expires_in,
    ));

    for _ in 0..POLL_ATTEMPTS {
        sleep(POLL_EVERY).await;
        // Every failure here is "not authorised yet" until the code expires,
        // which the attempt count stands in for.
        if let Ok(refresh_token) =
            gen_refresh_token_from_device_code(&info.device_code, tenant).await
        {
            TeamsClient::new(config)?.store_refresh_token(refresh_token)?;
            emit(&json!({ "event": "authenticated" }));
            return Ok(());
        }
    }

    Err(anyhow!("Authentication timed out. Please try again."))
}

fn device_code_line(user_code: &str, verification_url: &str, expires_in: &str) -> Value {
    json!({
        "event": "device_code",
        "user_code": user_code,
        "verification_url": verification_url,
        // A string in the Microsoft response, seconds in ours.
        "expires_in": expires_in.parse::<u64>().unwrap_or(900),
    })
}

/// Flushed on every line: the reader is drawing a window from this, not
/// reading a file at the end.
fn emit(line: &Value) {
    println!("{}", line);
    let _ = std::io::stdout().flush();
}

async fn status(config: &Config) -> Result<()> {
    let client = TeamsClient::new(config)?;

    if client.is_authenticated() {
        print_success("Authenticated");

        // Try to get user info
        match client.get_me().await {
            Ok(profile) => {
                if let Some(name) = profile.display_name {
                    println!("  User: {}", name);
                }
                if let Some(email) = profile.mail {
                    println!("  Email: {}", email);
                }
            }
            Err(_) => {
                print_info("Token may be expired. Run 'squads-cli auth refresh' to renew.");
            }
        }
    } else {
        print_error("Not authenticated");
        print_info("Run 'squads-cli auth login' to authenticate.");
    }

    Ok(())
}

async fn logout(config: &Config) -> Result<()> {
    let client = TeamsClient::new(config)?;
    client.clear_tokens()?;
    print_success("Logged out successfully");
    Ok(())
}

async fn refresh(config: &Config) -> Result<()> {
    let client = TeamsClient::new(config)?;

    if !client.is_authenticated() {
        print_error("Not authenticated. Run 'squads-cli auth login' first.");
        return Ok(());
    }

    print_info("Refreshing tokens...");

    // Getting a token will automatically refresh if needed
    match client.get_me().await {
        Ok(_) => {
            print_success("Tokens refreshed successfully");
        }
        Err(e) => {
            print_error(&format!("Failed to refresh tokens: {}", e));
            print_info("You may need to re-authenticate with 'squads-cli auth login'");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::device_code_line;

    #[test]
    fn the_device_code_line_carries_what_a_window_needs() {
        let line = device_code_line("ABCD1234", "https://microsoft.com/devicelogin", "900");
        assert_eq!(line["event"], "device_code");
        assert_eq!(line["user_code"], "ABCD1234");
        assert_eq!(
            line["verification_url"],
            "https://microsoft.com/devicelogin"
        );
        assert_eq!(line["expires_in"], 900);
    }

    #[test]
    fn an_unreadable_lifetime_falls_back_to_fifteen_minutes() {
        assert_eq!(device_code_line("A", "https://x", "")["expires_in"], 900);
    }
}
