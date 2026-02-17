use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenEstimatorConfig {
    pub chars_per_token: usize,
    pub max_file_bytes: u64,
}

impl Default for TokenEstimatorConfig {
    fn default() -> Self {
        Self {
            chars_per_token: 4,
            max_file_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    pub token_estimator: TokenEstimatorConfig,
    /// When true, generate "skeleton" file content (function bodies pruned) for supported languages.
    pub skeleton_mode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(".context-slicer"),
            token_estimator: TokenEstimatorConfig::default(),
            skeleton_mode: true,
        }
    }
}

pub fn load_config(repo_root: &Path) -> Config {
    let path = repo_root.join(".context-slicer.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Config::default();
    };

    serde_json::from_str::<Config>(&text).unwrap_or_else(|_| Config::default())
}
