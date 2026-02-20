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
| **Exploration** | `tree` / `ls` — filenames only | `cortex_code_explorer(map_overview)` — files + public symbols inside | Instant architecture map |
| **Reading Code** | `cat` — 2 000-line dump | `cortex_symbol_analyzer(read_source)` — exact AST node only | Nuclear token savings |
| **Finding Stuff** | `grep` — string matches incl. comments | `cortex_symbol_analyzer(find_usages)` — AST-accurate, zero false positives | Calls / Type Refs / Fields |
| **Refactoring** | `git diff` — line & whitespace noise | `cortex_chronos(save_checkpoint)` + `cortex_chronos(compare_checkpoint)` | Crystal-clear semantic diff |
| **Cross-Service** | Manual file-by-file search | `cortex_symbol_analyzer(propagation_checklist)` — Proto → Rust → TS | Prevents missing propagation |
| **Blast Radius** | Guessing | `cortex_symbol_analyzer(blast_radius)` — Incoming + Outgoing callers | Safe rename / delete |

---

## 🛠️ MCP Tool Reference (v1.5.0 — Megatool API)

> **Megatool API:** 10 standalone tools consolidated into 4 megatools with `action` enum routing. Old tool names are accepted as compatibility shims but deprecated. Use the new API below.

### 🔍 `cortex_code_explorer` — Code Explorer Megatool
🔥 Always use instead of ls/tree/find/cat. Two modes via `action`:

**`action: map_overview`** — Bird's-eye repo map (files + public symbols). Use first on any unfamiliar codebase.
- `target_dir` (**required**) — directory to map (use `'.'` for whole repo)
- `search_filter` — case-insensitive substring, **OR via `|`** (e.g. `"auth|user"`)
- `ignore_gitignore` — set `true` to include generated / git-ignored files
- `max_chars` — output cap (hard cap 8 000 chars)

**`action: deep_slice`** — Token-budget-aware XML slice of a file or directory. Skeletonises all source (bodies pruned, imports collapsed).
- `target` (**required**) — relative path to file or directory
- `query` — optional semantic vector search; ranks files by relevance first
- `budget_tokens` — token budget (default 32 000)
- **Inline / spill**: output ≤ 8 KB returned inline; larger output written to temp file — use `read_file` to access it

### 🎯 `cortex_symbol_analyzer` — Symbol Analysis Megatool
🔥 Always use instead of grep/rg/ag. Four modes via `action`:

**`action: read_source`** — Extracts the exact, full source of any symbol (function, struct, class, const) via AST.
- `path` (**required**) — source file containing the symbol
- `symbol_name` (**required**) — target symbol name
- `symbol_names: ["A","B","C"]` — batch mode: multiple symbols in one call

**`action: find_usages`** — 100% accurate AST usages, zero false positives from comments or strings. Categorises: **Calls** / **TypeRefs** / **FieldInits**.
- `symbol_name` + `target_dir` (**required**)

**`action: blast_radius`** — Shows who calls the function (Incoming) and what the function calls (Outgoing). **Use before any rename, move, or delete.**
- `symbol_name` + `target_dir` (**required**)

**`action: propagation_checklist`** — Strict Markdown checklist grouped by language/domain (Proto → Rust → TS → Python). **Use before changing any shared type, interface, or API contract.**
- `symbol_name` (**required**); `changed_path` for legacy contract-file mode
- `ignore_gitignore: true` — includes generated stubs (gRPC, Protobuf, etc.)

### ⏳ `cortex_chronos` — Snapshot Megatool (AST Time Machine)
⚖️ **NEVER use `git diff` for AI refactors.** Three modes via `action`:

**`action: save_checkpoint`** — Snapshots a symbol's AST under a semantic tag (e.g. `pre-refactor`). **Call before any non-trivial edit.**
- `path` + `symbol_name` + `semantic_tag` (**required**)

**`action: list_checkpoints`** — Lists all saved snapshots grouped by tag.

**`action: compare_checkpoint`** — Structural diff between two snapshots; ignores whitespace and line-number noise.
- `symbol_name` + `tag_a` + `tag_b` (**required**)

### 🚨 `run_diagnostics` — Compiler Whisperer
Auto-detects project type (`cargo check` / `tsc --noEmit`), runs the compiler, maps errors directly to AST source lines. **Run immediately after any code edit.**


---

## 🏗️ Core Architecture

- **Nuclear Skeletonisation** — function bodies collapse to signatures, imports stripped, indentation flattened
- **JIT Hybrid Vector Search** — `model2vec-rs` (pure Rust, < 100 MB RAM); `xxh3` content hashing; incremental updates on-demand only
- **Enterprise Workspace Engine** — auto-discovers nested microservices (`Cargo.toml`, `package.json`, `pyproject.toml`) and routes token budgets across monorepos
- **Bulletproof Safety** — null-byte detection, 1 MB file cap, minified-bundle guard, UTF-8 lossy fallback, index auto-repair

---

## 🤖 Agentic Workflow Playbook

Want to see what CortexAST looks like when an agent runs it like a Senior Architect?
Read: [USE_CASES.md](USE_CASES.md)

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
## CortexAST Priority Rules (Megatool API v1.5+)

