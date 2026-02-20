use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::inspector::read_symbol;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub tag: String,
    pub path: String,
    pub symbol: String,
    pub code: String,
    pub created_unix_ms: u64,
}

fn checkpoints_dir(repo_root: &Path, cfg: &Config, namespace: &str) -> PathBuf {
    let ns = if namespace.trim().is_empty() {
        "default"
    } else {
        namespace.trim()
    };
    repo_root.join(&cfg.output_dir).join("checkpoints").join(ns)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn sanitize_for_filename(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        } else {
            out.push('-');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

fn resolve_path(repo_root: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        repo_root.join(p)
    }
}

fn normalize_checkpoint_path(repo_root: &Path, abs_path: &Path) -> String {
    // Best-effort canonicalization to reduce mismatch from path variants.
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let abs_path = abs_path
        .canonicalize()
        .unwrap_or_else(|_| abs_path.to_path_buf());

    let rel = abs_path
        .strip_prefix(&repo_root)
        .unwrap_or(abs_path.as_path());
    let mut out = rel.to_string_lossy().replace('\\', "/");
    if out.starts_with("./") {
        out = out.trim_start_matches("./").to_string();
    }
    out
}

fn normalize_checkpoint_path_hint(repo_root: &Path, hint: &str) -> String {
    let abs = resolve_path(repo_root, hint.trim());
    normalize_checkpoint_path(repo_root, &abs)
}

fn normalize_record_path(repo_root: &Path, stored_path: &str) -> String {
    let stored_path = stored_path.trim().replace('\\', "/");
    let pb = PathBuf::from(&stored_path);
    if pb.is_absolute() {
        normalize_checkpoint_path(repo_root, &pb)
    } else {
        stored_path.trim_start_matches("./").to_string()
    }
}

fn guess_code_fence(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "cs" => "csharp",
        "cpp" | "cc" | "cxx" | "hpp" | "h" => "cpp",
        _ => "",
    }
}

pub fn checkpoint_symbol(
    repo_root: &Path,
    cfg: &Config,
    path: &str,
    symbol_name: &str,
    tag: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(anyhow!("Missing semantic tag"));
    }
    let symbol_name = symbol_name.trim();
    if symbol_name.is_empty() {
        return Err(anyhow!("Missing symbol_name"));
    }
    let path = path.trim();
    if path.is_empty() {
        return Err(anyhow!("Missing path"));
    }
    let ns = namespace.unwrap_or("default").trim();
    let ns = if ns.is_empty() { "default" } else { ns };

    let abs = resolve_path(repo_root, path);
    let code = read_symbol(&abs, symbol_name).with_context(|| {
        format!(
            "Failed to extract symbol `{symbol_name}` from {}",
            abs.display()
        )
    })?;

    let rel_path = normalize_checkpoint_path(repo_root, &abs);

    let rec = CheckpointRecord {
        tag: tag.to_string(),
        path: rel_path.clone(),
        symbol: symbol_name.to_string(),
        code,
        created_unix_ms: now_unix_ms(),
    };

    let dir = checkpoints_dir(repo_root, cfg, ns);
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    let fname = format!(
        "{}__{}__{}.json",
        sanitize_for_filename(tag),
        sanitize_for_filename(symbol_name),
        rec.created_unix_ms
    );
    let final_path = dir.join(fname);
    let tmp_path = final_path.with_extension("json.tmp");

    let json_text = serde_json::to_string_pretty(&rec).context("Failed to serialize checkpoint")?;
    fs::write(&tmp_path, json_text)
        .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("Failed to rename checkpoint to {}", final_path.display()))?;

    Ok(format!(
        "Checkpoint saved.\n- namespace: `{}`\n- tag: `{}`\n- symbol: `{}`\n- path: `{}`\n- file: {}",
        ns,
        rec.tag,
        rec.symbol,
        rec.path,
        final_path.display()
    ))
}

fn load_all(dir: &Path) -> Vec<CheckpointRecord> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };

    for ent in entries.flatten() {
        let p = ent.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = match fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let rec = match serde_json::from_str::<CheckpointRecord>(&text) {
            Ok(r) => r,
            Err(_) => continue,
        };
        out.push(rec);
    }

    out.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
    out
}

fn load_all_with_files(dir: &Path) -> Vec<(PathBuf, CheckpointRecord)> {
    let mut out: Vec<(PathBuf, CheckpointRecord)> = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };

    for ent in entries.flatten() {
        let p = ent.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = match fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let rec = match serde_json::from_str::<CheckpointRecord>(&text) {
            Ok(r) => r,
            Err(_) => continue,
        };
        out.push((p, rec));
    }

    out.sort_by(|(_pa, a), (_pb, b)| b.created_unix_ms.cmp(&a.created_unix_ms));
    out
}

