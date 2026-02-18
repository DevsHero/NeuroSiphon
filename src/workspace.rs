//! Workspace member discovery for monorepos and multi-service repositories.
//!
//! Handles:
//!  - Cargo workspace members (Cargo.toml `[workspace] members = [...]`)
//!  - npm/pnpm/yarn workspaces (package.json `"workspaces": [...]`)
//!  - Auto-detected sub-projects (any sub-dir that contains its own manifest)
//!  - Double / triple nested microservices (e.g. `services/foo/bar/Cargo.toml`)
//!
//! The output is a flat list of `WorkspaceMember` descriptors, each representing one
//! logical service / package that can be independently sliced.

use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    /// Display name of the member (crate name, npm package name, dir name).
    pub name: String,
    /// Path relative to the workspace root.
    pub rel_path: String,
    /// Absolute path to the member directory.
    pub abs_path: PathBuf,
    /// Primary manifest file type detected.
    pub manifest_kind: ManifestKind,
    /// Nesting depth (0 = direct child of root, 1 = one level down, etc.).
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestKind {
    Cargo,
    Npm,
    Python,
    Go,
    Unknown,
}

impl std::fmt::Display for ManifestKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestKind::Cargo => write!(f, "cargo"),
            ManifestKind::Npm => write!(f, "npm"),
            ManifestKind::Python => write!(f, "python"),
            ManifestKind::Go => write!(f, "go"),
            ManifestKind::Unknown => write!(f, "unknown"),
        }
    }
}

/// Parse the `[workspace] members` list from a root `Cargo.toml`.
/// Returns glob-or-literal relative paths.
fn parse_cargo_workspace_members(cargo_toml: &Path) -> Vec<String> {
    let text = match std::fs::read_to_string(cargo_toml) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let val: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    val.get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse npm/yarn/pnpm `workspaces` field from `package.json`.
/// Supports both array form and `{"workspaces": {"packages": [...]}}`.
fn parse_npm_workspace_members(package_json: &Path) -> Vec<String> {
    let text = match std::fs::read_to_string(package_json) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let workspaces = v.get("workspaces");
    let arr = match workspaces {
        Some(serde_json::Value::Array(a)) => a.clone(),
        Some(serde_json::Value::Object(o)) => o
            .get("packages")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => return vec![],
    };

    arr.iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}

/// Resolve glob patterns (like `"services/*"`) relative to `root`, returning
/// all matching subdirectory paths that also contain a manifest file.
fn resolve_workspace_globs(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut found: HashSet<PathBuf> = HashSet::new();
    let manifest_names = ["Cargo.toml", "package.json", "pyproject.toml", "setup.py", "go.mod"];

    for pat in patterns {
        // Normalise: remove trailing "/**" or "/*" often seen in workspace configs.
        let clean = pat.trim_end_matches("/**").trim_end_matches("/*");
        let abs_pattern = root.join(clean).to_string_lossy().to_string();

        // Try glob expansion.
        if let Ok(paths) = glob::glob(&abs_pattern) {
            for entry in paths.flatten() {
                if entry.is_dir() {
                    // Check that at least one manifest exists inside.
                    let has_manifest = manifest_names.iter().any(|m| entry.join(m).exists());
                    if has_manifest {
                        found.insert(entry);
                    }
                } else {
                    // It's a manifest file itself (e.g., explicit `services/foo/Cargo.toml`).
                    if let Some(parent) = entry.parent() {
                        found.insert(parent.to_path_buf());
                    }
                }
            }
        } else {
            // Not a glob — treat as literal path.
            let abs = root.join(clean);
            if abs.is_dir() {
                found.insert(abs);
            }
        }
    }

    let mut v: Vec<PathBuf> = found.into_iter().collect();
    v.sort();
    v
}

/// Extract a human-readable name for a member directory.
fn member_name(abs_path: &Path) -> String {
    // Try Cargo.toml [package] name.
    let cargo = abs_path.join("Cargo.toml");
    if cargo.exists() {
        if let Ok(text) = std::fs::read_to_string(&cargo) {
            if let Ok(v) = text.parse::<toml::Value>() {
                if let Some(name) = v.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
                    return name.to_string();
                }
            }
        }
    }
    // Try package.json "name".
    let pkg = abs_path.join("package.json");
    if pkg.exists() {
        if let Ok(text) = std::fs::read_to_string(&pkg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                    return name.to_string();
                }
            }
        }
    }
    // Fall back to directory name.
    abs_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn detect_manifest_kind(abs_path: &Path) -> ManifestKind {
    if abs_path.join("Cargo.toml").exists() {
        ManifestKind::Cargo
    } else if abs_path.join("package.json").exists() {
        ManifestKind::Npm
    } else if abs_path.join("pyproject.toml").exists() || abs_path.join("setup.py").exists() {
        ManifestKind::Python
    } else if abs_path.join("go.mod").exists() {
        ManifestKind::Go
    } else {
        ManifestKind::Unknown
    }
}

/// Heavy-dir names that should be skipped when auto-scanning for sub-projects.
/// These match the same list as `scanner.rs` exclusions.
fn is_heavy_dir(name: &str) -> bool {
    matches!(
        name,
        "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".git"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".neurosiphon"
            | ".turbo"
            | ".next"
            | ".nuxt"
            | "coverage"
            | "htmlcov"
            | ".tox"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | "vendor"
            | ".bundle"
            | ".gradle"
            | ".m2"
            | ".pub-cache"
            | "tmp"
            | "temp"
            | "logs"
            | ".cache"
            | "out"
    )
}

