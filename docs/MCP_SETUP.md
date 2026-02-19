# CortexAST MCP Setup

CortexAST is a **Pure Rust MCP server** (stdio JSON-RPC). No editor-side add-on required.

## 1) Get the Binary

**Option A — Download pre-built binary** (recommended):

Visit [Releases](https://github.com/DevsHero/CortexAST/releases/latest) and download the binary for your OS. Make it executable on macOS/Linux:

```bash
chmod +x cortexast-macos-aarch64   # adjust filename for your platform
```

**Option B — Build from source**:

```bash
git clone https://github.com/DevsHero/CortexAST.git
cd CortexAST
cargo build --release
# binary: ./target/release/cortexast
```

## 2) Connect an MCP Client

Example config (Claude Desktop style):

```json
{
  "mcpServers": {
    "cortexast": {
      "command": "/absolute/path/to/cortexast",
      "args": ["mcp"]
    }
  }
}
```

Restart your MCP client.

## 3) MCP Tools

```
├─ get_context_slice(target, budget_tokens?, query?, query_limit?, repoPath?)
│  └─ Returns: token-budget-aware XML slice (skeletonized source)
├─ map_repo(target_dir, repoPath?)
│  └─ Returns: compact hierarchical text map of files + public symbols
├─ read_symbol(path, symbol_name, repoPath?)
│  └─ Returns: exact full source of a single symbol via AST
├─ find_usages(target_dir, symbol_name, repoPath?)
│  └─ Returns: all semantic references (no comment/string noise)
├─ call_hierarchy(target_dir, symbol_name, repoPath?)
│  └─ Returns: definition location + outgoing calls + incoming callers
└─ run_diagnostics(repoPath)
   └─ Returns: compiler errors pinned to file:line with code context

Chronos (AST Time Machine):

├─ save_checkpoint(path, symbol_name, semantic_tag, repoPath?)
│  └─ Saves a disk-backed snapshot under `.cortexast/checkpoints/`
├─ list_checkpoints(repoPath?)
│  └─ Lists available semantic tags + stored symbols
└─ compare_checkpoint(symbol_name, tag_a, tag_b, path?, repoPath?)
   └─ Displays Tag A and Tag B symbol code blocks (no unified diff)
```

## 4) Optional Repo Config

CortexAST reads `.cortexast.json` from the target repo root.
It only accepts `.cortexast.json`.

Note on real-world usage:

- For MCP usage, `.cortexast.json` is re-read on every tool call, so config edits take effect on the next request (no server restart required).
- If you change `vector_search.model` or `vector_search.chunk_lines`, CortexAST will automatically reset/rebuild the local vector index on the next query.

Example:

```json
{
  "output_dir": ".cortexast",
  "scan": {
    "exclude_dir_names": ["generated", "tmp", "fixtures"]
  },
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
