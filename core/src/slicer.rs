use crate::config::Config;
use crate::inspector::try_render_skeleton_from_source;
use crate::mapper::build_repo_map_scoped;
use crate::scanner::{scan_workspace, ScanOptions};
use crate::xml_builder::build_context_xml;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SliceMeta {
    pub repo_root: PathBuf,
    pub target: PathBuf,
    pub budget_tokens: usize,
    pub total_tokens: usize,
    pub total_files: usize,
    pub total_bytes: u64,
}

pub fn estimate_tokens_from_bytes(total_bytes: u64, chars_per_token: usize) -> usize {
    if chars_per_token == 0 {
        return total_bytes as usize;
    }

    // Heuristic: ~4 chars per token. We use bytes as a proxy for chars.
    ((total_bytes as f64) / (chars_per_token as f64)).ceil() as usize
}

/// Slice a specific list of repo-relative file paths into context XML.
///
/// Paths are assumed repo-relative with '/' separators.
pub fn slice_paths_to_xml(repo_root: &Path, rel_paths: &[String], budget_tokens: usize, cfg: &Config) -> Result<(String, SliceMeta)> {
    let repo_root = repo_root.to_path_buf();
    let target = PathBuf::from(".");

    // Build entries in the provided order (assumed relevance-ranked).
    let mut entries: Vec<crate::scanner::FileEntry> = Vec::new();
    for rel in rel_paths {
        let rel_norm = rel.replace('\\', "/");
        let abs = repo_root.join(&rel_norm);
        let meta = match std::fs::metadata(&abs) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let bytes = meta.len();
        if bytes == 0 || bytes > cfg.token_estimator.max_file_bytes {
            continue;
        }
        entries.push(crate::scanner::FileEntry {
            abs_path: abs,
            rel_path: PathBuf::from(rel_norm),
            bytes,
        });
    }

    let all_paths: Vec<String> = entries
        .iter()
        .map(|e| e.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    let repository_map_text = build_repository_map_text(&all_paths);

    let mut files_for_xml: Vec<(String, String)> = Vec::new();
    let mut total_bytes: u64 = 64;
    total_bytes = total_bytes
        .saturating_add(estimate_xml_repository_map_overhead_bytes())
        .saturating_add(repository_map_text.len() as u64);

    for e in entries.iter() {
        let bytes = match std::fs::read(&e.abs_path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content_full = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
        let rel = e.rel_path.to_string_lossy().replace('\\', "/");

        let content = if cfg.skeleton_mode {
            match try_render_skeleton_from_source(&e.abs_path, &content_full) {
                Ok(Some(s)) => s,
                Ok(None) => truncate_unknown(&rel, &content_full),
                Err(_) => truncate_unknown(&rel, &content_full),
            }
        } else {
            content_full
        };

        let overhead = estimate_xml_file_overhead_bytes(&rel);
        let new_total = total_bytes
            .saturating_add(overhead)
            .saturating_add(content.len() as u64);
        let est = estimate_tokens_from_bytes(new_total, cfg.token_estimator.chars_per_token);
        if est > budget_tokens {
            continue;
        }

        total_bytes = new_total;
        files_for_xml.push((rel, content));
    }

    let total_tokens = estimate_tokens_from_bytes(total_bytes, cfg.token_estimator.chars_per_token);
    let xml = build_context_xml(Some(&repository_map_text), &files_for_xml)?;

    let meta = SliceMeta {
        repo_root,
        target,
        budget_tokens,
        total_tokens,
        total_files: files_for_xml.len(),
        total_bytes,
    };

    Ok((xml, meta))
}

fn estimate_xml_file_overhead_bytes(rel_path: &str) -> u64 {
    // Rough but consistent overhead estimate for:
    // <file path="{path}"><![CDATA[{content}]]></file>
    // (not counting content length)
    //
    // Constant parts:
    // <file path="  -> 12 bytes
    // ">          -> 2 bytes
    // <![CDATA[    -> 9 bytes
    // ]]></file>   -> 10 bytes
    // Total const  -> 33 bytes
    33u64 + rel_path.len() as u64
}

fn estimate_xml_repository_map_overhead_bytes() -> u64 {
    // <repository_map><![CDATA[...]]></repository_map>
    // Rough constant overhead (not counting map content bytes).
    40
}

fn truncation_header_for_path(rel_path: &str) -> &'static str {
    let p = rel_path.to_lowercase();
    if p.ends_with(".md") || p.ends_with(".txt") || p.ends_with(".toml") || p.ends_with(".yaml") || p.ends_with(".yml") {
        "# TRUNCATED\n"
    } else {
        "/* TRUNCATED */\n"
    }
}

fn truncate_unknown(rel_path: &str, content: &str) -> String {
    let max_lines: usize = 50;
    let max_bytes: usize = 2048;

    // Find a UTF-8 boundary at or before max_bytes.
    let mut cut = content.len().min(max_bytes);
    if cut < content.len() {
        while cut > 0 && !content.is_char_boundary(cut) {
            cut -= 1;
        }
    }
    let head = &content[..cut];

    let out_lines: Vec<&str> = head.lines().take(max_lines).collect();
    // If the original content had fewer than max_lines lines but we cut by bytes, keep it as-is.
    let truncated = cut < content.len() || content.lines().count() > max_lines;
    let mut out = String::new();
    out.push_str(truncation_header_for_path(rel_path));
    out.push_str(&out_lines.join("\n"));
    out.push('\n');
    if truncated {
        out.push_str("\n/* ... */\n");
    }
    out
}

fn is_manifest_file(rel_path: &str) -> bool {
    let p = rel_path.to_lowercase();
    p.ends_with("cargo.toml") || p.ends_with("package.json")
}

fn compact_cargo_toml(content: &str) -> Option<String> {
    let value: toml::Value = content.parse().ok()?;
    let mut out = toml::map::Map::new();

    for k in [
        "package",
        "lib",
        "bin",
        "workspace",
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "features",
    ] {
        if let Some(v) = value.get(k) {
            out.insert(k.to_string(), v.clone());
        }
    }

    toml::to_string_pretty(&toml::Value::Table(out)).ok()
}

fn compact_package_json(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    let mut out = serde_json::Map::new();
    for k in [
        "name",
        "version",
        "private",
        "type",
        "workspaces",
        "main",
        "module",
        "types",
        "exports",
        "scripts",
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(val) = v.get(k) {
            out.insert(k.to_string(), val.clone());
        }
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(out)).ok()
}

fn importance_score(rel_path: &str) -> i64 {
    let p = rel_path.to_lowercase();
    let file = p.rsplit('/').next().unwrap_or(p.as_str());

    // Hard deprioritize tests.
    let mut score: i64 = 0;
    if p.contains("/test/")
        || p.contains("/tests/")
        || file.contains(".spec.")
        || file.contains(".test.")
        || file.contains("_test.")
        || file.starts_with("test_")
    {
        // Task 2: test demotion (only include if lots of leftover budget).
        score -= 1000;
    }

    // Entry points / top-level glue.
    if matches!(
        file,
        "main.rs"
            | "lib.rs"
            | "mod.rs"
            | "build.rs"
            | "index.ts"
            | "index.tsx"
            | "main.ts"
            | "main.tsx"
            | "app.tsx"
            | "cli.ts"
            | "cli.js"
            | "main.go"
            | "main.py"
    ) {
        score += 120;
    }

    // Core source dirs.
    if p.contains("/src/") || p.contains("/core/") {
        score += 30;
    }

    // Manifests are important, but we compact them.
    if is_manifest_file(&p) {
        score += 60;
    }

    // Docs/config (medium).
    if file == "readme.md" || file.ends_with(".md") {
        score += 10;
    }
    if file.ends_with(".toml") || file.ends_with(".yaml") || file.ends_with(".yml") || file.ends_with(".json") {
        score += 5;
    }

    // Deprioritize generated/vendor-ish.
    if p.contains("/dist/") || p.contains("/target/") {
        score -= 30;
    }

    score
}

fn compute_repo_map_indegree(repo_root: &Path, target: &Path) -> HashMap<String, u32> {
    // Build a best-effort file graph using mapper.rs (polyglot import extraction).
    // We only need indegree counts for ranking.
    let scope = if target.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        target.to_path_buf()
    };

    let map = match build_repo_map_scoped(repo_root, &scope) {
        Ok(m) => m,
        Err(_) => return HashMap::new(),
    };

    let mut id_to_path: HashMap<String, String> = HashMap::new();
    for n in map.nodes {
        // mapper emits `path` as a repo-relative path (or best-effort); normalize.
        id_to_path.insert(n.id.clone(), n.path.replace('\\', "/"));
    }

    let mut indegree: HashMap<String, u32> = HashMap::new();
    for e in map.edges {
        if let Some(dst_path) = id_to_path.get(&e.target) {
            *indegree.entry(dst_path.clone()).or_insert(0) += 1;
        }
    }

    indegree
}

