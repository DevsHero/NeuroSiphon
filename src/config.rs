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
    /// Vector search defaults when using `--query`.
    pub vector_search: VectorSearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VectorSearchConfig {
    /// HuggingFace model repo ID used by Model2Vec-RS.
    pub model: String,
    /// Number of lines per chunk when building the vector index.
    pub chunk_lines: usize,
    /// Default max number of unique file paths to return for vector search.
    /// (If CLI `--query-limit` is provided, it wins. If omitted, we may auto-tune.)
    pub default_query_limit: usize,
}

impl Default for VectorSearchConfig {
    fn default() -> Self {
        Self {
            model: "minishlab/potion-base-8M".to_string(),
            chunk_lines: 40,
            default_query_limit: 30,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(".context-slicer"),
            token_estimator: TokenEstimatorConfig::default(),
            skeleton_mode: true,
            vector_search: VectorSearchConfig::default(),
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