- 🔍 Explore repos/files → `cortex_code_explorer(action: map_overview)` or `(action: deep_slice)`. NEVER use ls/tree/find/cat.
- 🎯 Look up a symbol → `cortex_symbol_analyzer(action: read_source)`. NEVER use grep/rg.
- 🎯 Find all usages → `cortex_symbol_analyzer(action: find_usages)` before changing any symbol signature.
- 🎯 Measure blast radius → `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename, move, or delete.
- 🎯 Cross-boundary propagation → `cortex_symbol_analyzer(action: propagation_checklist)` before changing a shared type/struct/interface.
- ⏳ Save a snapshot → `cortex_chronos(action: save_checkpoint)` BEFORE any non-trivial refactor.
- ⏳ Verify a refactor → `cortex_chronos(action: compare_checkpoint)` AFTER editing (NEVER use git diff for this).
- 🚨 Check for errors → `run_diagnostics` immediately after any code edit.
```

### Cursor

**File:** `.cursor/rules/cortexast.mdc` (or `.cursorrules`)

```markdown
---
description: CortexAST tool priority rules for AI agents
globs: ["**/*"]
alwaysApply: true
---

## CortexAST Priority Rules (Megatool API v1.5+)

- 🔍 Explore repos/files → `cortex_code_explorer(action: map_overview)` or `(action: deep_slice)`. NEVER use ls/tree/find/cat.
- 🎯 Look up a symbol → `cortex_symbol_analyzer(action: read_source)`. NEVER use grep/rg.
- 🎯 Find all usages → `cortex_symbol_analyzer(action: find_usages)` before changing any symbol signature.
- 🎯 Measure blast radius → `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename, move, or delete.
- 🎯 Cross-boundary propagation → `cortex_symbol_analyzer(action: propagation_checklist)` before changing a shared type/struct/interface.
- ⏳ Save a snapshot → `cortex_chronos(action: save_checkpoint)` BEFORE any non-trivial refactor.
- ⏳ Verify a refactor → `cortex_chronos(action: compare_checkpoint)` AFTER editing (NEVER use git diff for this).
- 🚨 Check for errors → `run_diagnostics` immediately after any code edit.
```

### Windsurf

**File:** `.windsurfrules`

```markdown
## CortexAST Priority Rules (Megatool API v1.5+)

- 🔍 Explore repos/files → `cortex_code_explorer(action: map_overview)` or `(action: deep_slice)`. NEVER use ls/tree/find/cat.
- 🎯 Look up a symbol → `cortex_symbol_analyzer(action: read_source)`. NEVER use grep/rg.
- 🎯 Find all usages → `cortex_symbol_analyzer(action: find_usages)` before changing any symbol signature.
- 🎯 Measure blast radius → `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename, move, or delete.
- 🎯 Cross-boundary propagation → `cortex_symbol_analyzer(action: propagation_checklist)` before changing a shared type/struct/interface.
- ⏳ Save a snapshot → `cortex_chronos(action: save_checkpoint)` BEFORE any non-trivial refactor.
- ⏳ Verify a refactor → `cortex_chronos(action: compare_checkpoint)` AFTER editing (NEVER use git diff for this).
- 🚨 Check for errors → `run_diagnostics` immediately after any code edit.
```

### Cline / Claude Dev

**File:** `.clinerules`

```markdown
## CortexAST Priority Rules (Megatool API v1.5+)

- 🔍 Explore repos/files → `cortex_code_explorer(action: map_overview)` or `(action: deep_slice)`. NEVER use ls/tree/find/cat.
- 🎯 Look up a symbol → `cortex_symbol_analyzer(action: read_source)`. NEVER use grep/rg.
- 🎯 Find all usages → `cortex_symbol_analyzer(action: find_usages)` before changing any symbol signature.
- 🎯 Measure blast radius → `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename, move, or delete.
- 🎯 Cross-boundary propagation → `cortex_symbol_analyzer(action: propagation_checklist)` before changing a shared type/struct/interface.
- ⏳ Save a snapshot → `cortex_chronos(action: save_checkpoint)` BEFORE any non-trivial refactor.
- ⏳ Verify a refactor → `cortex_chronos(action: compare_checkpoint)` AFTER editing (NEVER use git diff for this).
- 🚨 Check for errors → `run_diagnostics` immediately after any code edit.
```

### Claude Desktop

Add to `claude_desktop_config.json` → `systemPrompt`:

```
CortexAST Priority Rules (Megatool API v1.5+):
- Explore repos/files → cortex_code_explorer with action: map_overview (structure) or action: deep_slice (content). NEVER use ls/tree/find/cat.
- Look up a symbol → cortex_symbol_analyzer with action: read_source. NEVER use grep/rg.
- Find all usages → cortex_symbol_analyzer with action: find_usages before changing any symbol signature.
- Measure blast radius → cortex_symbol_analyzer with action: blast_radius BEFORE any rename, move, or delete.
- Cross-boundary propagation → cortex_symbol_analyzer with action: propagation_checklist before changing a shared type/struct.
- Save a snapshot → cortex_chronos with action: save_checkpoint BEFORE any non-trivial refactor.
- Verify a refactor → cortex_chronos with action: compare_checkpoint AFTER editing (NEVER use git diff for this).
- Check for errors → run_diagnostics immediately after any code edit.
```

---

## 🤝 Contributing

PRs welcome.

- Core: Rust (Tokio, Rayon, Model2Vec, Tree-sitter)
- Focus: performance, compression ratio, multi-language correctness

See [CHANGELOG.md](CHANGELOG.md) for version history.

Crafted with 🦀 by [DevsHero](https://github.com/DevsHero).