fn focus_full_file_rel(repo_root: &Path, target: &Path) -> Option<String> {
    let abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        repo_root.join(target)
    };

    let meta = std::fs::metadata(&abs).ok()?;
    if !meta.is_file() {
        return None;
    }

    let rel = abs.strip_prefix(repo_root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn build_repository_map_text(all_paths: &[String]) -> String {
    // Paths-only, ultra-compressed.
    // Safety caps for huge repos.
    let max_lines: usize = 4000;
    let max_bytes: usize = 64 * 1024;

    let mut out = String::new();
    out.push_str("# REPOSITORY_MAP\n");

    let mut bytes_written: usize = out.len();

    for (lines_written, p) in all_paths.iter().enumerate() {
        if lines_written >= max_lines {
            out.push_str("# ... (truncated)\n");
            break;
        }
        let add = p.len() + 1;
        if bytes_written + add > max_bytes {
            out.push_str("# ... (truncated)\n");
            break;
        }
        out.push_str(p);
        out.push('\n');
        bytes_written += add;
    }

    out
}

pub fn slice_to_xml(repo_root: &Path, target: &Path, budget_tokens: usize, cfg: &Config) -> Result<(String, SliceMeta)> {
    let opts = ScanOptions {
        repo_root: repo_root.to_path_buf(),
        target: target.to_path_buf(),
        max_file_bytes: cfg.token_estimator.max_file_bytes,
        exclude_dir_names: vec![
            ".git".into(),
            "node_modules".into(),
            "dist".into(),
            "target".into(),
            cfg.output_dir.to_string_lossy().to_string(),
        ],
    };

    let mut entries = scan_workspace(&opts)?;

    // Task 1: only the exact target file (if target is a file) is allowed to stay FULL.
    // If target is a directory, everything is treated as context and will be skeletonized/truncated.
    let focus_full_rel = focus_full_file_rel(repo_root, target);

    // Task 3: importance-based sorting.
    // Task 2: Aider-style ranking: score by incoming edges from the repo map.
    let indegree = compute_repo_map_indegree(repo_root, target);
    entries.sort_by(|a, b| {
        let a_rel = a.rel_path.to_string_lossy().replace('\\', "/");
        let b_rel = b.rel_path.to_string_lossy().replace('\\', "/");

        let mut a_score = importance_score(&a_rel);
        let mut b_score = importance_score(&b_rel);

        a_score += *indegree.get(&a_rel).unwrap_or(&0) as i64 * 10;
        b_score += *indegree.get(&b_rel).unwrap_or(&0) as i64 * 10;

        b_score
            .cmp(&a_score)
            .then_with(|| a_rel.cmp(&b_rel))
    });

    // Task 3: repository map header (paths-only) for everything that was eligible after filtering.
    let mut all_paths: Vec<String> = entries
        .iter()
        .map(|e| e.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    all_paths.sort();
    let repository_map_text = build_repository_map_text(&all_paths);

    // Greedy fit by ranked order: include files until budget reached.
    // IMPORTANT: Budget is computed using the *actual emitted content size*.
    let mut files_for_xml: Vec<(String, String)> = Vec::new();
    // Include rough overhead for XML declaration + root element.
    let mut total_bytes: u64 = 64;
    // Include repository map header.
    total_bytes = total_bytes
        .saturating_add(estimate_xml_repository_map_overhead_bytes())
        .saturating_add(repository_map_text.len() as u64);

    for e in entries {
        let bytes = match std::fs::read(&e.abs_path)
            .with_context(|| format!("Failed to read file: {}", e.abs_path.display()))
        {
            Ok(b) => b,
            Err(_) => continue,
        };

        let content_full = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
        let rel = e.rel_path.to_string_lossy().to_string();

        // Task 1 + Task 2: skeleton-first pipeline.
        // - Only the exact focus file (if target is a file) stays FULL.
        // - Manifests are compacted (unless focus file).
        // - Supported source files are skeletonized (unless focus file).
        // - Unknown types are truncated (never full by default).
        let is_focus_full = focus_full_rel.as_ref().is_some_and(|f| f == &rel.replace('\\', "/"));
        let content = if is_focus_full {
            content_full
        } else if rel.to_lowercase().ends_with("cargo.toml") {
            compact_cargo_toml(&content_full).unwrap_or_else(|| content_full.clone())
        } else if rel.to_lowercase().ends_with("package.json") {
            compact_package_json(&content_full).unwrap_or_else(|| content_full.clone())
        } else if cfg.skeleton_mode {
            match try_render_skeleton_from_source(&e.abs_path, &content_full) {
                Ok(Some(s)) => s,
                Ok(None) => truncate_unknown(&rel, &content_full),
                Err(_) => truncate_unknown(&rel, &content_full),
            }
        } else {
            content_full
        };

        let overhead = estimate_xml_file_overhead_bytes(&rel);
        let new_total = total_bytes
            .saturating_add(overhead)
            .saturating_add(content.len() as u64);
        let est = estimate_tokens_from_bytes(new_total, cfg.token_estimator.chars_per_token);
        if est > budget_tokens {
            continue;
        }

        total_bytes = new_total;
        files_for_xml.push((rel, content));
    }

    let total_tokens = estimate_tokens_from_bytes(total_bytes, cfg.token_estimator.chars_per_token);

    let xml = build_context_xml(Some(&repository_map_text), &files_for_xml)?;

    let meta = SliceMeta {
        repo_root: repo_root.to_path_buf(),
        target: target.to_path_buf(),
        budget_tokens,
        total_tokens,
        total_files: files_for_xml.len(),
        total_bytes,
    };

    Ok((xml, meta))
}
