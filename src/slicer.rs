use crate::config::Config;
use crate::inspector::try_render_skeleton_from_source;
use crate::mapper::build_repo_map_scoped;
use crate::scanner::{scan_workspace, FileEntry, ScanOptions};
use crate::workspace::{discover_workspace_members, WorkspaceDiscoveryOptions};
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

    let mut score: i64 = 0;

    // ── Test demotion ────────────────────────────────────────────────────
    if p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/test_")
        || file.contains(".spec.")
        || file.contains(".test.")
        || file.contains("_test.")
        || file.starts_with("test_")
    {
        score -= 1000;
    }

    // ── Entry points / glue ──────────────────────────────────────────────
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
            | "app.ts"
            | "cli.ts"
            | "cli.js"
            | "main.go"
            | "main.py"
            | "__init__.py"
    ) {
        score += 120;
    }

    // ── Microservice / API entry points ──────────────────────────────────
    if matches!(
        file,
        "server.rs"
            | "handler.rs"
            | "handlers.rs"
            | "routes.rs"
            | "router.rs"
            | "api.rs"
            | "controller.rs"
            | "service.rs"
            | "app.rs"
            | "server.ts"
            | "handler.ts"
            | "routes.ts"
            | "router.ts"
            | "api.ts"
            | "controller.ts"
            | "service.ts"
            | "server.py"
            | "app.py"
    ) {
        score += 90;
    }

    // ── Core source dirs ─────────────────────────────────────────────────
    // Boost /src/ at any nesting depth (handles services/foo/src/, apps/bar/src/, etc.)
    let src_depth_bonus = p.matches("/src/").count() as i64;
    score += src_depth_bonus * 30;

    if p.contains("/core/") || p.contains("/lib/") || p.contains("/common/") || p.contains("/shared/") {
        score += 25;
    }

    // ── Manifests (compacted, but structurally important) ─────────────────
    if is_manifest_file(&p) {
        score += 60;
    }

    // ── Docs / config ─────────────────────────────────────────────────────
    if file == "readme.md" || file.ends_with(".md") {
        score += 10;
    }
    if file.ends_with(".toml") || file.ends_with(".yaml") || file.ends_with(".yml") || file.ends_with(".json") {
        score += 5;
    }

    // ── Deprioritise deep data / generated dirs ───────────────────────────
    if p.contains("/dist/") || p.contains("/target/") || p.contains("/generated/") || p.contains("/migrations/") {
        score -= 30;
    }

    // ── De-prioritise overly deep paths (heuristic: >6 slashes is probably data) ──
    let depth = p.chars().filter(|&c| c == '/').count() as i64;
    if depth > 6 {
        score -= (depth - 6) * 5;
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

/// Variant that accepts a pre-formatted multi-section string (used by huge-codebase mode).
fn build_repository_map_text_raw(sections_text: &str) -> String {
    let max_bytes: usize = 96 * 1024; // slightly larger limit for monorepos

    let mut out = String::from("# REPOSITORY_MAP\n");
    let to_add = &sections_text[..sections_text.len().min(max_bytes)];
    out.push_str(to_add);
    if sections_text.len() > max_bytes {
        out.push_str("\n# ... (truncated)\n");
    }
    out
}

/// Shared inner function: convert a ranked list of `FileEntry` into context XML.
fn build_xml_from_entries(
    entries: Vec<crate::scanner::FileEntry>,
    repo_root: &Path,
    target: &Path,
    budget_tokens: usize,
    cfg: &Config,
    focus_full_rel: Option<String>,
) -> Result<(String, SliceMeta)> {
    let mut all_paths: Vec<String> = entries
        .iter()
        .map(|e| e.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    all_paths.sort();
    let repository_map_text = build_repository_map_text(&all_paths);

    let mut files_for_xml: Vec<(String, String)> = Vec::new();
    let mut total_bytes: u64 = 64;
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

        let content_full = String::from_utf8(bytes)
            .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).to_string());
        let rel = e.rel_path.to_string_lossy().to_string();

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

pub fn slice_to_xml(repo_root: &Path, target: &Path, budget_tokens: usize, cfg: &Config) -> Result<(String, SliceMeta)> {
    // ── Huge-codebase auto-detection ──────────────────────────────────────
    // Perform a cheap pre-scan to count files if needed for auto-detection.
    let use_huge = cfg.huge_codebase.enabled || {
        // Quick estimate: count manifest files as a proxy for workspace size.
        let is_large_workspace = is_large_workspace(repo_root);
        is_large_workspace
    };

    if use_huge && target == Path::new(".") {
        return slice_to_xml_huge(repo_root, budget_tokens, cfg);
    }

    let opts = build_scan_options(repo_root, target, cfg);

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

    build_xml_from_entries(entries, repo_root, target, budget_tokens, cfg, focus_full_rel)
}

/// Estimate whether this is a "large workspace" by counting top-level manifests
/// or workspace member indicators without doing a full walk.
fn is_large_workspace(root: &Path) -> bool {
    // A workspace Cargo.toml with many `members` entries = large.
    let cargo_toml = root.join("Cargo.toml");
    if cargo_toml.exists() {
        if let Ok(text) = std::fs::read_to_string(&cargo_toml) {
            let member_count = text.matches('"').count() / 2; // rough: each quoted path = one member
            if member_count >= 5 {
                return true;
            }
        }
    }

    // A package.json with many workspaces entries.
    let pkg_json = root.join("package.json");
    if pkg_json.exists() {
        if let Ok(v) = std::fs::read_to_string(&pkg_json).map(|t| serde_json::from_str::<serde_json::Value>(&t)) {
            if let Ok(v) = v {
                let has_workspaces = v.get("workspaces").is_some();
                if has_workspaces {
                    return true;
                }
            }
        }
    }

    false
}

/// Build `ScanOptions` for a given repo root and target.
/// Properly handles the case where `target` is a Rust `target/` *inside* a service
/// by not over-excluding by name, but instead always excluding the root-level `target/`.
fn build_scan_options(repo_root: &Path, target: &Path, cfg: &Config) -> ScanOptions {
    let mut exclude_dirs = vec![
        ".git".into(),
        "node_modules".into(),
        cfg.output_dir.to_string_lossy().to_string(),
    ];

    // Only exclude `target` and `dist` as top-level build dirs, not when they are
    // a user's *intended scanning target*.
    let target_abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        repo_root.join(target)
    };

    let target_name = target_abs.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if target_name != "target" {
        exclude_dirs.push("target".into());
    }
    if target_name != "dist" {
        exclude_dirs.push("dist".into());
    }

    ScanOptions {
        repo_root: repo_root.to_path_buf(),
        target: target.to_path_buf(),
        max_file_bytes: cfg.token_estimator.max_file_bytes,
        exclude_dir_names: exclude_dirs,
    }
}