pub fn delete_checkpoints(
    repo_root: &Path,
    cfg: &Config,
    symbol_name: Option<&str>,
    semantic_tag: Option<&str>,
    path_hint: Option<&str>,
    namespace: Option<&str>,
) -> Result<String> {
    let ns = namespace.unwrap_or("default").trim();
    let ns = if ns.is_empty() { "default" } else { ns };
    let dir = checkpoints_dir(repo_root, cfg, ns);
    if !dir.exists() {
        return Ok(format!(
            "No checkpoint store found for namespace '{}' (nothing to delete).",
            ns
        ));
    }

    let symbol_name = symbol_name.map(|s| s.trim()).filter(|s| !s.is_empty());
    let semantic_tag = semantic_tag.map(|s| s.trim()).filter(|s| !s.is_empty());
    let path_hint = path_hint.map(|s| s.trim()).filter(|s| !s.is_empty());

    // Namespace-only purge: no filters provided → delete the entire namespace directory.
    if symbol_name.is_none() && semantic_tag.is_none() && path_hint.is_none() {
        let count = fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                    .count()
            })
            .unwrap_or(0);
        fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to remove namespace dir {}", dir.display()))?;
        return Ok(format!(
            "Purged namespace '{}': deleted {} checkpoint file(s) and the directory {}.",
            ns,
            count,
            dir.display()
        ));
    }

    let path_hint_rel = path_hint.map(|p| normalize_checkpoint_path_hint(repo_root, p));

    let mut deleted: usize = 0;
    let mut matched: usize = 0;
    let mut errors: Vec<String> = Vec::new();

    for (file_path, rec) in load_all_with_files(&dir) {
        if let Some(sym) = symbol_name {
            if rec.symbol != sym {
                continue;
            }
        }
        if let Some(tag) = semantic_tag {
            if rec.tag != tag {
                continue;
            }
        }
        if let Some(ref hint) = path_hint_rel {
            if normalize_record_path(repo_root, &rec.path) != *hint {
                continue;
            }
        }

        matched += 1;
        match fs::remove_file(&file_path) {
            Ok(_) => deleted += 1,
            Err(e) => errors.push(format!("- {}: {e}", file_path.display())),
        }
    }

    if matched == 0 {
        let mut filters: Vec<String> = Vec::new();
        if let Some(sym) = symbol_name {
            filters.push(format!("symbol='{sym}'"));
        }
        if let Some(tag) = semantic_tag {
            filters.push(format!("tag='{tag}'"));
        }
        if let Some(h) = path_hint_rel {
            filters.push(format!("path='{h}'"));
        }
        return Ok(format!(
            "No checkpoints matched the provided filters ({}).\nTip: run list_checkpoints to see what exists.",
            if filters.is_empty() { "no filters".to_string() } else { filters.join(", ") }
        ));
    }

    let mut out = format!(
        "Deleted {deleted}/{matched} checkpoint(s) from namespace '{}' ({}).",
        ns,
        dir.display()
    );
    if !errors.is_empty() {
        out.push_str("\n\nSome deletes failed:\n");
        out.push_str(&errors.join("\n"));
    }
    Ok(out)
}

pub fn list_checkpoints(repo_root: &Path, cfg: &Config, namespace: Option<&str>) -> Result<String> {
    // If a specific namespace is requested, list only that one.
    // If namespace is None or empty, list ALL namespaces.
    let parent = repo_root.join(&cfg.output_dir).join("checkpoints");

    let ns_dirs: Vec<(String, PathBuf)> =
        if let Some(ns) = namespace.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let dir = checkpoints_dir(repo_root, cfg, ns);
            if !dir.exists() {
                return Ok(format!("*(no checkpoints in namespace '{ns}' yet)*"));
            }
            vec![(ns.to_string(), dir)]
        } else {
            // Walk all namespace subdirectories under the checkpoints parent.
            let mut dirs: Vec<(String, PathBuf)> = Vec::new();
            if parent.exists() {
                for ent in fs::read_dir(&parent).into_iter().flatten().flatten() {
                    let p = ent.path();
                    if p.is_dir() {
                        let ns_name = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        dirs.push((ns_name, p));
                    }
                }
                dirs.sort_by(|(a, _), (b, _)| a.cmp(b));
                // Backward compat: surface any flat .json checkpoints stored
                // directly inside checkpoints/ (written before namespace
                // sub-directories were introduced).  load_all is non-recursive
                // so it only picks up files in the immediate parent, not the
                // already-listed namespace sub-dirs.
                if !load_all(&parent).is_empty() {
                    dirs.push(("(legacy)".to_string(), parent.clone()));
                }
            }
            if dirs.is_empty() {
                return Ok("*(no checkpoints yet)*".to_string());
            }
            dirs
        };

    let mut out = String::new();
    out.push_str("## Checkpoints\n");

    for (ns_name, dir) in &ns_dirs {
        let all = load_all(dir);
        if all.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### Namespace: `{}`\n\n", ns_name));
        let mut by_tag: BTreeMap<String, Vec<CheckpointRecord>> = BTreeMap::new();
        for rec in all {
            by_tag.entry(rec.tag.clone()).or_default().push(rec);
        }
        for (tag, mut recs) in by_tag {
            recs.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
            out.push_str(&format!("#### `{}`\n", tag));
            for r in recs.iter().take(50) {
                out.push_str(&format!("- `{}` — `{}`\n", r.symbol, r.path));
            }
            if recs.len() > 50 {
                out.push_str(&format!("- *... {} more*\n", recs.len() - 50));
            }
            out.push('\n');
        }
    }

    if out.trim_end() == "## Checkpoints" {
        return Ok("*(no checkpoints yet)*".to_string());
    }
    Ok(out)
}

