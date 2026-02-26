# CortexAST 🧠⚡

> **The AI-Native Code Intelligence Backend for LLM Agents**
> Pure Rust · MCP Server · Semantic Code Navigation · AST Time Machine · Self-Evolving Wasm Parsers

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Version](https://img.shields.io/badge/version-2.1.0-blue)](./CHANGELOG.md)

---

## What is CortexAST?

> 👁️ **CortexAST is the "eyes"** — read-only code intelligence. For write/execute capabilities, see the companion [`cortex-act`](https://github.com/DevsHero/cortex-act) project (the "hands").

CortexAST is a **production-grade MCP (Model Context Protocol) server** that gives AI coding agents (Claude, Gemini, GPT-4o, etc.) the ability to:

- **Navigate codebases semantically** — find symbols, blast-radius analysis, cross-file propagation checklists
- **Evolve itself** — download and hot-reload WebAssembly language parsers at runtime (Go, PHP, Ruby, Java, …)
- **Time-travel your codebase** — Chronos snapshot system for pre/post-refactor AST-level comparison
- **Search local memory** — hybrid semantic + keyword search over your codebase history

> ✋ To **edit files**, **run commands**, or **patch configs**, use [cortex-act](https://github.com/DevsHero/cortex-act) instead.

---

## Feature Modules

### 🔭 cortex_code_explorer
Bird's-eye symbol map (`map_overview`) and token-budgeted XML slice (`deep_slice`) of any codebase.

### 🎯 cortex_symbol_analyzer
AST-accurate `read_source`, `find_usages`, `blast_radius`, and `propagation_checklist` — no grep false positives.

### ⏳ cortex_chronos
Save/compare/rollback named AST snapshots. Detects semantic regressions that `git diff` hides.

### 🧠 cortex_memory_retriever / cortex_remember
Global memory journal (`~/.cortexast/global_memory.jsonl`) with vector-semantic recall.

### 🌐 cortex_manage_ast_languages — Self-Evolving Agent
Download and **hot-reload Wasm parsers** at runtime. No restart required. Supports Go, PHP, C++, C#, Java, Ruby, C, and Dart.
```json
{ "action": "add", "languages": ["go", "php", "cpp"] }
```

### 📋 cortex_get_rules
Fetches and filters codebase AI rules based on context (frontend/backend/db). **Requires CortexSync**.

### 🌍 cortex_list_network
Discover all AI-tracked codebases in your machine. **Requires CortexSync**.

---

## Ecosystem Requirement: CortexSync 🧠

For full functionality, **CortexSync** (the "Brain") must be running in the background.

| Tool | Dependent on CortexSync? | Why? |
|---|---|---|
| `cortex_remember` | **Yes** | Persists task outcomes to the global journal. |
| `cortex_memory_retriever`| **Yes** | Performs semantic vector search over past decisions. |
| `cortex_get_rules` | **Yes** | Fetches centralized rules from the synchronized rule engine. |
| `cortex_list_network` | **Yes** | Reads the global network map of codebases. |
| `cortex_code_explorer` | No | Local AST analysis. |
| `cortex_symbol_analyzer` | No | Local AST analysis. |

If `cortex-sync` is offline, these tools will strictly return a graceful warning without interrupting the agent's workflow.

---

---

## Quick Start

### Prerequisites
- Rust 1.80+
- Ollama or [LM Studio](https://lmstudio.ai) running locally (optional, for Auto-Healer)

### Build & Run
```bash
git clone https://github.com/DevsHero/CortexAST
cd CortexAST
cargo build --release

# Run as MCP server (stdio)
./target/release/cortexast
```

### MCP Config (`~/.cursor/mcp.json` or Claude Desktop)
```json
{
  "mcpServers": {
    "cortexast": {
      "command": "/path/to/cortexast",
      "args": []
    }
  }
}
```

---

---

## Usage Examples

### Semantic Explorer — Bird's-eye view of a project
```json
{
  "name": "cortex_code_explorer",
  "arguments": {
    "action": "map_overview",
    "target_dir": "."
  }
}
```

### Symbol Search — Find all usages across the repo
```json
{
  "name": "cortex_symbol_analyzer",
  "arguments": {
    "action": "find_usages",
    "symbol_name": "AuthService",
    "target_dir": "."
  }
}
```

### Time Travel — Compare AST after refactor
```json
{
  "name": "cortex_chronos",
  "arguments": {
    "action": "compare_checkpoint",
    "symbol_name": "login",
    "tag_a": "pre-refactor",
    "tag_b": "__live__"
  }
}
```


## Self-Evolving Wasm Language Support

| Always Available | Downloadable on Demand |
|---|---|
| Rust, TypeScript/JS, Python | Go, PHP, Ruby, Java, C, C++, C#, Dart |

```bash
# Agent calls this automatically when it detects a new language:
cortex_manage_ast_languages { "action": "add", "languages": ["go", "dart"] }
```

---

## Development

```bash
# Run all unit tests
cargo test

# Check (no link)
cargo check

---

## Architecture

```
CortexAST (binary)
└── src/
    ├── server.rs         # MCP stdio server — all tool schemas + handlers
    ├── inspector.rs      # LanguageConfig, LanguageDriver, Symbol, run_query
    ├── grammar_manager.rs # Wasm download + hot-reload (GitHub releases)
    ├── vector_store.rs    # model2vec embeddings + cache invalidation
    ├── chronos.rs         # AST snapshot time machine (Chronos)
    ├── memory.rs          # global_memory.jsonl journal client
    └── project_map.rs     # Network map for multi-repo roaming
```


## License

MIT — See [LICENSE](./LICENSE)

---

*Built with ❤️ in Rust · Semantic precision for the AI age*