use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub output: OutputConfig,
    #[serde(default)]
    pub api: ApiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Azure AD tenant (default: "organizations" for multi-tenant)
    #[serde(default = "default_tenant")]
    pub tenant: String,
}

fn default_tenant() -> String {
    "organizations".to_string()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            tenant: default_tenant(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Default output format
    #[serde(default = "default_format")]
    pub default_format: String,
    /// Enable colored output
    #[serde(default = "default_true")]
    pub color: bool,
}

fn default_format() -> String {
    "table".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            default_format: default_format(),
            color: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// API region (emea, amer, apac)
    #[serde(default = "default_region")]
    pub region: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_region() -> String {
    "emea".to_string()
}

fn default_timeout() -> u64 {
    30
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            region: default_region(),
            timeout: default_timeout(),
        }
    }
}

impl Config {
    /// Get the project directories
    pub fn project_dirs() -> Option<ProjectDirs> {
        ProjectDirs::from("", "squads-cli", "squads-cli")
    }

    /// Get the config file path
    pub fn config_path() -> Result<PathBuf> {
        let dirs = Self::project_dirs().context("Could not determine config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get the cache directory
    pub fn cache_dir() -> Result<PathBuf> {
        let dirs = Self::project_dirs().context("Could not determine cache directory")?;
        Ok(dirs.cache_dir().to_path_buf())
    }

    /// Load configuration from file
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config file: {:?}", config_path))?;
            toml::from_str(&content)
                .with_context(|| format!("Failed to parse config file: {:?}", config_path))
        } else {
            Ok(Self::default())
        }
    }

    /// Save configuration to file
    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        // Ensure directory exists
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config file: {:?}", config_path))?;

        Ok(())
    }
}
