//! Configuration for tokenless.
//!
//! Stored at `~/.tokenless/config.json`. Controls global feature flags.
//! Environment variable `TOKENLESS_STATS_ENABLED` overrides file config.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Global tokenless configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenlessConfig {
    /// Whether to record compression stats (default: true)
    #[serde(default = "default_true")]
    pub stats_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TokenlessConfig {
    fn default() -> Self {
        Self {
            stats_enabled: true,
        }
    }
}

impl TokenlessConfig {
    fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".tokenless/config.json")
    }

    /// Whether a config file exists on disk.
    pub fn config_file_exists() -> bool {
        Self::config_path().exists()
    }

    /// Load config: env var overrides file config, file config overrides defaults.
    /// Priority: TOKENLESS_STATS_ENABLED env > config.json file > default(true)
    pub fn load() -> Self {
        // Check env var first
        if let Ok(val) = std::env::var("TOKENLESS_STATS_ENABLED") {
            let enabled =
                val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes");
            return Self {
                stats_enabled: enabled,
            };
        }

        // Fall back to file config
        let path = Self::config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save config to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
    }

    /// Returns true if stats recording is enabled (env override or file config).
    pub fn is_stats_enabled(&self) -> bool {
        self.stats_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TokenlessConfig::default();
        assert!(config.is_stats_enabled());
    }

    #[test]
    fn test_load_missing_file() {
        let path = TokenlessConfig::config_path();
        // Backup existing config if present
        let backup = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_file(&path);
        // Clear env override
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
        let config = TokenlessConfig::load();
        assert!(config.is_stats_enabled());
        // Restore backup
        if let Some(content) = backup {
            let _ = std::fs::write(&path, content);
        }
    }

    #[test]
    fn test_load_invalid_json() {
        let path = TokenlessConfig::config_path();
        // Backup existing config if present
        let backup = std::fs::read_to_string(&path).ok();
        let _ = std::fs::write(&path, "not json");
        // Clear env override
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
        let config = TokenlessConfig::load();
        assert!(config.is_stats_enabled());
        // Restore backup or clean up
        if let Some(content) = backup {
            let _ = std::fs::write(&path, content);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn test_env_override_enabled() {
        std::env::set_var("TOKENLESS_STATS_ENABLED", "1");
        let config = TokenlessConfig::load();
        assert!(config.is_stats_enabled());
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
    }

    #[test]
    fn test_env_override_disabled() {
        std::env::set_var("TOKENLESS_STATS_ENABLED", "0");
        let config = TokenlessConfig::load();
        assert!(!config.is_stats_enabled());
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
    }

    #[test]
    fn test_env_override_true_string() {
        std::env::set_var("TOKENLESS_STATS_ENABLED", "true");
        let config = TokenlessConfig::load();
        assert!(config.is_stats_enabled());
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
    }

    #[test]
    fn test_env_override_overrides_file() {
        let path = TokenlessConfig::config_path();
        // Write file config with stats_enabled=false
        let backup = std::fs::read_to_string(&path).ok();
        let _ = std::fs::write(&path, "{\"stats_enabled\":false}");
        // Set env override to enable
        std::env::set_var("TOKENLESS_STATS_ENABLED", "1");
        let config = TokenlessConfig::load();
        assert!(config.is_stats_enabled());
        // Clean up
        std::env::remove_var("TOKENLESS_STATS_ENABLED");
        if let Some(content) = backup {
            let _ = std::fs::write(&path, content);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}
