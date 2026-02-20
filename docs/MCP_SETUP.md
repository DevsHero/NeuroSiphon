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

CortexAST exposes **4 Megatools** (preferred) with `action` enums.
Legacy tool names are accepted as compatibility shims but are deprecated.

```
Megatools (preferred):

├─ cortex_code_explorer(action, ...)
│  ├─ action=map_overview(target_dir, search_filter?, max_chars?, ignore_gitignore?, repoPath?)
│  └─ action=deep_slice(target, budget_tokens?, query?, query_limit?, skeleton_only?, max_chars?, repoPath?)
│     └─ Returns: token-budget-aware XML slice (optionally skeleton-only)

├─ cortex_symbol_analyzer(action, ...)
│  ├─ action=read_source(path, symbol_name? | symbol_names?, skeleton_only?, max_chars?, repoPath?)
│  ├─ action=find_usages(target_dir, symbol_name, max_chars?, repoPath?)
│  ├─ action=find_implementations(target_dir, symbol_name, max_chars?, repoPath?)
│  ├─ action=blast_radius(target_dir, symbol_name, max_chars?, repoPath?)
│  └─ action=propagation_checklist(symbol_name, aliases?, target_dir?, ignore_gitignore?, max_chars?, repoPath?)

├─ cortex_chronos(action, ...)
│  ├─ action=save_checkpoint(path, symbol_name, semantic_tag, repoPath?)
│  ├─ action=list_checkpoints(repoPath?)
│  ├─ action=compare_checkpoint(symbol_name, tag_a, tag_b, path?, repoPath?)
│  │  └─ Magic: tag_b="__live__" compares tag_a against current filesystem state (requires path)
│  └─ action=delete_checkpoint(symbol_name?, semantic_tag?/tag?, path?, repoPath?)

└─ run_diagnostics(repoPath, max_chars?)
  └─ Returns: compiler errors pinned to file:line with code context
```

Output safety:
- All tools support `max_chars` (default 15000, max 30000). The server truncates inline and appends a marker to prevent editor-side spill.

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
