use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::config::Config;

/// Cache manager for storing tokens and data
pub struct Cache {
    cache_dir: PathBuf,
}

impl Cache {
    /// Create a new cache manager
    pub fn new() -> Result<Self> {
        let cache_dir = Config::cache_dir()?;
        fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache directory: {:?}", cache_dir))?;
        Ok(Self { cache_dir })
    }

    /// Get the path for a cache file
    fn file_path(&self, filename: &str) -> PathBuf {
        self.cache_dir.join(filename)
    }

    /// Save data to cache
    pub fn save<T: Serialize>(&self, filename: &str, data: &T) -> Result<()> {
        let path = self.file_path(filename);
        let content = serde_json::to_string_pretty(data).context("Failed to serialize data")?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write cache file: {:?}", path))?;
        Ok(())
    }

    /// Load data from cache
    pub fn load<T: DeserializeOwned>(&self, filename: &str) -> Result<Option<T>> {
        let path = self.file_path(filename);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read cache file: {:?}", path))?;
        let data = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse cache file: {:?}", path))?;
        Ok(Some(data))
    }

    /// Delete a cache file
    pub fn delete(&self, filename: &str) -> Result<()> {
        let path = self.file_path(filename);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to delete cache file: {:?}", path))?;
        }
        Ok(())
    }

    /// Check if a cache file exists
    pub fn exists(&self, filename: &str) -> bool {
        self.file_path(filename).exists()
    }

    /// Clear all cache files
    pub fn clear(&self) -> Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)
                .with_context(|| format!("Failed to clear cache directory: {:?}", self.cache_dir))?;
            fs::create_dir_all(&self.cache_dir).with_context(|| {
                format!("Failed to recreate cache directory: {:?}", self.cache_dir)
            })?;
        }
        Ok(())
    }
}

// Token cache file names
pub const TOKENS_FILE: &str = "tokens.json";
pub const TEAMS_FILE: &str = "teams.json";
pub const CHATS_FILE: &str = "chats.json";
pub const USERS_FILE: &str = "users.json";
pub const ME_FILE: &str = "me.json";
