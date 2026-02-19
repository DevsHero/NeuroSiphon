# 🧠 CortexAST

**Extract the Signal. Discard the Noise.**
_The God-Tier Context Optimizer for LLMs. Pure Rust. Native MCP._

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)
[![MCP Ready](https://img.shields.io/badge/MCP-Ready-blue)](https://modelcontextprotocol.io/)
[![Build & Release](https://github.com/DevsHero/CortexAST/actions/workflows/release.yml/badge.svg)](https://github.com/DevsHero/CortexAST/actions/workflows/release.yml)

---

## ⚡️ What is CortexAST?

**CortexAST** is not just another file reader for AI. It is a **Context Refinery**.

When you feed code to an LLM (Claude, ChatGPT), a huge chunk of your tokens are wasted on:

- ❌ Massive import lists (`import { a, b, c, ... }`)
- ❌ Boilerplate comments
- ❌ Indentation / whitespace
- ❌ Implementation details irrelevant to your question

**CortexAST fixes this via “Nuclear Optimization”.**
It parses your code (AST), understands structure, nukes the fat, and feeds the LLM only the **pure logic marrow** it needs to reason.

---

## 🚀 The Launch Promises

- **JIT Vector Indexing**: "Always fresh, zero latency." (no watchers, no stale results)
- **Nuclear Optimization**: "Cuts token usage by 60%." (AST skeletonization + import nuking)
- **Pure Rust**: "No Python/Node bloat." (single native binary)

---

## 👑 Why It’s the King of Context Efficiency

| Feature | Standard Tools (copy/paste) | 🧠 CortexAST |
| :--- | :--- | :--- |
| **Optimization** | None (full text) | **Nuclear** (AST skeleton + import nuking) |
| **Search** | Grep / filename | **Hybrid** (vector semantics + graph ranking) |
| **Token Usage** | Bloated | **Cuts token usage by 60%** (often 40–60% less noise) |
| **Speed** | Overhead-heavy | **Pure Rust** (fast scan + fast slice) |
| **Privacy** | Often cloud-dependent | **100% local** (local embeddings + local index) |

### 🛠️ Key Technologies

1. **Nuclear Skeletonization**
   - Functions collapse to signatures: `fn complex_logic() { /* ... */ }`
   - Imports collapse to one line: `// ... (imports nuked)`
   - Indentation is flattened to save whitespace tokens

2. **JIT Vector Indexing (Always Fresh)**
  - No background watcher: **0 CPU until you query**
  - Before every search, CortexAST refreshes the index using a fast size sweep + **xxh3 content hashing** + incremental embed updates
  - Result: you can edit/generate files and query immediately without stale results

3. **Lightweight Hybrid Search**
   - Uses **Model2Vec** (pure Rust, no ONNX runtime) for fast embeddings
   - Uses a **flat-file JSON index** + brute-force cosine (trivial for typical repo sizes)
   - Ranked against a dependency graph so you get **core logic first**

4. **Native MCP Server**
   - Built for **Claude Desktop** / **Cursor** / any MCP client
   - No plugins, no API keys, no cloud required

---

## 🧬 New MCP Tools (v1.3.0)

CortexAST now exposes **nine** AST-powered MCP tools. The three latest give an LLM a God-tier view of any codebase with zero token waste.

- **`map_repo`** (The God's Eye)
  - Args: `target_dir`
  - Returns a compact **hierarchical text tree** of every source file grouped by directory, listing only **exported/public symbols** per file.
  - Capped at ~8 000 chars. Perfect for orientation before diving deeper.

- **`call_hierarchy`** (The Call Graph)
  - Args: `target_dir`, `symbol_name`
  - Returns three sections: **Definition** location, **Outgoing calls** (callees inside the body), and **Incoming calls** (all callers with enclosing function context).
  - Works without compilation — raw tree-sitter AST only.

- **`run_diagnostics`** (The Compiler Oracle)
  - Args: `repoPath`
  - Auto-detects project type (`Cargo.toml` → `cargo check`; `package.json` → `tsc --noEmit`).
  - Returns a structured Markdown error report pinned to file:line with 1-line code context per error.

---

## 🧬 AST Tools (v1.2.0)

CortexAST exposes two earlier AST-powered tools designed to *avoid heavy full-file reads* and *avoid noisy grep*.

- **`read_symbol`** (The X-Ray)
  - Args: `path`, `symbol_name`
  - Returns the **full, unpruned** source of that symbol by extracting the exact Tree-sitter byte range.
  - Best for: “Show me the full implementation of `ConvertRequest`.”

- **`find_usages`** (The AST-Tracer)
  - Args: `target_dir`, `symbol_name`
  - Walks files with `ignore` + parses with Tree-sitter.
  - Matches only semantic identifier nodes (not comments/strings) and returns a dense list of:
    - `[File Path: Line Number]` + 2 lines above/below as context

---

## 🏆 CortexAST v1.0.0: Official Benchmarks

Target: CortexAST Source Code (10+ Rust Files, Core Logic)

Hardware: Apple M4 pro / 14CPU 20GPU 24GB RAM

Evidence (CLI run screenshot): [screenshot/Screenshot 2569-02-18 at 12.35.37.png](screenshot/Screenshot%202569-02-18%20at%2012.35.37.png)

| Metric | Raw Source (Baseline) | 🧠 CortexAST (Nuclear) | Improvement |
|---|---:|---:|---:|
| Total Size | 127,536 Bytes | 9,842 Bytes | 92.3% Smaller |
| Est. Tokens (≈ bytes/4) | ~31,884 tokens | ~2,460 tokens | 29,424 Tokens Saved |
| Processing Time | N/A | 0.07 Seconds | Instant (JIT) |
| Information Density | Low (Noise Heavy) | God Tier (Pure Logic) | Refined |
| LLM Context Space | 100% Full | 7.7% Used | 92.3% Free Space |

## 📦 Installation

### Option A — Pre-built Binary (fastest)

Download the latest binary from [Releases](https://github.com/DevsHero/CortexAST/releases/latest):

| Platform | Download |
|---|---|
| Linux x86_64 | `cortexast-linux-x86_64` |
| Linux ARM64 | `cortexast-linux-aarch64` |
| macOS Intel | `cortexast-macos-x86_64` |
| macOS Apple Silicon | `cortexast-macos-aarch64` |
| Windows x86_64 | `cortexast-windows-x86_64.exe` |

```bash
# macOS / Linux — make executable
chmod +x cortexast-*
./cortexast-macos-aarch64 --help
```

### Option B — Build from Source

```bash
git clone https://github.com/DevsHero/CortexAST.git
cd CortexAST
cargo build --release
# Binary: ./target/release/cortexast
```

See [BUILDING.md](docs/BUILDING.md) for cross-compilation and platform-specific instructions.

---

## 🔌 MCP Setup

Add to your MCP client config (example uses Claude Desktop style JSON): 

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

Restart your MCP client. That’s it. 

See [MCP_SETUP.md](docs/MCP_SETUP.md)

---

## 🎮 Usage

### Automatic (via Chat)

Example:

> “@cortexast What is the authentication flow in this project?”

CortexAST will:

1. Vector-search for “authentication”
2. Graph-rank the results (core logic > tests)
3. Skeletonize and nuke imports to fit your token budget
4. Return an optimized context slice

### Deep-Dive (AST Tools)

If the LLM needs precise information without reading whole files:

- Get a codebase bird's-eye view via `map_repo`
- Trace full call graphs (incoming + outgoing) via `call_hierarchy`
- Ask for a symbol implementation via `read_symbol`
- Trace semantic usages across the repo via `find_usages`
- Run compiler diagnostics and get pinned errors via `run_diagnostics`

These work even when the repo doesn’t compile and the LSP is broken.

---

## 🗒️ Changelog

### v1.3.0
- **`map_repo`** — compact hierarchical codebase map: directories + exported symbols, ~8 000-char budget
- **`call_hierarchy`** — outgoing call targets (tree-sitter `call_expression` extraction) + incoming callers with enclosing-function context
- **`run_diagnostics`** — auto-detect Rust/TypeScript project, run `cargo check` / `tsc --noEmit`, parse structured JSON output, return pinned errors with code snippets

### v1.2.0

- **Vector index v2**: deterministic cache invalidation via **xxh3 content hashing** (no more mtime/git-checkout drift)
- **AST-aware semantic chunking**: large files embed as multiple chunks (less vector dilution)
- **Symbol anchoring**: boosts results when query contains an exact symbol name
- **New MCP tools**: `read_symbol` and `find_usages`

### Manual (CLI)

```bash
# Optimized slice of a single service
cortexast --target src --budget-tokens 32000 --xml

# Semantic search for specific concepts
cortexast --target . --query "database connection" --xml

# === MONOREPO / MICROSERVICE SUPPORT ===

# Huge-codebase mode: splits budget across ALL workspace members automatically.
# Works with Cargo workspaces, npm workspaces, and auto-detected sub-projects.
# Handles double/triple nested services (services/foo/bar/Cargo.toml, etc.)
cortexast --target . --huge --xml

# Inspect all discovered workspace members without slicing:
cortexast --list-members

# Target a specific nested service within a monorepo:
cortexast --target services/core_api --xml
cortexast --target apps/frontend --xml

# Query across the whole monorepo (JIT hybrid search spans all services):
cortexast --query "gRPC handler for embeddings" --xml
```

---

## 🏗️ Architecture

CortexAST drops heavy infra in favor of a compact, custom-built engine:

- **Vector Store**: flat-file JSON index + brute-force cosine similarity
- **Parser**: tree-sitter + safe fallbacks for broad language coverage
- **Walker**: `ignore` crate that respects `.gitignore` and auto-skips high-noise dirs (e.g. `target` and other generated build outputs)
- **Workspace Engine**: `workspace.rs` — discovers all workspace members (Cargo, npm, Python, Go) up to N levels deep, supports glob include/exclude patterns

### 🏢 Monorepo & Huge-Codebase Support

CortexAST ships production-grade support for the most complex repository structures:

| Feature | Details |
|---|---|
| **Auto-detection** | Repos with ≥5 declared workspace members activate huge mode automatically |
| **Nested services** | Scans up to 3 levels deep by default — handles `services/*/`, `apps/*/`, `packages/*/` patterns |
| **Budget splitting** | Each workspace member gets a proportional token share; no single service crowds out others |
| **Root context** | Top-level workspace manifests and READMEs always included at minimal cost |
| **Explicit control** | `--huge` forces huge mode; `--list-members` shows what was detected |

#### Huge-Codebase Benchmark ( 22 services, 429 files)

| Mode | Files Included | Output Size | Est. Tokens | Time |
|---|---|---|---|---|
| `--target .` (normal) | ~32 | ~128 KB | ~32K | 0.57s |
| `--target . --huge` | **274** | **314 KB** | **~78K** | **0.60s** |
| `--target services/core_api` | 63 | 83 KB | ~21K | 0.08s |
| `--query "embedding pipeline"` | 21 | 19 KB | ~5K | 0.25s |

> All benchmarks: Apple M4 Pro, release build.

#### `.cortexast.json` — Huge-Codebase Config

Notes:

- CortexAST re-reads `.cortexast.json` on each run/request; changes apply immediately on the next slice/query.
- Changing `vector_search.model` or `vector_search.chunk_lines` triggers an automatic vector-index rebuild the next time you use `--query` (or MCP query mode).

```json
{
  "vector_search": {
    "model": "minishlab/potion-retrieval-32M",
    "chunk_lines": 40,
    "default_query_limit": 30
  },
  "token_estimator": {
    "chars_per_token": 4,
    "max_file_bytes": 524288
  },
  "scan": {
    "exclude_dir_names": ["generated", "tmp", "fixtures"]
  },
  "huge_codebase": {
    "enabled": true,
    "member_scan_depth": 3,
    "min_member_budget": 4000,
    "include_members": ["services/*", "shared/*"],
    "exclude_members": ["**/tmp", "**/sample-*"]
  }
}
```

### 🛡️ Bulletproof Design

CortexAST is engineered to survive “dirty” enterprise monorepos:

- **Binary Safety**: Detects null bytes and skips binary/non-UTF8 files safely (no crashes on `.exe`, encrypted keys, or garbage bytes)
- **Resource Guard**: Strict **1MB hard limit per file** + **line-length checks** to prevent minified/generated code from hanging the parser
- **Self-Healing Index**: Detects corrupted vector indices and auto-rebuilds on the next query (no manual cleanup required)
- **Chaos Tested**: Validated against edge cases (null bytes, massive single-line files, and broken JSON)

---

## 🤝 Contributing

PRs welcome.

- Core: Rust (Tokio, Rayon, Model2Vec)
- Focus: performance, compression ratio, multi-language correctness

Crafted with 🦀 by DevsHero.