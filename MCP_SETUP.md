# Context-Slicer MCP Setup & Configuration

## Status: ✅ Setup Complete

### 1. Updated mcp.json
- **Renamed**: "context-slicer-local" → "context-slicer"
- **Description**: Added note about vector search support + auto-tune parameters
- **Binary Path**: `/Users/hero/Documents/GitHub/context-slicer/target/release/context-slicer mcp`

### 2. Latest Features (New Build)
- ✅ **Auto-tuned `--query-limit`**: Heuristic based on `--budget-tokens` + config default
- ✅ **`--embed-model`**: Override embedding model (e.g., `minishlab/potion-retrieval-32M`)
- ✅ **`--chunk-lines`**: Override chunk size for vector indexing
- ✅ **`vector_search` config block**: Set defaults in `.context-slicer.json`
- ✅ **Incremental indexing**: Skip re-reading unchanged files (faster reruns)

### 3. MCP Tools Available
```
├─ get_context_slice(target, budget_tokens?, repoPath?)
│  └─ Returns: XML context slice
├─ get_repo_map(scope?, repoPath?)
│  └─ Returns: JSON repo map with nodes + edges
├─ read_file_skeleton(path, repoPath?)
│  └─ Returns: Low-token skeleton view (function bodies pruned)
└─ read_file_full(path, repoPath?)
   └─ Returns: Full raw file content
```

### 4. Quick Test (After VS Code Restart)
Use the MCP extension in VS Code:
1. Open a file in the repo
2. Right-click → "Call MCP Tool" in command palette
3. Select "context-slicer" → "get_context_slice"
4. Set params: `target: "core/src"` | `budget_tokens: 12000`
5. View the resulting XML in an output panel

### 5. CLI Usage Examples

#### Auto-tune vector search (new default behavior):
```bash
./target/release/context-slicer \
  --target src \
  --query "skeleton inspector vector" \
  --budget-tokens 12000 \
  --xml > /dev/null
```
→ `--query-limit` is auto-computed; final budget enforced by slicer

#### Custom embedding model (retrieval-optimized):
```bash
./target/release/context-slicer \
  --target . \
  --query "vector search" \
  --embed-model minishlab/potion-retrieval-32M \
  --budget-tokens 32000 \
  --xml > /dev/null
```

#### Custom chunk size (larger chunks):
```bash
./target/release/context-slicer \
  --target . \
  --query "config index" \
  --chunk-lines 20 \
  --budget-tokens 12000 \
  --xml > /dev/null
```

#### Override query limit explicitly:
```bash
./target/release/context-slicer \
  --target . \
  --query "slicer mapper" \
  --query-limit 15 \
  --budget-tokens 12000 \
  --xml > /dev/null
```

### 6. Configuration File (.context-slicer.json)

Set organization-wide defaults:
```json
{
  "output_dir": ".context-slicer",
  "skeleton_mode": true,
  "vector_search": {
    "model": "minishlab/potion-retrieval-32M",
    "chunk_lines": 10,
    "default_query_limit": 25
  },
  "token_estimator": {
    "chars_per_token": 4,
    "max_file_bytes": 1048576
  }
}
```

### 7. Performance Notes

Sample benchmarks (on a macOS dev machine):

| Scenario | Baseline | Vector (Warm) | Cold Start |
|----------|----------|---------------|-----------|
| core/src (12k tokens) | ~77ms | ~57ms (0.74×) | N/A |
| repo-wide (32k tokens) | ~134ms | ~117ms (0.87×) | ~1.2s |

- **Warm query**: Vector search reuses indexed DB; typically faster for large repos
- **Cold start**: First query builds the index + loads embedding model; ~1-2s overhead
- **Best for**: Large repos where you want to narrow scope; inefficient for tiny codebases

### 8. Best Practice Workflow

1. **Always scope `--target`** to the smallest subtree (most important step)
2. **Use `--query`** to automatically select relevant files
3. **Omit `--query-limit`** to let it auto-tune (or set explicitly if recall is too low)
4. **Choose embedding model** based on your task:
   - `potion-base-8M`: General purpose (fast, small; default)
   - `potion-retrieval-32M`: Optimized for code retrieval tasks
   - `potion-multilingual-128M`: Multi-language support

### 9. Next Steps
- Restart VS Code to reconnect the MCP server
- Verify MCP tools show up in VS Code's command palette
- Optionally test query mode with your own codebase

---

*Last updated: 2026-02-17 (with auto-tune + incremental indexing)*
