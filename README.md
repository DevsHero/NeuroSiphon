# 🧠 CortexAST

**The AI-Native IDE Backend. Extract the Signal, Discard the Noise.**
_Empowering LLM Agents with God-tier semantic understanding, pure AST vision, and nuclear token efficiency._

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)
[![MCP Ready](https://img.shields.io/badge/MCP-Ready-blue)](https://modelcontextprotocol.io/)
[![Build & Release](https://github.com/DevsHero/CortexAST/actions/workflows/release.yml/badge.svg)](https://github.com/DevsHero/CortexAST/actions/workflows/release.yml)

---

## ⚡️ The Paradigm Shift: From "Slicer" to "Orchestrator"

Most AI coding agents rely on tools built for *human eyeballs* (like `cat`, `grep`, `tree`, or `git diff`). For an LLM, these tools are toxic. They flood the context window with whitespace, comments, unstructured noise, and force the AI into "amnesia" due to pagination.

**CortexAST is different. It is a sensory system built strictly for AI brains.**
Powered by Tree-sitter (AST) and written in ultra-fast Rust, CortexAST bridges the gap by giving your AI agents a deterministic, high-fidelity understanding of your entire codebase—dropping token usage by up to 90% while retaining 100% of the architectural logic.

---

## 🥊 Why CortexAST Crushes Native IDE Tools

| Task | ❌ Standard IDE Tools (For Humans) | 🧠 CortexAST (For AI Agents) | Result for LLMs |
| :--- | :--- | :--- | :--- |
| **Exploration** | `tree` or `ls` (Only shows filenames) | **`map_repo`** (Shows files + exact public Structs/Functions inside) | Instant architectural map. |
| **Reading Code** | `cat` or IDE Read File (Loads 2,000 lines) | **`read_symbol`** (X-Rays only the exact 50-line AST node needed) | Nuclear token savings. |
| **Finding Stuff** | `grep` (String-matching noise & comments) | **`find_usages`** (AST-aware, categorized by Calls, Type Refs, Fields) | Zero false positives. |
| **Refactoring** | `git diff` (Indent/whitespace line noise) | **`Chronos`** (Semantic AST snapshot comparisons) | Crystal clear AI reasoning. |
| **Cross-Service** | Guessing & manual searching | **`propagation_checklist`** (Proto ➔ Rust ➔ TS checklist) | Prevents stray/missing lines. |

---

## 🛠️ The God-Tier MCP Toolset (v2.0)

CortexAST exposes highly optimized, short-named tools via the Model Context Protocol (MCP) to ensure AI agents prefer them over basic terminal commands.

### 🗺️ The Navigator
* **`map_repo`**: The God's Eye. Returns a hierarchical map of the codebase showing files and their exported symbols. Includes **Smart Filtering** (`search_filter: "auth"`) to avoid token-heavy pagination.

### 🎯 The Surgeon
* **`read_symbol`**: The X-Ray. Why read a whole file? Pass an array of `["symbol_A", "symbol_B"]` and CortexAST will extract *only* the exact Tree-sitter byte ranges for those functions/structs in a single batch request.

### 🕸️ The Architect
* **`call_hierarchy`**: Blast Radius analyzer. Shows exactly what calls a function (Incoming) and what the function calls (Outgoing). Crucial for safe refactoring.
* **`find_usages`**: Semantic tracer. Finds 100% accurate AST usages across the workspace, beautifully categorized into *Function Calls*, *Field Initializations*, and *Type References*.
* **`propagation_checklist`**: The Cross-Language Safety Net. Generates a strict Markdown checklist (e.g., `.proto` ➔ `.rs` ➔ `.ts`) to ensure the AI updates all layers of a microservice when a core contract changes.

### 🚨 The Debugger
* **`run_diagnostics`**: The Compiler Whisperer. Auto-detects the project type (`cargo check` / `tsc --noEmit`), runs the compiler, and maps raw errors directly back to 1-line AST source context.

### ⏳ Chronos (The AST Time Machine)
Git diffs confuse LLMs with line-number and whitespace noise. Chronos allows the AI to "Save State" and compare semantic logic.
* **`save_checkpoint`**: Snapshots a specific symbol's AST to disk with a semantic tag (e.g., `pre-refactor`). Zero RAM bloat.
* **`list_checkpoints`**: Shows available time periods.
* **`compare_checkpoint`**: Compares `baseline` vs `post-error-handling` side-by-side structurally.

---

## 🏆 Nuclear Optimization: Official Benchmarks

Target: CortexAST Source Code (10+ Rust Files, Core Logic)
Hardware: Apple M4 Pro / 14CPU 20GPU 24GB RAM

| Metric | Raw Copy-Paste (Standard IDE) | 🧠 CortexAST (Nuclear Mode) |
| :--- | :--- | :--- |
| **Total Size** | 127,536 Bytes | **9,842 Bytes (92.3% Smaller)** |
| **Est. Token Cost** | ~31,884 tokens | **~2,460 tokens** |
| **Processing Time**| N/A | **< 0.1 Seconds (Pure Rust)** |
| **Information Density**| Low (Noise Heavy) | **God Tier (Pure Logic)** |

---

## 🏗️ Core Architecture 

* **Nuclear Skeletonization**: Functions collapse to signatures (`fn logic() { /* ... */ }`), imports are nuked, and indentation is flattened.
* **JIT Hybrid Vector Search**: Uses `model2vec-rs` (pure Rust, <100MB RAM) for lightning-fast embedding. Zero background watchers—it hashes content via `xxh3` and updates incrementally *only* when you query.
* **Enterprise Workspace Engine**: Discovers nested microservices (`services/foo/bar/Cargo.toml` or `apps/frontend/package.json`) and intelligently routes context budgets across monorepos automatically.
* **Bulletproof Safety**: Detects null bytes, skips massive minified bundles (1MB hard cap), and survives broken code without crashing.

---

## 👑 Why It’s the King of Context Efficiency

| Feature | Standard Tools (copy/paste) | 🧠 CortexAST |
| :--- | :--- | :--- |
| **Optimization** | None (full text) | **Nuclear** (AST skeleton + import nuking) |
| **Search** | Grep / filename | **Hybrid** (vector semantics + graph ranking) |
| **Token Usage** | Bloated | **Cuts token usage by 60%** (often 40–60% less noise) |
| **Speed** | Overhead-heavy | **Pure Rust** (fast scan + fast slice) |
| **Privacy** | Often cloud-dependent | **100% local** (local embeddings + local index) |



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

## 🤝 Contributing

PRs welcome.

- Core: Rust (Tokio, Rayon, Model2Vec)
- Focus: performance, compression ratio, multi-language correctness

Crafted with 🦀 by DevsHero.