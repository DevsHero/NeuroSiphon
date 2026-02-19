use anyhow::{Context, Result};
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::config::load_config;
use crate::chronos::{checkpoint_symbol, compare_symbol, list_checkpoints};
use crate::inspector::{render_skeleton, read_symbol, find_usages, repo_map, call_hierarchy, run_diagnostics};
use crate::mapper::{build_repo_map, build_repo_map_scoped};
use crate::slicer::{slice_paths_to_xml, slice_to_xml};
use crate::scanner::{scan_workspace, ScanOptions};
use crate::vector_store::{CodebaseIndex, IndexJob};
use rayon::prelude::*;

#[derive(Default)]
pub struct ServerState {
    repo_root: Option<PathBuf>,
    cached_repo_map: Option<serde_json::Value>,
}

impl ServerState {
    fn repo_root_from_params(&mut self, params: &serde_json::Value) -> PathBuf {
        let repo_root = params
            .get("repoPath")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| self.repo_root.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        self.repo_root = Some(repo_root.clone());
        repo_root
    }

    fn tool_list(&self, id: serde_json::Value) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {
                        "name": "get_context_slice",
                        "description": "Generate an XML context slice for a target directory/file within a repo. Supports optional semantic vector search via the `query` parameter.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "target": { "type": "string", "description": "Relative path within the repo to slice (file or directory). Use '.' for whole repo." },
                                "budget_tokens": { "type": "integer", "exclusiveMinimum": 0, "description": "Token budget (default 32000)" },
                                "query": { "type": "string", "description": "Optional: semantic search query. When provided, returns only the most relevant files." },
                                "query_limit": { "type": "integer", "description": "Max files to return for query mode (default auto-tuned)" }
                            },
                            "required": ["target"]
                        }
                    },
                    {
                        "name": "get_repo_map",
                        "description": "Return a repository map (nodes + edges) for the repo or a scoped subdirectory",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "scope": { "type": "string", "description": "Optional subdirectory path to scope mapping" }
                            }
                        }
                    },
                    {
                        "name": "read_file_skeleton",
                        "description": "Return a low-token skeleton view of a file (function bodies pruned)",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "path": { "type": "string", "description": "Path to the file (relative to repoPath, or absolute)" }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "read_file_full",
                        "description": "Return the full raw content of a file",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "path": { "type": "string", "description": "Path to the file (relative to repoPath, or absolute)" }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "neurosiphon_read_symbol",
                        "description": "Extract the full, unpruned source code of a specific symbol (function, struct, impl block, class, etc.) from a file. Uses Tree-sitter to locate the exact declaration node — no skeleton pruning. Returns the complete implementation with a header showing the file:line range. Much cheaper than reading the entire file when you only need one symbol.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root (used to resolve relative paths)" },
                                "path": { "type": "string", "description": "Path to the source file (relative to repoPath, or absolute)" },
                                "symbol_name": { "type": "string", "description": "Exact name of the symbol to extract (e.g. 'process_request', 'ConvertRequest', 'MyStruct')" }
                            },
                            "required": ["path", "symbol_name"]
                        }
                    },
                    {
                        "name": "neurosiphon_checkpoint_symbol",
                        "description": "Chronos: save a disk-backed snapshot of a specific symbol using a human-readable semantic tag (e.g. 'baseline', 'pre-refactor'). Stored under .neurosiphon/checkpoints/ by default.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root (used to resolve relative paths)" },
                                "path": { "type": "string", "description": "Path to the source file (relative to repoPath, or absolute)" },
                                "symbol_name": { "type": "string", "description": "Exact name of the symbol to snapshot" },
                                "semantic_tag": { "type": "string", "description": "Human-readable tag for this checkpoint (e.g. 'baseline')" }
                            },
                            "required": ["path", "symbol_name", "semantic_tag"]
                        }
                    },
                    {
                        "name": "neurosiphon_list_checkpoints",
                        "description": "Chronos: list available disk-backed symbol checkpoints (grouped by semantic tag).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" }
                            }
                        }
                    },
                    {
                        "name": "neurosiphon_compare_symbol",
                        "description": "Chronos: compare two saved symbol snapshots by semantic tag. Output is side-by-side Markdown blocks (no unified diff).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "symbol_name": { "type": "string", "description": "Exact symbol name to compare" },
                                "tag_a": { "type": "string", "description": "First tag (e.g. 'baseline')" },
                                "tag_b": { "type": "string", "description": "Second tag (e.g. 'post-error-handling')" },
                                "path": { "type": "string", "description": "Optional: disambiguate if the same tag+symbol exists in multiple files" }
                            },
                            "required": ["symbol_name", "tag_a", "tag_b"]
                        }
                    },
                    {
                        "name": "neurosiphon_find_usages",
                        "description": "Find all semantic usages of a symbol across the workspace using Tree-sitter AST analysis. Excludes false positives from comments and string literals. Returns a dense listing of every call site, type reference, and identifier usage with 2-line context. Works even when the project fails to compile.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "target_dir": { "type": "string", "description": "Directory to search in (relative to repoPath, or absolute). Use '.' for the whole repo." },
                                "symbol_name": { "type": "string", "description": "Symbol name to find usages of (e.g. 'process_request', 'ConvertRequest')" }
                            },
                            "required": ["target_dir", "symbol_name"]
                        }
                    },
                    {
                        "name": "neurosiphon_repo_map",
                        "description": "Return a compact hierarchical text map of the entire codebase showing file paths and their exported/public symbols only. Designed for LLM consumption — gives a God's-eye overview of what exists and where without reading every file. Output is grouped by directory and capped at ~8 000 chars. Much cheaper than get_context_slice for navigation.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "target_dir": { "type": "string", "description": "Directory to map (relative to repoPath, or absolute). Use '.' for the whole repo." }
                            },
                            "required": ["target_dir"]
                        }
                    },
                    {
                        "name": "neurosiphon_call_hierarchy",
                        "description": "Analyse the complete call hierarchy for a named symbol. Returns three sections: (1) Definition — file and line where the symbol is declared. (2) Outgoing calls — identifiers called from within the symbol's body. (3) Incoming calls — all callers of this symbol with enclosing function context. Works without compilation via raw AST analysis.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "target_dir": { "type": "string", "description": "Directory to search in (relative to repoPath, or absolute). Use '.' for the whole repo." },
                                "symbol_name": { "type": "string", "description": "Exact symbol name to analyse (e.g. 'process_request', 'MyStruct')" }
                            },
                            "required": ["target_dir", "symbol_name"]
                        }
                    },
                    {
                        "name": "neurosiphon_diagnostics",
                        "description": "Run the project's native compiler/type-checker and return a structured error report pinned to source locations with inline code context. Auto-detects project type: Cargo.toml → `cargo check --message-format=json`; package.json → `npx tsc --noEmit`. Errors capped at 20, warnings at 10. Each entry includes a 1-line context window.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the project root (must contain Cargo.toml or package.json)" }
                            },
                            "required": ["repoPath"]
                        }
                    }
                ]
            }
        })
    }

    fn tool_call(&mut self, id: serde_json::Value, params: &serde_json::Value) -> serde_json::Value {
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let ok = |text: String| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type":"text","text": text }], "isError": false }
            })
        };

        let err = |msg: String| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type":"text","text": msg }], "isError": true }
            })
        };

        match name {
            "get_context_slice" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(target_str) = args.get("target").and_then(|v| v.as_str()) else {
                    return err("Missing target".to_string());
                };
                let target = PathBuf::from(target_str);
                let budget_tokens = args.get("budget_tokens").and_then(|v| v.as_u64()).unwrap_or(32_000) as usize;
                let cfg = load_config(&repo_root);

                // Optional vector search query.
                if let Some(q) = args.get("query").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                    let query_limit = args.get("query_limit").and_then(|v| v.as_u64()).map(|n| n as usize);
                    match self.run_query_slice(&repo_root, &target, q, query_limit, budget_tokens, &cfg) {
                        Ok(xml) => return ok(xml),
                        Err(e) => return err(format!("query slice failed: {e}")),
                    }
                }

                match slice_to_xml(&repo_root, &target, budget_tokens, &cfg) {
                    Ok((xml, _meta)) => ok(xml),
                    Err(e) => err(format!("slice failed: {e}")),
                }
            }
            "get_repo_map" => {
                let repo_root = self.repo_root_from_params(&args);
                let scope = args.get("scope").and_then(|v| v.as_str()).map(PathBuf::from);

                let map_json = if let Some(scope) = scope {
                    match build_repo_map_scoped(&repo_root, &scope) {
                        Ok(m) => serde_json::to_value(m).ok(),
                        Err(_) => None,
                    }
                } else {
                    match build_repo_map(&repo_root) {
                        Ok(m) => serde_json::to_value(m).ok(),
                        Err(_) => None,
                    }
                };

                let Some(v) = map_json else {
                    return err("Failed to build repo map".to_string());
                };

                self.cached_repo_map = Some(v.clone());
                ok(serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
            }
            "read_file_skeleton" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let abs = resolve_path(&repo_root, p);

                match render_skeleton(&abs) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("skeleton failed: {e}")),
                }
            }
            "read_file_full" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let abs = resolve_path(&repo_root, p);

                match std::fs::read_to_string(&abs).with_context(|| format!("Failed to read {}", abs.display())) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("read failed: {e}")),
                }
            }
            "neurosiphon_read_symbol" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name".to_string());
                };
                let abs = resolve_path(&repo_root, p);
                match read_symbol(&abs, sym) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("read_symbol failed: {e}")),
                }
            }
            "neurosiphon_checkpoint_symbol" => {
                let repo_root = self.repo_root_from_params(&args);
                let cfg = load_config(&repo_root);
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name".to_string());
                };
                let tag = args
                    .get("semantic_tag")
                    .and_then(|v| v.as_str())
                    .or_else(|| args.get("tag").and_then(|v| v.as_str()))
                    .unwrap_or("");
                match checkpoint_symbol(&repo_root, &cfg, p, sym, tag) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("checkpoint_symbol failed: {e}")),
                }
            }
            "neurosiphon_list_checkpoints" => {
                let repo_root = self.repo_root_from_params(&args);
                let cfg = load_config(&repo_root);
                match list_checkpoints(&repo_root, &cfg) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("list_checkpoints failed: {e}")),
                }
            }
            "neurosiphon_compare_symbol" => {
                let repo_root = self.repo_root_from_params(&args);
                let cfg = load_config(&repo_root);
                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name".to_string());
                };
                let Some(tag_a) = args.get("tag_a").and_then(|v| v.as_str()) else {
                    return err("Missing tag_a".to_string());
                };
                let Some(tag_b) = args.get("tag_b").and_then(|v| v.as_str()) else {
                    return err("Missing tag_b".to_string());
                };
                let path = args.get("path").and_then(|v| v.as_str());
                match compare_symbol(&repo_root, &cfg, sym, tag_a, tag_b, path) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("compare_symbol failed: {e}")),
                }
            }
            "neurosiphon_find_usages" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                    return err("Missing target_dir".to_string());
                };
                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name".to_string());
                };
                let target_dir = resolve_path(&repo_root, target_str);
                match find_usages(&target_dir, sym) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("find_usages failed: {e}")),
                }
            }
            "neurosiphon_repo_map" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                    return err("Missing target_dir".to_string());
                };
                let target_dir = resolve_path(&repo_root, target_str);
                match repo_map(&target_dir) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("repo_map failed: {e}")),
                }
            }
            "neurosiphon_call_hierarchy" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                    return err("Missing target_dir".to_string());
                };
                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name".to_string());
                };
                let target_dir = resolve_path(&repo_root, target_str);
                match call_hierarchy(&target_dir, sym) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("call_hierarchy failed: {e}")),
                }
            }
            "neurosiphon_diagnostics" => {
                let repo_root = self.repo_root_from_params(&args);
                match run_diagnostics(&repo_root) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("diagnostics failed: {e}")),
                }
            }
            _ => err(format!("Tool not found: {name}")),
        }
    }

    /// Run vector-search-based slicing (query mode) from the MCP server.
    fn run_query_slice(
        &mut self,
        repo_root: &PathBuf,
        target: &PathBuf,
        query: &str,
        query_limit: Option<usize>,
        budget_tokens: usize,
        cfg: &crate::config::Config,
    ) -> anyhow::Result<String> {
        let mut exclude_dir_names = vec![
            ".git".into(),
            "node_modules".into(),
            "dist".into(),
            "target".into(),
            cfg.output_dir.to_string_lossy().to_string(),
        ];
        exclude_dir_names.extend(cfg.scan.exclude_dir_names.iter().cloned());

        let opts = ScanOptions {
            repo_root: repo_root.clone(),
            target: target.clone(),
            max_file_bytes: cfg.token_estimator.max_file_bytes,
            exclude_dir_names,
        };
        let entries = scan_workspace(&opts)?;

        let db_dir = repo_root.join(&cfg.output_dir).join("db");
        let model_id = cfg.vector_search.model.as_str();
        let chunk_lines = cfg.vector_search.chunk_lines;
        let mut index = CodebaseIndex::open(repo_root, &db_dir, model_id, chunk_lines)?;

        let limit = query_limit.unwrap_or_else(|| {
            let budget_based = (budget_tokens / 1_500).clamp(8, 60);
            budget_based.min(cfg.vector_search.default_query_limit).max(1)
        });
        let max_candidates = (limit * 12).clamp(80, 400);
        let terms: Vec<String> = query.split_whitespace()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s.len() >= 2)
            .collect();

        let mut scored: Vec<(i32, usize)> = entries.iter().enumerate().map(|(i, e)| {
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");
            (score_path(&rel, &terms), i)
        }).collect();
        scored.sort_by(|(sa, ia), (sb, ib)| sb.cmp(sa).then_with(|| entries[*ia].bytes.cmp(&entries[*ib].bytes)));

        let mut to_index: Vec<(String, PathBuf)> = Vec::new();
        for (_score, idx) in scored.iter().take(max_candidates) {
            let e = &entries[*idx];
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");
            if matches!(index.needs_reindex_path(&rel, &e.abs_path), Ok(true)) {
                to_index.push((rel, e.abs_path.clone()));
            }
        }

        let jobs: Vec<IndexJob> = to_index.par_iter().filter_map(|(rel, abs)| {
            let bytes = std::fs::read(abs).ok()?;
            let content = String::from_utf8(bytes)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
            Some(IndexJob { rel_path: rel.clone(), abs_path: abs.clone(), content })
        }).collect();

        let rt = tokio::runtime::Runtime::new()?;
        let q_owned = query.to_string();
        let rel_paths: Vec<String> = rt.block_on(async move {
            let _ = index.index_jobs(&jobs, || {}).await;
            index.search(&q_owned, limit).await.unwrap_or_default()
        });

        let (xml, _meta) = if rel_paths.is_empty() {
            slice_to_xml(repo_root, target, budget_tokens, cfg)?
        } else {
            slice_paths_to_xml(repo_root, &rel_paths, budget_tokens, cfg)?
        };
        Ok(xml)
    }
}

