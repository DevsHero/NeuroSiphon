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

fn checkpoints_dir(repo_root: &Path, cfg: &Config) -> PathBuf {
    repo_root.join(&cfg.output_dir).join("checkpoints")
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

pub fn checkpoint_symbol(repo_root: &Path, cfg: &Config, path: &str, symbol_name: &str, tag: &str) -> Result<String> {
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

    let abs = resolve_path(repo_root, path);
    let code = read_symbol(&abs, symbol_name)
        .with_context(|| format!("Failed to extract symbol `{symbol_name}` from {}", abs.display()))?;

    let rel_path = abs
        .strip_prefix(repo_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().replace('\\', "/"));

    let rec = CheckpointRecord {
        tag: tag.to_string(),
        path: rel_path.clone(),
        symbol: symbol_name.to_string(),
        code,
        created_unix_ms: now_unix_ms(),
    };

    let dir = checkpoints_dir(repo_root, cfg);
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
    fs::write(&tmp_path, json_text).with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path).with_context(|| format!("Failed to rename checkpoint to {}", final_path.display()))?;

    Ok(format!(
        "Checkpoint saved.\n- tag: `{}`\n- symbol: `{}`\n- path: `{}`\n- file: {}",
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
) -> Result<String> {
    let dir = checkpoints_dir(repo_root, cfg);
    if !dir.exists() {
        return Ok("No checkpoint store found (nothing to delete).".to_string());
    }

    let symbol_name = symbol_name.map(|s| s.trim()).filter(|s| !s.is_empty());
    let semantic_tag = semantic_tag.map(|s| s.trim()).filter(|s| !s.is_empty());

    let path_hint_rel = path_hint
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let abs = resolve_path(repo_root, p);
            abs.strip_prefix(repo_root)
                .map(|pp| pp.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| abs.to_string_lossy().replace('\\', "/"))
        });

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
            if rec.path != *hint {
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
        "Deleted {deleted}/{matched} checkpoint(s) from {}.",
        dir.display()
    );
    if !errors.is_empty() {
        out.push_str("\n\nSome deletes failed:\n");
        out.push_str(&errors.join("\n"));
    }
    Ok(out)
}

pub fn list_checkpoints(repo_root: &Path, cfg: &Config) -> Result<String> {
    let dir = checkpoints_dir(repo_root, cfg);
    if !dir.exists() {
        return Ok("*(no checkpoints yet)*".to_string());
    }

    let all = load_all(&dir);
    if all.is_empty() {
        return Ok("*(no checkpoints yet)*".to_string());
    }

    let mut by_tag: BTreeMap<String, Vec<CheckpointRecord>> = BTreeMap::new();
    for rec in all {
        by_tag.entry(rec.tag.clone()).or_default().push(rec);
    }

    let mut out = String::new();
    out.push_str("## Checkpoints\n\n");
    for (tag, mut recs) in by_tag {
        recs.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
        out.push_str(&format!("### `{}`\n", tag));
        for r in recs.iter().take(50) {
            out.push_str(&format!("- `{}` — `{}`\n", r.symbol, r.path));
        }
        if recs.len() > 50 {
            out.push_str(&format!("- *... {} more*\n", recs.len() - 50));
        }
        out.push('\n');
    }
    Ok(out)
}

fn find_one<'a>(recs: &'a [CheckpointRecord], symbol: &str, tag: &str, path: Option<&str>) -> Result<&'a CheckpointRecord> {
    let mut matches: Vec<&CheckpointRecord> = recs
        .iter()
        .filter(|r| r.symbol == symbol && r.tag == tag)
        .collect();

    if let Some(p) = path {
        matches.retain(|r| r.path == p);
    }

    match matches.len() {
        0 => Err(anyhow!("No checkpoint found for symbol `{symbol}` at tag `{tag}`")),
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
) -> Result<String> {
    let dir = checkpoints_dir(repo_root, cfg);
    let recs = load_all(&dir);
    if recs.is_empty() {
        return Err(anyhow!("No checkpoints found (directory missing or empty): {}", dir.display()));
    }

    let symbol_name = symbol_name.trim();
    let tag_a = tag_a.trim();
    let tag_b = tag_b.trim();
    if symbol_name.is_empty() || tag_a.is_empty() || tag_b.is_empty() {
        return Err(anyhow!("Missing required args: symbol_name, tag_a, tag_b"));
    }

    let rec_a = find_one(&recs, symbol_name, tag_a, path)?;
    let rec_b = find_one(&recs, symbol_name, tag_b, path)?;

    let fence = guess_code_fence(&rec_a.path);
    let mut out = String::new();
    out.push_str(&format!("## Comparison: `{}` (`{}` vs `{}`)\n\n", symbol_name, tag_a, tag_b));

    out.push_str(&format!("### `{}` — `{}`\n", tag_a, rec_a.path));
    out.push_str(&format!("```{}\n{}\n```\n\n", fence, rec_a.code.trim_end()));

    out.push_str(&format!("### `{}` — `{}`\n", tag_b, rec_b.path));
    out.push_str(&format!("```{}\n{}\n```\n", fence, rec_b.code.trim_end()));

    Ok(out)
}