fn find_one<'a>(
    repo_root: &Path,
    recs: &'a [CheckpointRecord],
    symbol: &str,
    tag: &str,
    path: Option<&str>,
) -> Result<&'a CheckpointRecord> {
    let mut matches: Vec<&CheckpointRecord> = recs
        .iter()
        .filter(|r| r.symbol == symbol && r.tag == tag)
        .collect();

    if let Some(p) = path {
        let hint = normalize_checkpoint_path_hint(repo_root, p);
        matches.retain(|r| normalize_record_path(repo_root, &r.path) == hint);
    }

    match matches.len() {
        0 => Err(anyhow!(
            "No checkpoint found for symbol `{symbol}` at tag `{tag}`"
        )),
        1 => Ok(matches[0]),
        _ => {
            let mut msg = format!("Multiple checkpoints match symbol `{symbol}` at tag `{tag}`. Please disambiguate with `path`.\nMatches:\n");
            for m in matches.iter().take(10) {
                msg.push_str(&format!("- {}\n", m.path));
            }
            Err(anyhow!(msg))
        }
    }
}

pub fn compare_symbol(
    repo_root: &Path,
    cfg: &Config,
    symbol_name: &str,
    tag_a: &str,
    tag_b: &str,
    path: Option<&str>,
    namespace: Option<&str>,
) -> Result<String> {
    let ns = namespace.unwrap_or("default").trim();
    let ns = if ns.is_empty() { "default" } else { ns };
    let dir = checkpoints_dir(repo_root, cfg, ns);
    let recs = load_all(&dir);
    if recs.is_empty() {
        return Err(anyhow!(
            "No checkpoints found (directory missing or empty): {}",
            dir.display()
        ));
    }

    let symbol_name = symbol_name.trim();
    let tag_a = tag_a.trim();
    let tag_b = tag_b.trim();
    if symbol_name.is_empty() || tag_a.is_empty() || tag_b.is_empty() {
        return Err(anyhow!("Missing required args: symbol_name, tag_a, tag_b"));
    }

    let rec_a = find_one(repo_root, &recs, symbol_name, tag_a, path)?;

    // Magic tag: compare against current filesystem state.
    // This avoids requiring a second checkpoint when you just want "before vs now".
    let live_record;
    let rec_b = if tag_b == "__live__" {
        let Some(p) = path.map(|s| s.trim()).filter(|s| !s.is_empty()) else {
            return Err(anyhow!(
                "tag_b='__live__' requires 'path' (the source file containing the symbol)."
            ));
        };
        let abs = resolve_path(repo_root, p);
        let code = read_symbol(&abs, symbol_name).with_context(|| {
            format!(
                "Failed to extract live symbol `{symbol_name}` from {}",
                abs.display()
            )
        })?;

        live_record = CheckpointRecord {
            tag: "__live__".to_string(),
            path: normalize_checkpoint_path(repo_root, &abs),
            symbol: symbol_name.to_string(),
            code,
            created_unix_ms: now_unix_ms(),
        };
        &live_record
    } else {
        find_one(repo_root, &recs, symbol_name, tag_b, path)?
    };

    let fence = guess_code_fence(&rec_a.path);
    let mut out = String::new();
    out.push_str(&format!(
        "## Comparison: `{}` (`{}` vs `{}`)\n\n",
        symbol_name, tag_a, tag_b
    ));

    out.push_str(&format!("### `{}` — `{}`\n", tag_a, rec_a.path));
    out.push_str(&format!("```{}\n{}\n```\n\n", fence, rec_a.code.trim_end()));

    out.push_str(&format!("### `{}` — `{}`\n", tag_b, rec_b.path));
    out.push_str(&format!("```{}\n{}\n```\n", fence, rec_b.code.trim_end()));

    Ok(out)
}
