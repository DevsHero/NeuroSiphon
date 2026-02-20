use anyhow::{Context, Result};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::WalkBuilder;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::ABSOLUTE_MAX_FILE_BYTES;

fn repomix_default_overrides(repo_root: &Path, exclude_dir_names: &[String]) -> Result<Override> {
    let mut ob = OverrideBuilder::new(repo_root);

    // Repomix-style optimization list (common high-noise artifacts).
    // Note: For directories, include patterns for both the directory entry and its descendants,
    // otherwise walkers may still descend into the directory.

    // NOTE: Override globs behave like ripgrep's `--glob` rules:
    // - If you add any *include* glob (no leading '!'), the walker becomes whitelisted.
    // - Globs with a leading '!' are *excludes*.
    // We want a normal walk (include everything) with a strong default exclude list.

    // Lockfiles
    ob.add("!**/*.lock")?;
    ob.add("!**/package-lock.json")?;
    ob.add("!**/pnpm-lock.yaml")?;
    ob.add("!**/yarn.lock")?;
    ob.add("!**/Cargo.lock")?;

    // Sourcemaps + images/icons
    ob.add("!**/*.map")?;
    ob.add("!**/*.svg")?;
    ob.add("!**/*.png")?;
    ob.add("!**/*.ico")?;
    ob.add("!**/*.jpg")?;
    ob.add("!**/*.jpeg")?;
    ob.add("!**/*.gif")?;

    // Common junk file types (binaries, generated, etc.)
    ob.add("!**/*.pyc")?;
    ob.add("!**/*.pyo")?;
    ob.add("!**/*.pyd")?;
    ob.add("!**/*.class")?;
    ob.add("!**/*.o")?;
    ob.add("!**/*.a")?;
    ob.add("!**/*.so")?;
    ob.add("!**/*.dylib")?;
    ob.add("!**/*.dll")?;
    ob.add("!**/*.exe")?;
    ob.add("!**/*.wasm")?;
    ob.add("!**/*.min.js")?;
    ob.add("!**/*.min.css")?;

    // Common build outputs / heavy dirs (multi-language)
    for d in [
        // VCS
        ".git",
        // JS/TS
        "node_modules",
        "dist",
        "build",
        "coverage",
        ".next",
        ".nuxt",
        ".vscode-test",
        ".vscode",
        "out",
        ".cortexast",
        ".turbo",
        ".svelte-kit",
        // Rust
        "target",
        // Python
        "__pycache__",
        ".venv",
        "venv",
        ".env",
        "env",
        ".tox",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        "htmlcov",
        ".hypothesis",
        "site-packages",
        // Dart / Flutter
        ".dart_tool",
        ".pub",
        ".pub-cache",
        ".flutter-plugins",
        ".flutter-plugins-dependencies",
        // Go
        "vendor",
        // Ruby
        ".bundle",
        // Java / JVM
        ".gradle",
        ".m2",
        // Misc
        ".cortexast",
        ".terraform",
        ".serverless",
        "tmp",
        "temp",
        "logs",
        ".cache",
    ] {
        ob.add(&format!("!**/{d}"))?;
        ob.add(&format!("!**/{d}/**"))?;
    }

    // Project-specific excluded dirs
    for d in exclude_dir_names {
        let d = d.trim().trim_matches('/');
        if d.is_empty() {
            continue;
        }
        ob.add(&format!("!**/{d}"))?;
        ob.add(&format!("!**/{d}/**"))?;
    }

    Ok(ob.build()?)
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub abs_path: PathBuf,
    pub rel_path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub repo_root: PathBuf,
    pub target: PathBuf,
    pub max_file_bytes: u64,
    pub exclude_dir_names: Vec<String>,
}

impl ScanOptions {
    pub fn target_root(&self) -> PathBuf {
        if self.target.is_absolute() {
            self.target.clone()
        } else {
            self.repo_root.join(&self.target)
        }
    }
}

pub fn scan_workspace(opts: &ScanOptions) -> Result<Vec<FileEntry>> {
    let target_root = opts.target_root();

    let meta = std::fs::metadata(&target_root)
        .with_context(|| format!("Target does not exist: {}", target_root.display()))?;

    if meta.is_file() {
        return scan_single_file(&opts.repo_root, &target_root, opts.max_file_bytes)
            .map(|v| v.into_iter().collect());
    }

    let mut entries = Vec::new();
    let overrides = repomix_default_overrides(&opts.repo_root, &opts.exclude_dir_names)?;

    // Hard exclude by directory component name. This is intentionally redundant with overrides,
    // because overrides alone are easy to misconfigure and we must never descend into heavy dirs
    // like `.git/` or `target/`.
    let mut excluded_dir_names: HashSet<String> = HashSet::new();
    for d in &opts.exclude_dir_names {
        let d = d.trim().trim_matches('/');
        if !d.is_empty() {
            excluded_dir_names.insert(d.to_string());
        }
    }

    let walker = WalkBuilder::new(&target_root)
        .standard_filters(true) // .gitignore, .ignore, hidden, etc.
        .overrides(overrides)
        .filter_entry(move |dent| {
            // Skip excluded directories by name (prevents descending).
            if dent.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                if let Some(name) = dent.path().file_name().and_then(|s| s.to_str()) {
                    if excluded_dir_names.contains(name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    for item in walker {
        let dent = match item {
            Ok(d) => d,
            Err(_) => continue,
        };

        if !dent.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        let abs_path = dent.into_path();

        let bytes = match std::fs::metadata(&abs_path).map(|m| m.len()) {
            Ok(b) => b,
            Err(_) => continue,
        };

        // Hard absolute cap — always skip before any config override can raise it.
        if bytes > ABSOLUTE_MAX_FILE_BYTES {
            crate::debug_log!(
                "[cortexast] skipping large file ({}): {}",
                humanize_bytes(bytes),
                abs_path.display()
            );
            continue;
        }

        if bytes == 0 || bytes > opts.max_file_bytes {
            continue;
        }

        let rel_path = path_relative_to(&abs_path, &opts.repo_root)
            .with_context(|| format!("Failed to relativize path: {}", abs_path.display()))?;

        entries.push(FileEntry {
            abs_path,
            rel_path,
            bytes,
        });
    }

    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(entries)
}

#[cfg(debug_assertions)]
fn humanize_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn scan_single_file(
    repo_root: &Path,
    abs_path: &Path,
    max_file_bytes: u64,
) -> Result<Vec<FileEntry>> {
    // Apply the same default overrides for consistency.
    let ov = repomix_default_overrides(repo_root, &[])?;

    let rel_path = path_relative_to(abs_path, repo_root)?;
    if ov.matched(&rel_path, /* is_dir */ false).is_ignore() {
        return Ok(vec![]);
    }

    let bytes = std::fs::metadata(abs_path)?.len();
    if bytes > ABSOLUTE_MAX_FILE_BYTES {
        crate::debug_log!(
            "[cortexast] skipping large file ({}): {}",
            humanize_bytes(bytes),
            abs_path.display()
        );
        return Ok(vec![]);
    }
    if bytes == 0 || bytes > max_file_bytes {
        return Ok(vec![]);
    }

    Ok(vec![FileEntry {
        abs_path: abs_path.to_path_buf(),
        rel_path,
        bytes,
    }])
}

fn path_relative_to(path: &Path, base: &Path) -> Result<PathBuf> {
    let rel = path
        .strip_prefix(base)
        .with_context(|| format!("{} is not under {}", path.display(), base.display()))?;
    Ok(rel.to_path_buf())
}
