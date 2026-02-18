use anyhow::{Context, Result};
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::config::load_config;
use crate::inspector::render_skeleton;
use crate::mapper::{build_repo_map, build_repo_map_scoped};
use crate::slicer::slice_to_xml;

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
                        "description": "Generate an XML context slice for a target directory/file within a repo",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "target": { "type": "string" },
                                "budget_tokens": { "type": "integer", "exclusiveMinimum": 0 }
                            },
                            "required": ["target"]
                        },
                        "execution": { "taskSupport": "forbidden" }
                    },
                    {
                        "name": "get_repo_map",
                        "description": "Return a repository map (nodes + edges) for the repo or a scoped subdirectory",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "scope": { "type": "string", "description": "Optional subdirectory path to scope mapping" }
                            }
                        },
                        "execution": { "taskSupport": "forbidden" }
                    },
                    {
                        "name": "read_file_skeleton",
                        "description": "Return a low-token skeleton view of a file (function bodies pruned)",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "path": { "type": "string" }
                            },
                            "required": ["path"]
                        },
                        "execution": { "taskSupport": "forbidden" }
                    },
                    {
                        "name": "read_file_full",
                        "description": "Return the full raw content of a file",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "repoPath": { "type": "string" },
                                "path": { "type": "string" }
                            },
                            "required": ["path"]
                        },
                        "execution": { "taskSupport": "forbidden" }
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

        let err = |text: String| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type":"text","text": text }], "isError": true }
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
                let abs = repo_root.join(p);

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
                let abs = repo_root.join(p);

                match std::fs::read_to_string(&abs).with_context(|| format!("Failed to read {}", abs.display())) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("read failed: {e}")),
                }
            }
            _ => err(format!("Tool not found: {name}")),
        }
    }
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

        let id = msg.get("id").cloned().unwrap_or(json!(null));
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let reply = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": msg.get("params").and_then(|p| p.get("protocolVersion")).cloned().unwrap_or(json!("2024-11-05")),
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": { "name": "context-slicer-rs", "version": "0.1.0" }
                }
            }),
            "tools/list" => state.tool_list(id),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                state.tool_call(id, &params)
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            }),
        };

        writeln!(stdout, "{}", reply)?;
        stdout.flush()?;
    }

    Ok(())
}
