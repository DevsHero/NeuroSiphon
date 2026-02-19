use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenEstimatorConfig {
    pub chars_per_token: usize,
    pub max_file_bytes: u64,
}

/// Controls workspace scanning behavior (what to skip).
///
/// Note: `.gitignore` is always respected by the scanner; these are additional
/// hard skips for noisy monorepo directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanConfig {
    /// Directory *names* to skip anywhere in the tree (e.g. "generated", "tmp").
    ///
    /// These are compared against path components, not full paths.
    pub exclude_dir_names: Vec<String>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            exclude_dir_names: vec![],
        }
    }
}

/// Hard safety ceiling: files larger than this are **always** skipped, regardless of config.
/// This protects low-RAM machines from trying to Tree-sitter-parse a 10 MB minified bundle.
pub const ABSOLUTE_MAX_FILE_BYTES: u64 = 1_000_000; // 1 MB

impl Default for TokenEstimatorConfig {
    fn default() -> Self {
        Self {
            chars_per_token: 4,
            // 512 KB default — enough for any real source file, blocks log/generated bloat.
            max_file_bytes: 512 * 1024,
        }
    }
}

/// Configuration for handling huge monorepo / multi-service workspaces.
///
/// Activated automatically when a workspace has many services, or explicitly with
/// `--huge` CLI flag, or by setting `huge_codebase.enabled = true` in `.cortexast.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HugeCodebaseConfig {
    /// Force huge-codebase mode regardless of auto-detection.
    pub enabled: bool,

    /// Max number of scanned source files before huge-codebase optimisations kick in.
    /// Auto-detection uses this threshold when `enabled` is false.
    pub file_count_threshold: usize,

    /// Max token budget per workspace member / sub-service when splitting context.
    /// Defaults to `budget_tokens / member_count`, floored at this value.
    pub min_member_budget: usize,

    /// Workspace members to include when the user targets the repo root (".").
    /// Supports glob patterns like "services/*", "apps/*", "packages/*".
    /// If empty, all detected workspace members are included.
    pub include_members: Vec<String>,

    /// Workspace members to always exclude (glob patterns).
    pub exclude_members: Vec<String>,

    /// When scanning a sub-service in huge mode, how many directories deep to look
    /// for nested workspace members (0 = only direct children, 2 = double/triple nesting).
    pub member_scan_depth: usize,

    /// Whether to deduplicate shared library code referenced by many services.
    pub dedup_shared_libs: bool,
}

impl Default for HugeCodebaseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            file_count_threshold: 150,
            min_member_budget: 4_000,
            include_members: vec![],
            exclude_members: vec![],
            member_scan_depth: 3,
            dedup_shared_libs: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    /// Settings that govern file discovery and exclusion.
    pub scan: ScanConfig,
    pub token_estimator: TokenEstimatorConfig,
    /// When true, generate "skeleton" file content (function bodies pruned) for supported languages.
    pub skeleton_mode: bool,
    /// Vector search defaults when using `--query`.
    pub vector_search: VectorSearchConfig,
    /// Settings that govern huge monorepo / multi-service workspace behaviour.
    pub huge_codebase: HugeCodebaseConfig,
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
            model: "minishlab/potion-retrieval-32M".to_string(),
            chunk_lines: 40,
            default_query_limit: 30,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(".cortexast"),
            scan: ScanConfig::default(),
            token_estimator: TokenEstimatorConfig::default(),
            skeleton_mode: true,
            vector_search: VectorSearchConfig::default(),
            huge_codebase: HugeCodebaseConfig::default(),
        }
    }
}

pub fn load_config(repo_root: &Path) -> Config {
    let primary = repo_root.join(".cortexast.json");

    let text = std::fs::read_to_string(&primary);
    let Ok(text) = text else { return Config::default() };

    serde_json::from_str::<Config>(&text).unwrap_or_else(|_| Config::default())
}
