//! Configuration options for Euler.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Whether hidden files (starting with '.') are visible by default.
    pub show_hidden: bool,

    /// Whether to show the interactive diff confirmation dialog on `:w`.
    pub confirm_diff: bool,

    /// Whether deleted files are moved to system trash (true) or permanently deleted (false).
    pub trash_deletes: bool,

    /// Whether to sort filenames using natural sorting (e.g. "2.txt" before "10.txt").
    pub natural_sort: bool,

    /// Default command for opening files in terminal (e.g. "nvim", "vim", "nano").
    pub default_editor: String,

    /// System opener command for `gx` (e.g. "xdg-open", "open").
    pub opener: String,

    /// Whether to explicitly disable all arrow keys across the entire application.
    pub disable_arrow_keys: bool,
}

impl Default for Config {
    fn default() -> Self {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "nano".to_string());

        Self {
            show_hidden: true,
            confirm_diff: true,
            trash_deletes: true,
            natural_sort: true,
            default_editor: editor,
            opener: "xdg-open".to_string(),
            disable_arrow_keys: true,
        }
    }
}

impl Config {
    /// Load config from `$HOME/.config/euler/config.yaml`, or return default.
    pub fn load_or_default() -> Self {
        if let Some(mut path) = dirs_config_path() {
            path.push("euler");
            path.push("config.yaml");
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(config) = serde_yaml::from_str::<Config>(&content) {
                        return config;
                    }
                }
            }
        }
        Config::default()
    }
}

/// Helper to get the user's config directory.
fn dirs_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config"))
}
