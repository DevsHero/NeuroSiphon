use anyhow::Result;
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::config::load_config;
use crate::chronos::{checkpoint_symbol, compare_symbol, list_checkpoints};
use crate::inspector::{extract_symbols_from_source, render_skeleton, read_symbol, find_usages, repo_map_with_filter, call_hierarchy, run_diagnostics};
use crate::slicer::{slice_paths_to_xml, slice_to_xml};
use crate::scanner::{scan_workspace, ScanOptions};
use crate::vector_store::{CodebaseIndex, IndexJob};
use rayon::prelude::*;

#[derive(Default)]
pub struct ServerState {
    repo_root: Option<PathBuf>,
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
                        "name": "map_repo",
                        "description": "🔥 ALWAYS USE THIS INSTEAD OF ls/tree/find. Returns a condensed, bird's-eye map of the entire codebase showing files and public symbols. Run this FIRST every single time you need to understand a repo's structure — it costs almost zero tokens.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "target_dir": { "type": "string", "description": "Directory to map (use '.' for whole repo)" },
                                "search_filter": { "type": "string", "description": "Optional: case-insensitive substring filter (NOT regex). Supports OR via `foo|bar|baz`. Matches file path/filename; for small folders it may also match symbol names." },
                                "max_chars": { "type": "integer", "description": "Optional: max output chars (hard cap 8000). Lower to save tokens." },
                                "ignore_gitignore": { "type": "boolean", "description": "Optional: when true, do not apply .gitignore/.ignore filters (default false). Useful when map_repo returns 0 files." }
                            },
                            "required": ["target_dir"]
                        }
                    },
                    {
                        "name": "read_symbol",
                        "description": "⚡ ALWAYS USE THIS INSTEAD OF cat, head, or any file read. Extracts the exact, full source of a symbol (function, struct, class) via AST. Supports batching via `symbol_names` to fetch multiple symbols from one file in one call.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "path": { "type": "string", "description": "Path to the source file" },
                                "symbol_name": { "type": "string", "description": "Exact name of the symbol (e.g. 'process_request')" },
                                "symbol_names": { "type": "array", "items": { "type": "string" }, "description": "Optional: fetch multiple symbols in one call (e.g. ['A','B','C']). If provided, `symbol_name` is ignored." }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "find_usages",
                        "description": "🎯 ALWAYS USE THIS INSTEAD OF grep/rg/ag/search. Finds 100% accurate semantic usages of any symbol across the entire workspace using AST analysis. Zero false positives from comments or strings. Categorizes hits (Calls vs Type Refs vs Field Inits) to reduce cognitive load.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "target_dir": { "type": "string", "description": "Directory to search in (use '.')" },
                                "symbol_name": { "type": "string", "description": "Symbol name to trace" }
                            },
                            "required": ["target_dir", "symbol_name"]
                        }
                    },
                    {
                        "name": "call_hierarchy",
                        "description": "🕸️ USE BEFORE REFACTORING: Analyzes the Blast Radius. Shows exactly who calls a function (Incoming) and what the function calls (Outgoing). Crucial for preventing breaking changes.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "target_dir": { "type": "string", "description": "Directory to search in (use '.')" },
                                "symbol_name": { "type": "string" }
                            },
                            "required": ["target_dir", "symbol_name"]
                        }
                    },
                    {
                        "name": "run_diagnostics",
                        "description": "🚨 Runs the compiler (e.g., cargo check, tsc) and maps errors directly to exact AST source lines. Use this instantly when code breaks.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" }
                            },
                            "required": ["repoPath"]
                        }
                    },
                    {
                        "name": "get_context_slice",
                        "description": "📦 USE FOR DEEP DIVES: Returns a token-budget-aware XML slice of a directory or file. Skeletonizes all source files (function bodies pruned, imports collapsed). Optionally accepts a semantic `query` to vector-search and return only the most relevant files first. Prefer this over reading raw files when you need multi-file context.",
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
                        "name": "save_checkpoint",
                        "description": "⏳ Save a safe 'save-state' snapshot of a specific AST symbol before you modify it. Use semantic tags like 'baseline' or 'pre-refactor'.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "path": { "type": "string" },
                                "symbol_name": { "type": "string" },
                                "semantic_tag": { "type": "string" }
                            },
                            "required": ["path", "symbol_name", "semantic_tag"]
                        }
                    },
                    {
                        "name": "list_checkpoints",
                        "description": "📋 List all saved Chronos snapshots grouped by semantic tag. Use this to recall what 'save states' are available before comparing.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" }
                            }
                        }
                    },
                    {
                        "name": "compare_checkpoint",
                        "description": "⚖️ CRITICAL: NEVER use `git diff` for comparing your AI refactoring. Use this to compare two Chronos snapshots of a symbol structurally. It ignores whitespace/line-number noise.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "symbol_name": { "type": "string" },
                                "tag_a": { "type": "string" },
                                "tag_b": { "type": "string" },
                                "path": { "type": "string", "description": "Optional: disambiguate if the same tag+symbol exists in multiple files" }
                            },
                            "required": ["symbol_name", "tag_a", "tag_b"]
                        }
                    },
                    {
                        "name": "propagation_checklist",
                        "description": "✅ Generates a cross-language propagation checklist to prevent missed updates (e.g., Proto → Rust → TS). Use this after changing contracts, public APIs, or shared schemas.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string", "description": "Absolute path to the repo root" },
                                "changed_path": { "type": "string", "description": "Path to the changed file (relative to repoPath or absolute)" },
                                "max_symbols": { "type": "integer", "description": "Optional: max extracted symbols to include (default 20)" }
                            },
                            "required": ["changed_path"]
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
            "read_symbol" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let abs = resolve_path(&repo_root, p);

                // Multi-symbol batching: symbol_names: ["A", "B", ...]
                if let Some(arr) = args.get("symbol_names").and_then(|v| v.as_array()) {
                    let mut out_parts: Vec<String> = Vec::new();
                    for v in arr {
                        let Some(sym) = v.as_str().filter(|s| !s.trim().is_empty()) else { continue };
                        match read_symbol(&abs, sym) {
                            Ok(s) => out_parts.push(s),
                            Err(e) => out_parts.push(format!("// ERROR reading `{sym}`: {e}")),
                        }
                    }
                    if out_parts.is_empty() {
                        return err("Missing symbol_names (non-empty array of strings)".to_string());
                    }
                    return ok(out_parts.join("\n\n"));
                }

                let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                    return err("Missing symbol_name or symbol_names".to_string());
                };
                match read_symbol(&abs, sym) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("read_symbol failed: {e}")),
                }
            }
            "save_checkpoint" => {
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
            "list_checkpoints" => {
                let repo_root = self.repo_root_from_params(&args);
                let cfg = load_config(&repo_root);
                match list_checkpoints(&repo_root, &cfg) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("list_checkpoints failed: {e}")),
                }
            }
            "compare_checkpoint" => {
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
            "find_usages" => {
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
            "map_repo" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                    return err("Missing target_dir".to_string());
                };
                let search_filter = args.get("search_filter").and_then(|v| v.as_str()).map(|s| s.trim()).filter(|s| !s.is_empty());
                let max_chars = args.get("max_chars").and_then(|v| v.as_u64()).map(|n| n as usize);
                let ignore_gitignore = args.get("ignore_gitignore").and_then(|v| v.as_bool()).unwrap_or(false);
                let target_dir = resolve_path(&repo_root, target_str);
                match repo_map_with_filter(&target_dir, search_filter, max_chars, ignore_gitignore) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("repo_map failed: {e}")),
                }
            }
            "call_hierarchy" => {
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
            "run_diagnostics" => {
                let repo_root = self.repo_root_from_params(&args);
                match run_diagnostics(&repo_root) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("diagnostics failed: {e}")),
                }
            }
            "propagation_checklist" => {
                let repo_root = self.repo_root_from_params(&args);
                let Some(changed_path) = args.get("changed_path").and_then(|v| v.as_str()) else {
                    return err("Missing changed_path".to_string());
                };
                let abs = resolve_path(&repo_root, changed_path);
                let max_symbols = args.get("max_symbols").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

                let mut out = String::new();
                out.push_str("Propagation checklist\n");
                out.push_str(&format!("Changed: {}\n\n", abs.display()));

                let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if ext == "proto" {
                    let raw = std::fs::read_to_string(&abs);
                    if let Ok(text) = raw {
                        let syms = extract_symbols_from_source(&abs, &text);
                        if !syms.is_empty() {
                            out.push_str("Detected contract symbols (sample):\n");
                            for s in syms.into_iter().take(max_symbols) {
                                out.push_str(&format!("- [{}] {}\n", s.kind, s.name));
                            }
                            out.push('\n');
                        }
                    }

                    out.push_str("Checklist (Proto → generated clients):\n");
                    out.push_str("- Regenerate Rust stubs (prost/tonic build, buf, or your codegen pipeline)\n");
                    out.push_str("- Regenerate TypeScript/JS clients (grpc-web/connect/buf generate, etc.)\n");
                    out.push_str("- Update server handlers for any renamed RPCs/messages/enums\n");
                    out.push_str("- Run `run_diagnostics` and service-level tests\n\n");
                    out.push_str("Suggested CortexAST probes (fast, AST-accurate):\n");
                    out.push_str("- `map_repo` with `search_filter` set to the service/message name\n");
                    out.push_str("- `find_usages` for each renamed message/service to find all consumers\n");
                } else {
                    out.push_str("Checklist (API change propagation):\n");
                    out.push_str("- `find_usages` on the changed symbol(s) to locate all call sites\n");
                    out.push_str("- `call_hierarchy` to understand blast radius before refactoring\n");
                    out.push_str("- Update dependent modules/services and re-run `run_diagnostics`\n");
                }

                ok(out)
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
                    "serverInfo": { "name": "cortexast", "version": "0.2.0" }
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