/// Huge-codebase mode: discover all workspace members, distribute the token budget
/// across them proportionally, slice each one, then merge into a single XML.
///
/// This guarantees that *every* service gets at least a skeleton of its entry points
/// rather than deeper services being completely crowded out by top-level files.
pub fn slice_to_xml_huge(repo_root: &Path, budget_tokens: usize, cfg: &Config) -> Result<(String, SliceMeta)> {
    let discovery_opts = WorkspaceDiscoveryOptions {
        max_depth: cfg.huge_codebase.member_scan_depth,
        include_patterns: cfg.huge_codebase.include_members.clone(),
        exclude_patterns: cfg.huge_codebase.exclude_members.clone(),
    };

    let members = discover_workspace_members(repo_root, &discovery_opts)?;

    if members.is_empty() {
        // No sub-projects found; fall back to plain slice.
        let opts = build_scan_options(repo_root, Path::new("."), cfg);
        let entries = scan_workspace(&opts)?;
        return build_xml_from_entries(
            entries,
            repo_root,
            Path::new("."),
            budget_tokens,
            cfg,
            None,
        );
    }

    // Budget per member: divide equally, but floor at min_member_budget.
    let member_count = members.len().max(1);
    let per_member_budget = (budget_tokens / member_count)
        .max(cfg.huge_codebase.min_member_budget)
        .min(budget_tokens);

    // Also include a root-level slice (top-level manifests, READMEs, workspace config).
    // This gets 10% of the total budget or 2000 tokens, whichever is smaller.
    let root_budget = (budget_tokens / 10).min(2_000).max(500);

    let mut all_files: Vec<(String, String)> = Vec::new();
    let mut repo_map_sections: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 64;

    // ── Root-level context (workspace manifest + README) ─────────────────
    {
        let root_opts = ScanOptions {
            repo_root: repo_root.to_path_buf(),
            target: PathBuf::from("."),
            max_file_bytes: cfg.token_estimator.max_file_bytes,
            exclude_dir_names: vec![
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "dist".into(),
                cfg.output_dir.to_string_lossy().to_string(),
                // Exclude any sub-directories that are workspace members — avoid duplication.
                // We include at most the top-level files, not the entire sub-dirs.
            ],
        };

        // Scan but only take files directly at root (depth == 0 components beyond root).
        if let Ok(root_entries) = scan_workspace(&root_opts) {
            let root_only: Vec<FileEntry> = root_entries
                .into_iter()
                .filter(|e| {
                    let rel = e.rel_path.to_string_lossy();
                    // Take only root-level files (no '/' in path means directly in root dir).
                    !rel.contains('/')
                })
                .collect();

            let root_section = format!("# ROOT (workspace root)\n");
            repo_map_sections.push(root_section);

            let mut root_used: u64 = 0;
            for e in root_only {
                if let Ok(bytes) = std::fs::read(&e.abs_path) {
                    let content_full = String::from_utf8(bytes)
                        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).to_string());
                    let rel = e.rel_path.to_string_lossy().replace('\\', "/");

                    let content = if rel.to_lowercase().ends_with("cargo.toml") {
                        compact_cargo_toml(&content_full).unwrap_or(content_full)
                    } else if rel.to_lowercase().ends_with("package.json") {
                        compact_package_json(&content_full).unwrap_or(content_full)
                    } else {
                        truncate_unknown(&rel, &content_full)
                    };

                    let overhead = estimate_xml_file_overhead_bytes(&rel);
                    let added = overhead + content.len() as u64;
                    if root_used + added > root_budget as u64 * 4 {
                        break;
                    }
                    root_used += added;
                    total_bytes = total_bytes.saturating_add(added);
                    all_files.push((rel, content));
                }
            }
        }
    }

    // ── Per-member slices ─────────────────────────────────────────────────
    for member in &members {
        let member_opts = build_scan_options(repo_root, Path::new(&member.rel_path), cfg);
        let mut entries = match scan_workspace(&member_opts) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entries.is_empty() {
            continue;
        }

        // Sort by importance within this member.
        let indegree = compute_repo_map_indegree(repo_root, Path::new(&member.rel_path));
        entries.sort_by(|a, b| {
            let a_rel = a.rel_path.to_string_lossy().replace('\\', "/");
            let b_rel = b.rel_path.to_string_lossy().replace('\\', "/");
            let mut a_s = importance_score(&a_rel);
            let mut b_s = importance_score(&b_rel);
            a_s += *indegree.get(&a_rel).unwrap_or(&0) as i64 * 10;
            b_s += *indegree.get(&b_rel).unwrap_or(&0) as i64 * 10;
            b_s.cmp(&a_s).then_with(|| a_rel.cmp(&b_rel))
        });

        let section_header = format!("# {} ({})\n", member.name, member.rel_path);
        let section_paths: Vec<String> = entries
            .iter()
            .map(|e| e.rel_path.to_string_lossy().replace('\\', "/"))
            .collect();
        repo_map_sections.push(format!("{}{}", section_header, section_paths.join("\n")));

        let mut member_bytes: u64 = 0;
        for e in entries {
            let bytes = match std::fs::read(&e.abs_path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let content_full = String::from_utf8(bytes)
                .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).to_string());
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");

            let content = if rel.to_lowercase().ends_with("cargo.toml") {
                compact_cargo_toml(&content_full).unwrap_or(content_full)
            } else if rel.to_lowercase().ends_with("package.json") {
                compact_package_json(&content_full).unwrap_or(content_full)
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
            let added = overhead + content.len() as u64;
            let new_member_est = estimate_tokens_from_bytes(member_bytes + added, cfg.token_estimator.chars_per_token);
            if new_member_est > per_member_budget {
                continue;
            }

            member_bytes = member_bytes.saturating_add(added);
            total_bytes = total_bytes.saturating_add(added);
            all_files.push((rel, content));
        }
    }

    // Build repository map: combine all sections.
    let repo_map_text = {
        let combined = repo_map_sections.join("\n");
        build_repository_map_text_raw(&combined)
    };

    total_bytes = total_bytes
        .saturating_add(estimate_xml_repository_map_overhead_bytes())
        .saturating_add(repo_map_text.len() as u64);

    let total_tokens = estimate_tokens_from_bytes(total_bytes, cfg.token_estimator.chars_per_token);
    let xml = build_context_xml(Some(&repo_map_text), &all_files)?;

    let meta = SliceMeta {
        repo_root: repo_root.to_path_buf(),
        target: PathBuf::from("."),
        budget_tokens,
        total_tokens,
        total_files: all_files.len(),
        total_bytes,
    };

    Ok((xml, meta))
}
