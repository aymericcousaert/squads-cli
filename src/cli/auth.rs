use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use clap::{Args, Subcommand};
use tokio::time::sleep;

use crate::api::{gen_device_code, gen_refresh_token_from_device_code, TeamsClient};
use crate::config::Config;

use super::output::{print_error, print_info, print_success, print_warning};

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

pub async fn execute(cmd: AuthCommand, config: &Config) -> Result<()> {
    match cmd.command {
        AuthSubcommand::Login {
            tenant,
            copy_code,
            no_browser,
        } => login(config, tenant, copy_code, no_browser).await,
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
) -> Result<()> {
    let tenant = tenant.as_ref().unwrap_or(&config.auth.tenant);

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
    let max_attempts = 60; // 5 minutes with 5 second intervals

    loop {
        sleep(Duration::from_secs(5)).await;
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
                if attempts >= max_attempts {
                    print_error("Authentication timed out. Please try again.");
                    return Ok(());
                }
                // Continue polling
            }
        }
    }
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
