# context-slicer

High-signal context slicing for coding agents.

What you get:
- **Rust CLI** that emits `.context-slicer/active_context.xml`
- **MCP stdio server** (JSON-RPC) so agents can call slicing + file reads
- **Skeleton-first** output (function bodies pruned) + aggressive noise reduction
- Optional **local vector search** (`--query`) backed by **LanceDB** + **Model2Vec**

## Status
- Rust CLI + MCP server: usable

## Quick start

### Build
Requirements: Rust toolchain.

- Build (release): `cargo build --release`

### Generate context (baseline)
From repo root:

- Slice a target folder: `./core/target/release/context-slicer --target . --budget-tokens 32000 --xml > /dev/null`
- Slice a target folder: `./target/release/context-slicer --target . --budget-tokens 32000 --xml > /dev/null`

Outputs:
- `.context-slicer/active_context.xml`
- `.context-slicer/active_context.meta.json`

### Generate context using vector search (hybrid)
Use `--query` to index (incrementally) and slice only the most relevant files:

- `./core/target/release/context-slicer --target . --query "vector search lancedb" --budget-tokens 32000 --xml > /dev/null`
- `./target/release/context-slicer --target . --query "vector search lancedb" --budget-tokens 32000 --xml > /dev/null`

Tips:
- To avoid pulling in unrelated areas, scope indexing to the subtree you care about:
  - `./target/release/context-slicer --target src --query "vector search lancedb" --xml > /dev/null`
- If you omit `--query-limit`, it auto-tunes based on your `--budget-tokens` (and config defaults).

## How it works

### Skeleton mode (default)
For supported languages, we prune function / method bodies (keeps structure and signatures).
We also aggressively reduce noise:
- Import blocks are collapsed into a single hint line (counts)
- Comment-only lines are removed (TODO/FIXME can be preserved)
- Brace-based languages are flattened (leading indentation stripped)

Unsupported code-like languages fall back to a regex-based skeleton (keeps definition-ish lines).

### Vector search storage
When `--query` is used:
- Vectors are stored in `.context-slicer/db` (LanceDB)
- Indexing is incremental via `.context-slicer/db/index_meta.json`
- Embeddings come from `model2vec-rs` (Model2Vec) using `minishlab/potion-base-8M` by default

## CLI reference

Common flags:
- `--target <PATH>`: directory or file to slice (relative to repo root)
- `--budget-tokens <N>`: approximate budget (default 32000)
- `--full`: disable skeleton mode (emits full file contents)
- `--xml`: print XML to stdout (still always writes `.context-slicer/active_context.xml`)

Vector search:
- `--query <TEXT>`: run vector search and slice only the most relevant files
- `--query-limit <N>`: max number of unique file paths returned (if omitted, auto-tuned)
- `--embed-model <MODEL_ID>`: override Model2Vec model (HF repo ID)
- `--chunk-lines <N>`: override snippet size (lines per file) used for indexing

MCP:
- `context-slicer mcp`

## Config
Optional `.context-slicer.json` in repo root:

```json
{
  "output_dir": ".context-slicer",
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

### Recommended workflow (best practice)
Goal: pull only the relevant files, using as few tokens as possible.

- Always scope `--target` to the smallest subtree you care about.
  - Example: `--target src` instead of `--target .`
- Use `--query` for “find the relevant area” and let the slicer enforce the token budget.
  - Example (auto limit): `./target/release/context-slicer --target src --query "index meta lancedb" --budget-tokens 12000 --xml > /dev/null`
- If recall is too low (misses files), raise `--budget-tokens` or set `--query-limit` explicitly.
  - Example: `--query-limit 20`
- If you do lots of retrieval queries, consider a retrieval-tuned model:
  - Example: `--embed-model minishlab/potion-retrieval-32M`

## MCP server
Start: `./target/release/context-slicer mcp`

Tools exposed:
- `context_slicer_get_context_slice`
- `context_slicer_get_repo_map`
- `context_slicer_read_file_skeleton`
- `context_slicer_read_file_full`

## Notes / troubleshooting
- First `--query` run downloads a small embedding model (cached).
- If you want maximum relevance, set `--target` to the sub-tree you care about.

## Benchmarks (sample)
These are sample timings from one developer machine (macOS, release build). Your results will vary.

- Repo-wide (`--target .`, `--budget-tokens 32000`)
  - Baseline median: ~0.134s
  - Vector (`--query "vector search lancedb"`, warm index) median: ~0.117s (~0.87×)
  - Vector cold start (no `.context-slicer/db`): ~1.241s (includes initial indexing + model load)
- Core-only (`--target core/src`, `--budget-tokens 12000`)
  - Baseline median: ~0.077s
  - Vector (`--query slicer`, warm index) median: ~0.057s (~0.74×)