/// Resolve a path parameter: if absolute, use as-is; otherwise join to repo_root.
fn resolve_path(repo_root: &PathBuf, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { repo_root.join(p) }
}

fn score_path(rel_path: &str, terms: &[String]) -> i32 {
    let p = rel_path.to_ascii_lowercase();
    let filename = p.rsplit('/').next().unwrap_or(&p);
    let mut score = 0i32;
    for t in terms {
        if filename.contains(t.as_str()) { score += 30; }
        else if p.contains(t.as_str()) { score += 10; }
    }
    score
}

pub fn run_stdio_server() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let mut state = ServerState::default();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }

        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // JSON-RPC notifications have no "id" field — don't respond.
        let has_id = msg.get("id").is_some();
        if !has_id {
            // Side-effect-only notifications (initialize ack, cancel, log, etc.) — ignore.
            continue;
        }

        let id = msg.get("id").cloned().unwrap_or(json!(null));
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let reply = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": msg.get("params").and_then(|p| p.get("protocolVersion")).cloned().unwrap_or(json!("2024-11-05")),
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": { "name": "neurosiphon-rs", "version": "0.2.0" }
                }
            }),
            "ping" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
            "tools/list" => state.tool_list(id),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                state.tool_call(id, &params)
            }
            // Return empty lists for resources/prompts — we don't implement them.
            "resources/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "resources": [] }
            }),
            "prompts/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "prompts": [] }
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }),
        };

        writeln!(stdout, "{}", reply)?;
        stdout.flush()?;
    }

    Ok(())
}
