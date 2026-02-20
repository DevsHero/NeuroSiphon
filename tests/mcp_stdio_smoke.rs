use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn mcp_stdio_smoke() {
    // `cargo test` sets this for integration tests.
    let bin = env!("CARGO_BIN_EXE_cortexast");
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cortexast mcp");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");

        // Keep each JSON-RPC message on one line (server reads by lines()).
        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "cortex_code_explorer",
                    "arguments": { "repoPath": repo_root, "action": "map_overview", "target_dir": "." }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "cortex_symbol_analyzer",
                    "arguments": { "repoPath": repo_root, "action": "read_source", "path": "src/inspector.rs", "symbol_name": "LanguageDriver" }
                }
            })
        )
        .unwrap();
    }

    // Close stdin so the server loop can exit.
    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("child stdout");
    let reader = BufReader::new(stdout);

    let mut replies_by_id: HashMap<i64, serde_json::Value> = HashMap::new();

    for line in reader.lines() {
        let line = line.expect("read stdout line");
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).expect("stdout is json");
        let id = v
            .get("id")
            .and_then(|x| x.as_i64())
            .expect("json-rpc response id");
        replies_by_id.insert(id, v);
        if replies_by_id.len() >= 4 {
            break;
        }
    }

    let status = child.wait().expect("wait child");
    assert!(status.success(), "mcp process should exit cleanly");

    // initialize
    {
        let v = replies_by_id.get(&1).expect("initialize reply");
        assert_eq!(v.get("jsonrpc").and_then(|x| x.as_str()), Some("2.0"));
        let result = v.get("result").expect("initialize result");
        assert!(result.get("capabilities").is_some());
    }

    // tools/list
    {
        let v = replies_by_id.get(&2).expect("tools/list reply");
        let tools = v
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .expect("tools array");
        let names: std::collections::HashSet<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        for required in [
            "cortex_code_explorer",
            "cortex_symbol_analyzer",
            "cortex_chronos",
            "run_diagnostics",
        ] {
            assert!(names.contains(required), "missing tool: {required}");
        }
    }

    // map_repo
    {
        let v = replies_by_id.get(&3).expect("map_repo reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(
            result.get("isError").and_then(|x| x.as_bool()),
            Some(false),
            "map_repo should not error"
        );
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("map_repo text");
        assert!(!text.is_empty(), "map_repo should return a non-empty text map");
        assert!(text.contains("src/"), "map_repo should reference src/ directory");
    }

    // read_symbol
    {
        let v = replies_by_id.get(&4).expect("read_symbol reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(result.get("isError").and_then(|x| x.as_bool()), Some(false));
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("read_symbol text");
        assert!(
            text.contains("LanguageDriver") || text.contains("trait ") || text.contains("fn "),
            "read_symbol should return source containing the symbol"
        );
    }
}
