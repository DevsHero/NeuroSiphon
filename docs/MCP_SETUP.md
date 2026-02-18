# NeuroSiphon MCP Setup

NeuroSiphon is a **Pure Rust MCP server** (stdio JSON-RPC). No editor-side add-on required.

## 1) Get the Binary

**Option A — Download pre-built binary** (recommended):

Visit [Releases](https://github.com/DevsHero/NeuroSiphon/releases/latest) and download the binary for your OS. Make it executable on macOS/Linux:

```bash
chmod +x neurosiphon-macos-aarch64   # adjust filename for your platform
```

**Option B — Build from source**:

```bash
git clone https://github.com/DevsHero/NeuroSiphon.git
cd NeuroSiphon
cargo build --release
# binary: ./target/release/neurosiphon
```

## 2) Connect an MCP Client

Example config (Claude Desktop style):

```json
{
  "mcpServers": {
    "neurosiphon": {
      "command": "/absolute/path/to/neurosiphon",
      "args": ["mcp"]
    }
  }
}
```

Restart your MCP client.

## 3) MCP Tools

```
├─ get_context_slice(target, budget_tokens?, repoPath?, query?, query_limit?)
│  └─ Returns: XML context slice
├─ get_repo_map(scope?, repoPath?)
│  └─ Returns: JSON repo map (nodes + edges)
├─ read_file_skeleton(path, repoPath?)
│  └─ Returns: Low-token skeleton view
└─ read_file_full(path, repoPath?)
   └─ Returns: Full raw file content
```

## 4) Optional Repo Config

NeuroSiphon reads `.neurosiphon.json` from the target repo root.
It only accepts `.neurosiphon.json`.

Example:

```json
{
  "output_dir": ".neurosiphon",
  "skeleton_mode": true,
  "vector_search": {
    "model": "minishlab/potion-base-8M",
    "chunk_lines": 40,
    "default_query_limit": 30
  },
  "token_estimator": {
    "chars_per_token": 4,
    "max_file_bytes": 1048576
  }
}
```
