use anyhow::{Context, Result};
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

fn repomix_default_gitignore(repo_root: &Path, exclude_dir_names: &[String]) -> Result<Gitignore> {
    let mut gb = GitignoreBuilder::new(repo_root);

    // Repomix-style optimization list (common high-noise artifacts).
    // Lockfiles
    gb.add_line(None, "**/*.lock")?;
    gb.add_line(None, "**/package-lock.json")?;
    gb.add_line(None, "**/pnpm-lock.yaml")?;
    gb.add_line(None, "**/yarn.lock")?;
    gb.add_line(None, "**/Cargo.lock")?;

    // Sourcemaps + images/icons
    gb.add_line(None, "**/*.map")?;
    gb.add_line(None, "**/*.svg")?;
    gb.add_line(None, "**/*.png")?;
    gb.add_line(None, "**/*.ico")?;
    gb.add_line(None, "**/*.jpg")?;
    gb.add_line(None, "**/*.jpeg")?;
    gb.add_line(None, "**/*.gif")?;

    // Common build outputs
    gb.add_line(None, "**/dist/**")?;
    gb.add_line(None, "**/build/**")?;
    gb.add_line(None, "**/coverage/**")?;
    gb.add_line(None, "**/.next/**")?;
    gb.add_line(None, "**/.nuxt/**")?;
    gb.add_line(None, "**/.vscode-test/**")?;
    gb.add_line(None, "**/.vscode/**")?;
    gb.add_line(None, "**/out/**")?;

    // Project-specific excluded dirs
    for d in exclude_dir_names {
        let d = d.trim();
        if d.is_empty() {
            continue;
        }
        gb.add_line(None, &format!("**/{}/**", d))?;
    }

    Ok(gb.build()?)
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
    let gi = repomix_default_gitignore(&opts.repo_root, &opts.exclude_dir_names)?;
    let walker = WalkBuilder::new(&target_root)
        .standard_filters(true) // .gitignore, .ignore, hidden, etc.
        .filter_entry(move |e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            !gi.matched_path_or_any_parents(e.path(), is_dir).is_ignore()
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
        // Overrides already handle excluded/junk patterns.

        let bytes = match std::fs::metadata(&abs_path).and_then(|m| Ok(m.len())) {
            Ok(b) => b,
            Err(_) => continue,
        };

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

fn scan_single_file(repo_root: &Path, abs_path: &Path, max_file_bytes: u64) -> Result<Vec<FileEntry>> {
    // Apply the same default override patterns for consistency.
    let gi = repomix_default_gitignore(repo_root, &[])?;
    if gi.matched_path_or_any_parents(abs_path, /* is_dir */ false).is_ignore() {
        return Ok(vec![]);
    }

    let bytes = std::fs::metadata(abs_path)?.len();
    if bytes == 0 || bytes > max_file_bytes {
        return Ok(vec![]);
    }

    let rel_path = path_relative_to(abs_path, repo_root)?;
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