/// Recursively scan `dir` for sub-directories that contain a manifest file, up to `max_depth` levels.
/// Already-found paths are recorded in `seen` to prevent duplicates.
fn scan_for_sub_projects(
    root: &Path,
    dir: &Path,
    current_depth: usize,
    max_depth: usize,
    seen: &mut HashSet<PathBuf>,
    results: &mut Vec<PathBuf>,
) {
    if current_depth > max_depth {
        return;
    }

    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') || is_heavy_dir(name) {
            continue;
        }
        if seen.contains(&path) {
            continue;
        }

        let manifest_names = ["Cargo.toml", "package.json", "pyproject.toml", "setup.py", "go.mod"];
        let has_manifest = manifest_names.iter().any(|m| path.join(m).exists());
        if has_manifest {
            // Only add if it's not the root dir itself.
            if path != root {
                seen.insert(path.clone());
                results.push(path.clone());
            }
        }

        // Recurse, whether or not a manifest was found (services may nest deeper).
        scan_for_sub_projects(root, &path, current_depth + 1, max_depth, seen, results);
    }
}

/// Options for workspace member discovery.
#[derive(Debug, Clone)]
pub struct WorkspaceDiscoveryOptions {
    /// How many directory levels below `root` to recurse when auto-scanning.
    /// 0 = only direct children, 3 = handles triple-nested services.
    pub max_depth: usize,
    /// Glob patterns to include (empty = include all).
    pub include_patterns: Vec<String>,
    /// Glob patterns to exclude.
    pub exclude_patterns: Vec<String>,
}

impl Default for WorkspaceDiscoveryOptions {
    fn default() -> Self {
        Self {
            max_depth: 3,
            include_patterns: vec![],
            exclude_patterns: vec![],
        }
    }
}

/// Discover all workspace members under `root`.
///
/// Strategy (in priority order):
/// 1. Parse explicit `[workspace] members` from root `Cargo.toml` or `workspaces` from root
///    `package.json` — these are the authoritative declarations.
/// 2. Auto-scan for any sub-directory containing a manifest file, up to `opts.max_depth`.
///
/// Include/exclude glob patterns are applied relative to `root`.
pub fn discover_workspace_members(root: &Path, opts: &WorkspaceDiscoveryOptions) -> Result<Vec<WorkspaceMember>> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut raw: Vec<PathBuf> = Vec::new();

    // ── Step 1: Explicit manifest-declared members ──────────────────────
    let root_cargo = root.join("Cargo.toml");
    if root_cargo.exists() {
        let globs = parse_cargo_workspace_members(&root_cargo);
        if !globs.is_empty() {
            for p in resolve_workspace_globs(root, &globs) {
                if !seen.contains(&p) {
                    seen.insert(p.clone());
                    raw.push(p);
                }
            }
        }
    }

    let root_pkg = root.join("package.json");
    if root_pkg.exists() {
        let globs = parse_npm_workspace_members(&root_pkg);
        if !globs.is_empty() {
            for p in resolve_workspace_globs(root, &globs) {
                if !seen.contains(&p) {
                    seen.insert(p.clone());
                    raw.push(p);
                }
            }
        }
    }

    // ── Step 2: Auto-scan for any sub-projects not already found ────────
    scan_for_sub_projects(root, root, 0, opts.max_depth, &mut seen, &mut raw);

    // ── Step 3: Build WorkspaceMember descriptors ────────────────────────
    let include_pats: Vec<Pattern> = opts
        .include_patterns
        .iter()
        .filter_map(|p| Pattern::new(p).ok())
        .collect();
    let exclude_pats: Vec<Pattern> = opts
        .exclude_patterns
        .iter()
        .filter_map(|p| Pattern::new(p).ok())
        .collect();

    let mut members: Vec<WorkspaceMember> = Vec::new();

    for abs_path in raw {
        let rel_path = match abs_path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        // Apply include/exclude filters.
        if !include_pats.is_empty() && !include_pats.iter().any(|p| p.matches(&rel_path)) {
            continue;
        }
        if exclude_pats.iter().any(|p| p.matches(&rel_path)) {
            continue;
        }

        let depth = rel_path.chars().filter(|&c| c == '/').count();
        let name = member_name(&abs_path);
        let manifest_kind = detect_manifest_kind(&abs_path);

        members.push(WorkspaceMember {
            name,
            rel_path,
            abs_path,
            manifest_kind,
            depth,
        });
    }

    // Sort by depth first, then alphabetically — shallow services first.
    members.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.rel_path.cmp(&b.rel_path)));

    Ok(members)
}

/// Check whether a given path is (or is inside) any known workspace member.
pub fn find_containing_member<'a>(
    members: &'a [WorkspaceMember],
    target: &Path,
) -> Option<&'a WorkspaceMember> {
    // Find the deepest member that is an ancestor of `target`.
    members
        .iter()
        .filter(|m| target.starts_with(&m.abs_path))
        .max_by_key(|m| m.abs_path.components().count())
}
