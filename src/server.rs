use anyhow::Result;
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::config::load_config;
use crate::chronos::{checkpoint_symbol, compare_symbol, list_checkpoints};
use crate::inspector::{extract_symbols_from_source, render_skeleton, read_symbol, find_usages, repo_map_with_filter, call_hierarchy, run_diagnostics, propagation_checklist};
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
                        "name": "cortex_code_explorer",
                        "description": "🔍 CODE EXPLORER MEGATOOL — 🔥 ALWAYS USE THIS INSTEAD OF ls, tree, find, or cat for any codebase exploration task. Provides two complementary lenses on a codebase: a fast bird's-eye symbol map or a deep token-budgeted XML slice. DECISION GUIDE → `map_overview`: use when you need to understand repo structure, discover file/symbol names, or orient yourself before starting a task — costs almost zero tokens, run this first on any new codebase. → `deep_slice`: use when you need actual function bodies or multi-file context for a specific edit — auto-skeletonizes files to fit a token budget and optionally vector-ranks files by semantic relevance to a query. Do NOT use `deep_slice` just to list files or symbols; use `map_overview` for that.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "action": {
                                    "type": "string",
                                    "enum": ["map_overview", "deep_slice"],
                                    "description": "Required — chooses the exploration mode.\n• `map_overview` — Returns a condensed bird's-eye map of all files and public symbols in a directory. Requires `target_dir` (use '.' for whole repo). Optional: `search_filter` (case-insensitive substring/OR filter on paths), `max_chars` (hard cap 8000), `ignore_gitignore`. Returns minimal tokens even for large repos — run this first on any unfamiliar codebase.\n• `deep_slice` — Returns a token-budget-aware XML slice of a file or directory with function bodies pruned to skeletons. Requires `target` (relative path to file or dir). Optional: `budget_tokens` (default 32000), `query` (semantic search to rank most-relevant files first), `query_limit` (max files returned in query mode). When `query` is set, only the most relevant files are included — use this to minimize token waste on large directories."
                                },
                                "repoPath": { "type": "string", "description": "Optional absolute path to the repo root (defaults to current working directory)." },

                                "target_dir": { "type": "string", "description": "(map_overview) Directory to map (use '.')" },
                                "search_filter": { "type": "string", "description": "(map_overview) Optional: case-insensitive substring filter (NOT regex). Supports OR via `foo|bar|baz`." },
                                "max_chars": { "type": "integer", "description": "(map_overview) Optional output cap (hard cap 8000)." },
                                "ignore_gitignore": { "type": "boolean", "description": "(map_overview) Optional: include git-ignored files." },

                                "target": { "type": "string", "description": "(deep_slice) Relative path within repo to slice (file or dir). Required for deep_slice." },
                                "budget_tokens": { "type": "integer", "exclusiveMinimum": 0, "description": "(deep_slice) Token budget (default 32000)." },
                                "query": { "type": "string", "description": "(deep_slice) Optional semantic query for vector-ranked slicing." },
                                "query_limit": { "type": "integer", "description": "(deep_slice) Optional max files in query mode." }
                            },
                            "required": ["action"]
                        }
                    },
                    {
                        "name": "cortex_symbol_analyzer",
                        "description": "🎯 SYMBOL ANALYSIS MEGATOOL — 🔥 ALWAYS USE THIS INSTEAD OF grep, rg, ag, or any text/regex search for symbol lookups. Uses tree-sitter AST analysis to deliver 100% accurate results with zero false positives from comments, strings, or name collisions. DECISION GUIDE → `read_source`: extract the exact full source of any function/class/struct from a file — always do this before editing a symbol. → `find_usages`: discover every call site, type reference, and field initialization across the workspace before changing a symbol's signature. → `blast_radius`: use BEFORE any rename, move, or delete to measure all incoming callers and outgoing callees — critical for preventing breaking changes. → `propagation_checklist`: use when modifying a shared type, interface, or proto message to generate an exhaustive checklist of every place that must be updated, grouped by language/domain. Never use grep or rg when this tool is available.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "action": {
                                    "type": "string",
                                    "enum": ["read_source", "find_usages", "blast_radius", "propagation_checklist"],
                                    "description": "Required — selects the analysis operation.\n• `read_source` — Extracts the exact full source of a named symbol (function, struct, class, method, variable) via AST from a specific file. Faster and more accurate than cat + manual scanning; zero regex ambiguity. Supports batch extraction: provide `symbol_names` array to fetch multiple symbols in one call. Requires `path` (source file) and `symbol_name`.\n• `find_usages` — Finds all semantic usages (calls, type references, field initializations) of a symbol across a workspace directory. Categorizes by usage type to reduce cognitive load. Requires `symbol_name` and `target_dir`.\n• `blast_radius` — Analyzes the full call hierarchy: shows who calls this function (incoming) and what the function itself calls (outgoing). Run this BEFORE every rename, move, or delete to understand true blast radius. Requires `symbol_name` and `target_dir`.\n• `propagation_checklist` — Generates a strict Markdown checklist of all places a symbol is used, grouped by language and domain, ensuring no cross-module update is missed. Requires `symbol_name`; use `changed_path` for legacy contract-file (e.g. .proto) mode."
                                },
                                "repoPath": { "type": "string", "description": "Optional absolute path to the repo root." },
                                "symbol_name": { "type": "string", "description": "Target symbol. Avoid regex or plural words (e.g. use 'auth', 'convert_request')." },
                                "target_dir": { "type": "string", "description": "Scope directory (use '.' for entire repo). Required for find_usages/blast_radius; optional for propagation_checklist (defaults '.')." },
                                "ignore_gitignore": { "type": "boolean", "description": "(propagation_checklist) Include generated / git-ignored stubs." },

                                "path": { "type": "string", "description": "(read_source) Source file containing the symbol. Required for read_source." },
                                "symbol_names": { "type": "array", "items": { "type": "string" }, "description": "(read_source) Optional batch mode. If provided, extracts all symbols from `path` and ignores `symbol_name`." },

                                "changed_path": { "type": "string", "description": "(propagation_checklist) Optional legacy mode: path to a changed contract file (e.g. .proto). If provided, overrides symbol-based mode." },
                                "max_symbols": { "type": "integer", "description": "(propagation_checklist legacy) Optional max extracted symbols (default 20)." }
                            },
                            "required": ["action", "symbol_name"]
                        }
                    },
                    {
                        "name": "cortex_chronos",
                        "description": "⏳ CHRONOS SNAPSHOT MEGATOOL — ⚖️ CRITICAL: NEVER use `git diff` to verify AI refactors; it produces whitespace and line-number noise that hides semantic regressions. Chronos saves named structural AST snapshots of individual symbols under human-readable semantic tags, then compares them at the AST level — ignoring all formatting noise. DECISION GUIDE → `save_checkpoint`: call this with a tag like 'pre-refactor' or 'baseline' BEFORE any non-trivial edit — takes milliseconds, creates an unambiguous rollback reference point. → `list_checkpoints`: call this to review all existing tags before choosing names for a new snapshot, avoiding accidental collisions. → `compare_checkpoint`: call this AFTER editing to structurally verify that only the intended changes were made and no silent regressions were introduced. This three-step workflow (save → edit → compare) is mandatory for safe AI-driven code changes.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "action": {
                                    "type": "string",
                                    "enum": ["save_checkpoint", "list_checkpoints", "compare_checkpoint"],
                                    "description": "Required — selects the Chronos operation.\n• `save_checkpoint` — Saves an AST-level snapshot of a named symbol under a semantic tag. Call this BEFORE any non-trivial refactor or edit. Requires `path` (source file path), `symbol_name`, and `semantic_tag` (or `tag` alias — use descriptive values like 'pre-refactor', 'baseline', 'v1-before-split').\n• `list_checkpoints` — Lists all saved snapshots grouped by semantic tag. Call this before a comparison to know which tags exist. Only `repoPath` is relevant (optional).\n• `compare_checkpoint` — Structurally compares two named snapshots of a symbol, ignoring whitespace and line-number differences. Returns an AST-level semantic diff. Call this AFTER editing to verify correctness. Requires `symbol_name`, `tag_a`, `tag_b`; `path` is optional as a disambiguation hint when the same tag+symbol exists in multiple files."
                                },
                                "repoPath": { "type": "string", "description": "Optional absolute path to the repo root." },

                                "path": { "type": "string", "description": "(save_checkpoint/compare_checkpoint) Source file path. Optional for compare (disambiguation)." },
                                "symbol_name": { "type": "string", "description": "(save_checkpoint/compare_checkpoint) Target symbol." },
                                "semantic_tag": { "type": "string", "description": "(save_checkpoint) Semantic tag (e.g. pre-refactor)." },
                                "tag": { "type": "string", "description": "(save_checkpoint) Alias for semantic_tag." },
                                "tag_a": { "type": "string", "description": "(compare_checkpoint) First tag." },
                                "tag_b": { "type": "string", "description": "(compare_checkpoint) Second tag." }
                            },
                            "required": ["action"]
                        }
                    },
                    {
                        "name": "run_diagnostics",
                        "description": "🚨 COMPILE-TIME DIAGNOSTICS — Runs the project's primary compiler (cargo check for Rust, tsc for TypeScript, gcc for C/C++, etc.) and maps every error and warning directly to exact AST source lines. Use this immediately after any code edit to catch compiler errors before proceeding to the next step — never assume an edit compiled successfully without running this. Returns structured output including file path, line number, error code, and message, ready for targeted fixing without additional file reads. Always prefer this over running the compiler manually in a terminal.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" }
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
            // ── Megatools ────────────────────────────────────────────────
            "cortex_code_explorer" => {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
                match action {
                    "map_overview" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'map_overview' requires the 'target_dir' parameter (e.g. '.' for the whole repo). \
                                Please call cortex_code_explorer again with action='map_overview' and target_dir='.'.".to_string()
                            );
                        };
                        let search_filter = args
                            .get("search_filter")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let max_chars = args.get("max_chars").and_then(|v| v.as_u64()).map(|n| n as usize);
                        let ignore_gitignore = args.get("ignore_gitignore").and_then(|v| v.as_bool()).unwrap_or(false);
                        let target_dir = resolve_path(&repo_root, target_str);

                        // Proactive guardrail: agents often hallucinate paths.
                        if !target_dir.exists() {
                            let mut entries: Vec<String> = Vec::new();
                            if let Ok(rd) = std::fs::read_dir(&repo_root) {
                                for e in rd.flatten() {
                                    if let Some(name) = e.file_name().to_str() {
                                        entries.push(name.to_string());
                                    }
                                }
                            }
                            entries.sort();
                            let shown: Vec<String> = entries.into_iter().take(30).collect();
                            return err(format!(
                                "Error: Path '{}' does not exist in repo root '{}'.\n\
Available top-level entries in this repo: [{}].\n\
Please correct your target_dir (or pass repoPath explicitly).",
                                target_str,
                                repo_root.display(),
                                shown
                                    .into_iter()
                                    .map(|s| format!("'{}'", s))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }

                        match repo_map_with_filter(&target_dir, search_filter, max_chars, ignore_gitignore) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("repo_map failed: {e}")),
                        }
                    }
                    "deep_slice" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let Some(target_str) = args.get("target").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'deep_slice' requires the 'target' parameter \
                                (relative path to a file or directory within the repo, e.g. 'src'). \
                                Please call cortex_code_explorer again with action='deep_slice' and target='<path>'.".to_string()
                            );
                        };
                        let target = PathBuf::from(target_str);
                        let budget_tokens = args.get("budget_tokens").and_then(|v| v.as_u64()).unwrap_or(32_000) as usize;
                        let cfg = load_config(&repo_root);

                        // Optional vector search query.
                        if let Some(q) = args.get("query").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                            let query_limit = args.get("query_limit").and_then(|v| v.as_u64()).map(|n| n as usize);
                            match self.run_query_slice(&repo_root, &target, q, query_limit, budget_tokens, &cfg) {
                                Ok(xml) => return ok(inline_or_spill(xml)),
                                Err(e) => return err(format!("query slice failed: {e}")),
                            }
                        }

                        match slice_to_xml(&repo_root, &target, budget_tokens, &cfg) {
                            Ok((xml, _meta)) => ok(inline_or_spill(xml)),
                            Err(e) => err(format!("slice failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_code_explorer: received '{action}'. \
                        Choose one of: 'map_overview' (repo structure map) or 'deep_slice' (token-budgeted content slice). \
                        Example: cortex_code_explorer with action='map_overview' and target_dir='.'"
                    )),
                }
            }
            "cortex_symbol_analyzer" => {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
                match action {
                    "read_source" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'read_source' requires both 'path' (source file containing the symbol) \
                                and 'symbol_name'. You omitted 'path'. \
                                Please call cortex_symbol_analyzer again with action='read_source', path='<file>', and symbol_name='<name>'. \
                                Tip: use cortex_code_explorer(action=map_overview) first if you are unsure of the file path.".to_string()
                            );
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
                                return err(
                                    "Error: action 'read_source' with 'symbol_names' requires a non-empty array of symbol name strings. \
                                    You provided an empty array or all entries were blank. \
                                    Example: symbol_names=['process_request', 'handle_error']".to_string()
                                );
                            }
                            return ok(out_parts.join("\n\n"));
                        }

                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'read_source' requires both 'path' and 'symbol_name'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='read_source', path='<file>', and symbol_name='<name>'. \
                                For batch extraction of multiple symbols from the same file, use symbol_names=['A','B'] instead.".to_string()
                            );
                        };
                        match read_symbol(&abs, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("read_symbol failed: {e}")),
                        }
                    }
                    "find_usages" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'find_usages' requires both 'symbol_name' and 'target_dir'. You omitted 'target_dir'. \
                                Use '.' to search the entire repo. \
                                Please call cortex_symbol_analyzer again with action='find_usages', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'find_usages' requires both 'symbol_name' and 'target_dir'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='find_usages', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let target_dir = resolve_path(&repo_root, target_str);
                        match find_usages(&target_dir, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("find_usages failed: {e}")),
                        }
                    }
                    "blast_radius" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'blast_radius' requires both 'symbol_name' and 'target_dir'. You omitted 'target_dir'. \
                                Use '.' to search the entire repo. \
                                Please call cortex_symbol_analyzer again with action='blast_radius', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'blast_radius' requires both 'symbol_name' and 'target_dir'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='blast_radius', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let target_dir = resolve_path(&repo_root, target_str);
                        match call_hierarchy(&target_dir, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("call_hierarchy failed: {e}")),
                        }
                    }
                    "propagation_checklist" => {
                        let repo_root = self.repo_root_from_params(&args);
                        // Legacy mode: changed_path checklist (if provided).
                        if let Some(changed_path) = args.get("changed_path").and_then(|v| v.as_str()).map(|s| s.trim()).filter(|s| !s.is_empty()) {
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
                                out.push_str("- `cortex_code_explorer` action=map_overview with `search_filter` set to the service/message name\n");
                                out.push_str("- `cortex_symbol_analyzer` action=find_usages for each renamed message/service to find all consumers\n");
                            } else {
                                out.push_str("Checklist (API change propagation):\n");
                                out.push_str("- `cortex_symbol_analyzer` action=find_usages on the changed symbol(s) to locate all call sites\n");
                                out.push_str("- `cortex_symbol_analyzer` action=blast_radius to understand blast radius before refactoring\n");
                                out.push_str("- Update dependent modules/services and re-run `run_diagnostics`\n");
                            }

                            return ok(out);
                        }

                        // New mode: symbol-based cross-boundary checklist.
                        let Some(sym) = args
                            .get("symbol_name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                        else {
                            return err(
                                "Error: action 'propagation_checklist' requires 'symbol_name' (the shared type/struct/interface to trace). \
                                You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='propagation_checklist' and symbol_name='<name>'. \
                                Alternatively, pass 'changed_path' (path to a .proto or contract file) for legacy file-based mode.".to_string()
                            );
                        };
                        let target_str = args.get("target_dir").and_then(|v| v.as_str()).unwrap_or(".");
                        let target_dir = resolve_path(&repo_root, target_str);
                        let ignore_gitignore = args.get("ignore_gitignore").and_then(|v| v.as_bool()).unwrap_or(false);
                        match propagation_checklist(&target_dir, sym, ignore_gitignore) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("propagation_checklist failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_symbol_analyzer: received '{action}'. \
                        Choose one of: 'read_source' (extract symbol AST), 'find_usages' (trace all call sites), \
                        'blast_radius' (call hierarchy before rename/delete), or 'propagation_checklist' (cross-module update checklist). \
                        Example: cortex_symbol_analyzer with action='find_usages', symbol_name='my_fn', and target_dir='.'"
                    )),
                }
            }
            "cortex_chronos" => {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").trim();
                match action {
                    "save_checkpoint" => {
                        let repo_root = self.repo_root_from_params(&args);
                        let cfg = load_config(&repo_root);
                        let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'save_checkpoint' requires 'path' (source file), 'symbol_name', and 'semantic_tag'. \
                                You omitted 'path'. \
                                Please call cortex_chronos again with action='save_checkpoint', path='<file>', \
                                symbol_name='<name>', and semantic_tag='pre-refactor' (or any descriptive tag).".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'save_checkpoint' requires 'path', 'symbol_name', and 'semantic_tag'. \
                                You omitted 'symbol_name'. \
                                Please call cortex_chronos again with action='save_checkpoint', path='<file>', \
                                symbol_name='<name>', and semantic_tag='pre-refactor'.".to_string()
                            );
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
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'symbol_name'. \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'. \
                                Tip: call cortex_chronos(action=list_checkpoints) first to see all available tags.".to_string()
                            );
                        };
                        let Some(tag_a) = args.get("tag_a").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'tag_a' (the 'before' snapshot tag). \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'. \
                                Tip: call cortex_chronos(action=list_checkpoints) to see all available tags.".to_string()
                            );
                        };
                        let Some(tag_b) = args.get("tag_b").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'tag_b' (the 'after' snapshot tag). \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'.".to_string()
                            );
                        };
                        let path = args.get("path").and_then(|v| v.as_str());
                        match compare_symbol(&repo_root, &cfg, sym, tag_a, tag_b, path) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("compare_symbol failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_chronos: received '{action}'. \
                        Choose one of: 'save_checkpoint' (snapshot before edit), 'list_checkpoints' (show all snapshots), \
                        or 'compare_checkpoint' (AST diff after edit). \
                        Example: cortex_chronos with action='save_checkpoint', path='src/main.rs', symbol_name='my_fn', and semantic_tag='pre-refactor'"
                    )),
                }
            }

            // Standalone tool
            "run_diagnostics" => {
                let repo_root = self.repo_root_from_params(&args);
                match run_diagnostics(&repo_root) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("diagnostics failed: {e}")),
                }
            }

            // ── Compatibility shims (not exposed in tool_list) ───────────
            // Keep these aliases so existing clients don't instantly break.
            "map_repo" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("map_overview");
                }
                self.tool_call(id, &json!({ "name": "cortex_code_explorer", "arguments": new_args }))
            }
            "get_context_slice" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("deep_slice");
                }
                self.tool_call(id, &json!({ "name": "cortex_code_explorer", "arguments": new_args }))
            }
            "read_symbol" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("read_source");
                }
                self.tool_call(id, &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }))
            }
            "find_usages" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("find_usages");
                }
                self.tool_call(id, &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }))
            }
            "call_hierarchy" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("blast_radius");
                }
                self.tool_call(id, &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }))
            }
            "propagation_checklist" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("propagation_checklist");
                }
                self.tool_call(id, &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }))
            }
            "save_checkpoint" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("save_checkpoint");
                }
                self.tool_call(id, &json!({ "name": "cortex_chronos", "arguments": new_args }))
            }
            "list_checkpoints" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("list_checkpoints");
                }
                self.tool_call(id, &json!({ "name": "cortex_chronos", "arguments": new_args }))
            }
            "compare_checkpoint" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("compare_checkpoint");
                }
                self.tool_call(id, &json!({ "name": "cortex_chronos", "arguments": new_args }))
            }

            // Deprecated (kept for now): skeleton reader
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

/// Returns `xml` inline when it is small enough for the agent context window.
/// For larger outputs, writes to a deterministic temp file and returns the path.
const INLINE_CHARS_THRESHOLD: usize = 8_000;

fn inline_or_spill(xml: String) -> String {
    if xml.len() <= INLINE_CHARS_THRESHOLD {
        return xml;
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    xml.hash(&mut h);
    let hash = h.finish();
    let path = std::path::PathBuf::from(format!("/tmp/cortexast_slice_{:x}.xml", hash));
    match std::fs::write(&path, xml.as_bytes()) {
        Ok(_) => format!(
            "📄 Output is large ({} chars, above {INLINE_CHARS_THRESHOLD}-char inline limit).\nWritten to: {}\n\nUse `read_file` tool with that path to read the full content.",
            xml.len(),
            path.display()
        ),
        Err(e) => format!(
            "(Output was {} chars — too large for inline, and failed to write to disk: {e})\n",
            xml.len()
        ),
    }
}
