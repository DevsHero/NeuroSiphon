# 🧠 CortexAST

**The AI-Native Code Intelligence Backend. Extract the Signal, Discard the Noise.**  
_Giving LLM agents deterministic, AST-level understanding of any codebase — at nuclear token efficiency._

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)
[![MCP Ready](https://img.shields.io/badge/MCP-Ready-blue)](https://modelcontextprotocol.io/)
[![Version](https://img.shields.io/badge/version-1.5.0-green)](CHANGELOG.md)

---

## ⚡ Why CortexAST

Most AI coding agents rely on tools built for *human eyeballs* — `cat`, `grep`, `tree`, `git diff`. For an LLM these are toxic: they flood the context window with whitespace, comments, full file dumps, and force the agent into "amnesia" from pagination.

**CortexAST is a sensory system built strictly for AI brains.**  
Powered by [Tree-sitter](https://tree-sitter.github.io/) and written in pure Rust, it gives agents a deterministic, high-fidelity understanding of entire codebases — cutting token usage by up to 90 % while preserving 100 % of the architectural logic.

---

## 🥊 CortexAST vs. Standard IDE Tools

| Task | ❌ Standard (For Humans) | 🧠 CortexAST (For AI) | Result |
|:---|:---|:---|:---|
| **Exploration** | `tree` / `ls` — filenames only | `map_repo` — files + public symbols inside | Instant architecture map |
| **Reading Code** | `cat` — 2 000-line dump | `read_symbol` — exact AST node only | Nuclear token savings |
| **Finding Stuff** | `grep` — string matches incl. comments | `find_usages` — AST-accurate, zero false positives | Calls / Type Refs / Fields |
| **Refactoring** | `git diff` — line & whitespace noise | `save_checkpoint` + `compare_checkpoint` | Crystal-clear semantic diff |
| **Cross-Service** | Manual file-by-file search | `propagation_checklist` — Proto → Rust → TS | Prevents missing propagation |
| **Blast Radius** | Guessing | `call_hierarchy` — Incoming + Outgoing callers | Safe rename / delete |

---

## 🛠️ MCP Tool Reference (v1.5.0)

### 🗺️ `map_repo` — God's Eye
Returns a hierarchical codebase map showing files and their exported symbols.

- `search_filter` — case-insensitive substring, **OR via `|`** (e.g. `"auth|user"`); matches file paths and, for repos ≤ 300 files, symbol names too
- `ignore_gitignore` — set `true` to include generated / git-ignored files
- `max_chars` — output cap (default 8 000 chars)
- Built-in guardrails: did-you-mean path recovery, regex-input warning, overflow diagnostics

### ⚡ `read_symbol` — X-Ray Extractor
Extracts the exact, full source of any symbol (function, struct, class, const) via AST.

- `symbol_names: ["A","B","C"]` — batch mode, multiple symbols in one call
- "Symbol not found" error: lists up to 30 available symbols + recovery hint pointing to `find_usages` / `map_repo`

### 🎯 `find_usages` — Semantic Tracer
**Always use instead of `grep` / `rg`.** 100 % accurate AST usages across the workspace, zero false positives from comments or strings. Categorises hits:
- **Calls** — function / method invocations
- **TypeRefs** — type annotations, generics
- **FieldInits** — struct field assignments

### 🕸️ `call_hierarchy` — Blast Radius Analyser
**Use before any function rename, move, or delete.** Shows who calls the function (Incoming) and what the function calls (Outgoing).

### 📦 `get_context_slice` — Deep Dive Slicer
Token-budget-aware XML slice of a directory or file. Skeletonises all source (bodies pruned, imports collapsed).

- `query` — optional semantic vector search; ranks files by relevance first
- **Inline / spill**: output ≤ 8 KB returned inline; larger output written to `/tmp/cortexast_slice_{hash}.xml` — use `read_file` to access it

### 🚨 `run_diagnostics` — Compiler Whisperer
Auto-detects project type (`cargo check` / `tsc --noEmit`), runs the compiler, maps errors directly to AST source lines.

### ⏳ Chronos — AST Time Machine
Save structural snapshots before edits and compare semantics after — without whitespace or line-number noise.

- `save_checkpoint` — **Use before any non-trivial edit or refactor.** Snapshots a symbol's AST to disk with a semantic tag (e.g. `pre-refactor`)
- `list_checkpoints` — shows all saved snapshots grouped by tag
- `compare_checkpoint` — structural diff between two snapshots; ignores whitespace and line-number noise

### 🎯 `propagation_checklist` — Cross-Boundary Safety Net
**Use before changing any shared type, struct, interface, or API contract.**  
Generates a strict Markdown checklist grouped by language / domain (Proto → Rust → TS → Python → Other).

- `symbol_name` — AST-traces the symbol across the entire workspace
- `ignore_gitignore: true` — includes generated stubs (gRPC, Protobuf, etc.)
- Line numbers per file (up to 5 shown, `…` suffix if more)
- Hard cap: 50 files, 8 000 chars; BLAST RADIUS WARNING if exceeded
- `changed_path` — legacy file-based mode (still supported)

---

## 🏆 Benchmarks

**Target:** CortexAST source (10+ Rust files, core logic)  
**Hardware:** Apple M4 Pro / 14 CPU · 20 GPU · 24 GB RAM

| Metric | Raw Copy-Paste | 🧠 CortexAST |
|:---|:---|:---|
| **Total Size** | 127 536 bytes | **9 842 bytes — 92.3 % smaller** |
| **Est. Token Cost** | ~31 884 tokens | **~2 460 tokens** |
| **Processing Time** | N/A | **< 0.1 s (Pure Rust)** |
| **Information Density** | Low (noise-heavy) | **High (pure logic)** |

---

## 🏗️ Core Architecture

- **Nuclear Skeletonisation** — function bodies collapse to signatures, imports stripped, indentation flattened
- **JIT Hybrid Vector Search** — `model2vec-rs` (pure Rust, < 100 MB RAM); `xxh3` content hashing; incremental updates on-demand only
- **Enterprise Workspace Engine** — auto-discovers nested microservices (`Cargo.toml`, `package.json`, `pyproject.toml`) and routes token budgets across monorepos
- **Bulletproof Safety** — null-byte detection, 1 MB file cap, minified-bundle guard, UTF-8 lossy fallback, index auto-repair

---
## 📦 Installation

### Option A — Pre-built Binary

Download from [Releases](https://github.com/DevsHero/CortexAST/releases/latest):

| Platform | File |
|---|---|
| macOS Apple Silicon | `cortexast-macos-aarch64` |
| macOS Intel | `cortexast-macos-x86_64` |
| Linux x86_64 | `cortexast-linux-x86_64` |
| Linux ARM64 | `cortexast-linux-aarch64` |
| Windows x86_64 | `cortexast-windows-x86_64.exe` |

```bash
chmod +x cortexast-macos-aarch64
./cortexast-macos-aarch64 --help
```

### Option B — Build from Source

```bash
git clone https://github.com/DevsHero/CortexAST.git
cd CortexAST
cargo build --release
# Binary: ./target/release/cortexast
```

See [docs/BUILDING.md](docs/BUILDING.md) for cross-compilation instructions.

---

## 🔌 MCP Setup

Add to your MCP client config (Claude Desktop / VS Code / Cursor / Cline / Windsurf):

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

See [docs/MCP_SETUP.md](docs/MCP_SETUP.md) for per-client setup instructions.

---

## � Recommended Agent Rules

To maximise CortexAST's effectiveness, add the rules below to your AI assistant's instruction file. This ensures the agent always prefers CortexAST tools over basic shell commands and follows the correct workflow to minimise hallucination and token waste.

### VS Code — GitHub Copilot

**File:** `.github/copilot-instructions.md`

```markdown
## CortexAST Priority Rules

- ALWAYS use `map_repo` instead of ls/tree/find
- ALWAYS use `read_symbol` instead of cat/head/any file read
- ALWAYS use `find_usages` instead of grep/rg/ag/semantic_search
- ALWAYS call `save_checkpoint` before any non-trivial edit or refactor
- ALWAYS call `propagation_checklist` before changing a shared type, struct, or interface
- Use `get_context_slice` with a `query` for multi-file overviews (output may spill to file — read it)
- Use `call_hierarchy` to measure blast radius before renaming, moving, or deleting any function
```

### Cursor

**File:** `.cursor/rules/cortexast.mdc` (or `.cursorrules`)

```markdown
---
description: CortexAST tool priority rules for AI agents
globs: ["**/*"]
alwaysApply: true
---

## CortexAST Priority Rules

- ALWAYS use `map_repo` instead of ls/tree/find
- ALWAYS use `read_symbol` instead of cat/head/any file read
- ALWAYS use `find_usages` instead of grep/rg/ag
- ALWAYS call `save_checkpoint` before any non-trivial edit or refactor
- ALWAYS call `propagation_checklist` before changing a shared type, struct, or interface
- Use `get_context_slice` with a `query` for multi-file overviews (output may spill to file — read it)
- Use `call_hierarchy` to measure blast radius before renaming, moving, or deleting any function
```

### Windsurf

**File:** `.windsurfrules`

```markdown
## CortexAST Priority Rules

- ALWAYS use `map_repo` instead of ls/tree/find
- ALWAYS use `read_symbol` instead of cat/head/any file read
- ALWAYS use `find_usages` instead of grep/rg/ag
- ALWAYS call `save_checkpoint` before any non-trivial edit or refactor
- ALWAYS call `propagation_checklist` before changing a shared type, struct, or interface
- Use `get_context_slice` with a `query` for multi-file overviews (output may spill to file — read it)
- Use `call_hierarchy` to measure blast radius before renaming, moving, or deleting any function
```

### Cline / Claude Dev

**File:** `.clinerules`

```markdown
## CortexAST Priority Rules

- ALWAYS use `map_repo` instead of ls/tree/find
- ALWAYS use `read_symbol` instead of cat/head/any file read
- ALWAYS use `find_usages` instead of grep/rg/ag
- ALWAYS call `save_checkpoint` before any non-trivial edit or refactor
- ALWAYS call `propagation_checklist` before changing a shared type, struct, or interface
- Use `get_context_slice` with a `query` for multi-file overviews (output may spill to file — read it)
- Use `call_hierarchy` to measure blast radius before renaming, moving, or deleting any function
```

### Claude Desktop

Add to `claude_desktop_config.json` → `systemPrompt`:

```
CortexAST Priority Rules:
- ALWAYS use map_repo instead of ls/tree/find
- ALWAYS use read_symbol instead of cat/head/any file read
- ALWAYS use find_usages instead of grep/rg/ag
- ALWAYS call save_checkpoint before any non-trivial edit or refactor
- ALWAYS call propagation_checklist before changing a shared type, struct, or interface
- Use get_context_slice with a query for multi-file overviews (output may spill to file — read it)
- Use call_hierarchy to measure blast radius before renaming, moving, or deleting any function
```

---

## 🤝 Contributing

PRs welcome.

- Core: Rust (Tokio, Rayon, Model2Vec, Tree-sitter)
- Focus: performance, compression ratio, multi-language correctness

See [CHANGELOG.md](CHANGELOG.md) for version history.

Crafted with 🦀 by [DevsHero](https://github.com/DevsHero).