# 🧠 CortexAST

**The AI-Native Code Intelligence Backend. Extract the Signal, Discard the Noise.**  
_Giving LLM agents deterministic, AST-level understanding of any codebase — at nuclear token efficiency._

[![CortexAST](https://img.shields.io/badge/Repo-CortexAST-blue?style=for-the-badge&logo=github)](https://github.com/cortex-works/cortex-ast)
[![CortexSync](https://img.shields.io/badge/Repo-CortexSync-orange?style=for-the-badge&logo=github)](https://github.com/cortex-works/cortex-sync)

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)
[![MCP Ready](https://img.shields.io/badge/MCP-Ready-blue)](https://modelcontextprotocol.io/)
[![Version](https://img.shields.io/badge/version-2.0.4-green)](CHANGELOG.md)

---

## ⚡ Why CortexAST?

Most AI coding agents rely on tools built for *human eyeballs* — `cat`, `grep`, `tree`, `git diff`. For an LLM these are toxic: they flood the context window with whitespace, comments, full file dumps, and force the agent into "amnesia" from pagination.

**CortexAST is a sensory system built strictly for AI brains.**  
Powered by [Tree-sitter](https://tree-sitter.github.io/) and written in pure Rust, it gives agents a deterministic, high-fidelity understanding of entire codebases — cutting token usage by up to **90%** while preserving 100% of the architectural logic.

### 🛡️ The Reader/Writer Symbiosis
CortexAST (The Reader) works hand-in-hand with **[CortexSync](https://github.com/cortex-works/cortex-sync)** (The Writer). CortexSync embeds your real-time agent telemetry into a journal that CortexAST then retrieves via `cortex_memory_retriever`, providing **Persistent Cross-Session Memory.**

---

## 🚀 Key Features

### 1. Kill Massive `.cursorrules` (Micro-Bootloader)
Tired of hit-and-miss results with a 5,000-line `.cursorrules` file? Use the **Micro-Bootloader** approach:
- Keep a **tiny instruction rule** telling the agent to use CortexAST.
- **Dynamic Retrieval:** The agent dynamically pulls documentation, history, and rules by calling CortexAST tools like `cortex_memory_retriever` as needed.
- **Result:** Drastic token savings and much higher completion accuracy.

### 2. 3-Tier Rule Engine
Manage enterprise-grade AI coding rules across three distinct levels:
- **Global Tier (`~/.cortexast/global_rules.yml`):** Rules that apply to *you* across all projects.
- **Team/Cluster Tier (`~/.cortexast/cluster/`):** Rules synced across your internal network for team-wide standards.
- **Project Tier (`.cortexast.json`):** Overrides and custom logic for the specific codebase you’re working on.

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

## 🛠️ MCP Tool Reference (v2.0.4 — Megatool API)

> **Megatool API:** 10 standalone tools consolidated into 4 megatools with `action` enum routing. Old tool names are accepted as compatibility shims but deprecated. Use the new API below.

### 🔍 `cortex_code_explorer` — Code Explorer Megatool
🔥 Always use instead of ls/tree/find/cat. Two modes via `action`:

**`action: map_overview`** — Bird's-eye repo map (files + public symbols). Use first on any unfamiliar codebase.
- `target_dir` (**required**) — directory to map (use `'.'` for whole repo)
- `search_filter` — case-insensitive substring, **OR via `|`** (e.g. `"auth|user"`)
- `ignore_gitignore` — set `true` to include generated / git-ignored files
- `max_chars` — optional output cap (default **8 000**; raise up to ~**30 000** only if your client can safely handle larger inline outputs)

**`action: deep_slice`** — Token-budget-aware XML slice of a file or directory.
- `target` (**required**) — relative path to file or directory
- `query` — optional semantic vector search; ranks files by relevance first
- `budget_tokens` — token budget (default 32 000)
- `skeleton_only: true` — enforce structural pruning (skeleton output only) regardless of repo config
- Output safety: server enforces a strict inline limit via `max_chars` (default **8 000**) and **truncates inline** to avoid editor-side "spill" behaviors

### 🎯 `cortex_symbol_analyzer` — Symbol Analysis Megatool
🔥 Always use instead of grep/rg/ag. Modes via `action`:

**`action: read_source`** — Extracts the exact, full source of any symbol (function, struct, class, const) via AST.
- `path` (**required**) — source file containing the symbol
- `symbol_name` (**required unless using `symbol_names`**) — target symbol name
- `symbol_names: ["A","B","C"]` — batch mode: multiple symbols in one call (ignores `symbol_name`)
- `skeleton_only: true` — return signatures/structure only (drastically reduces tokens when you only need the API)
- `max_chars` — optional output cap (default **8 000**)

**`action: find_usages`** — 100% accurate AST usages, zero false positives from comments or strings. Categorises: **Calls** / **TypeRefs** / **FieldAccesses** / **FieldInits**.
- `symbol_name` + `target_dir` (**required**)

**`action: find_implementations`** — Finds structs/classes that implement a given trait/interface (Rust `impl Trait for Type`, TS `class X implements Y`).
- `symbol_name` + `target_dir` (**required**)

**`action: blast_radius`** — Shows who calls the function (Incoming) and what the function calls (Outgoing). **Use before any rename, move, or delete.**
- `symbol_name` + `target_dir` (**required**)

**`action: propagation_checklist`** — Strict Markdown checklist grouped by language/domain (Proto → Rust → TS → Python). **Use before changing any shared type, interface, or API contract.**
- `symbol_name` (**required**); `changed_path` for legacy contract-file mode
- `ignore_gitignore: true` — includes generated stubs (gRPC, Protobuf, etc.)
- `aliases: ["otherName"]` — cross-boundary rename bridges; casing variants are auto-generated

### ⏳ `cortex_chronos` — Snapshot Megatool (AST Time Machine)
⚖️ **NEVER use `git diff` for AI refactors.** Three modes via `action`:

**`action: save_checkpoint`** — Snapshots a symbol's AST under a semantic tag (e.g. `pre-refactor`). **Call before any non-trivial edit.**
- `path` + `symbol_name` + `semantic_tag` (**required**)

**`action: list_checkpoints`** — Lists all saved snapshots grouped by tag.

**`action: compare_checkpoint`** — Structural diff between two snapshots; ignores whitespace and line-number noise.
- `symbol_name` + `tag_a` + `tag_b` (**required**)
- Magic: set `tag_b="__live__"` to compare `tag_a` against the current filesystem state (**requires `path`**)

**`action: delete_checkpoint`** — Deletes checkpoint files from the local store (housekeeping).
- Provide at least one filter: `symbol_name`, `semantic_tag`, or `path`.
- **Legacy Fallback:** If a filtered delete finds zero matches in the namespace-specific directory, it automatically searches the legacy flat `checkpoints/` directory to ensure backward compatibility.
- **Namespace Purge:** Omit all filters to purge an entire namespace. If the namespace doesn't exist, the tool returns a self-teaching error to prevent confusing tags with namespaces.

### 🚨 `run_diagnostics` — Compiler Whisperer
Auto-detects project type (`cargo check` / `tsc --noEmit`), runs the compiler, maps errors directly to AST source lines. **Run immediately after any code edit.**

### 🔒 Omni-Root Resolver & OS Safeguards
CortexAST uses a multi-stage priority chain to resolve the `repo_root`. The **MCP `initialize` request is the authoritative, protocol-level source** — it overwrites any earlier env-var bootstrap and is the only approach that works reliably across all IDEs.

Full resolution order (first non-dead value wins):
1. `repoPath` param in the tool call — per-call override
2. **MCP `initialize`** (`rootUri` / `rootPath` / `workspaceFolders[0].uri`) — protocol-level, canonical
3. `--root` CLI flag / `CORTEXAST_ROOT` env var — startup bootstrap
4. IDE env vars: `VSCODE_WORKSPACE_FOLDER`, `VSCODE_CWD`, `IDEA_INITIAL_DIRECTORY`, `PWD`/`INIT_CWD` (if ≠ `$HOME`) — checked at startup AND inside every tool call (belt-and-suspenders)
5. Find-up heuristic on the tool's own `path` / `target_dir` / `target` arg — walks ancestors for `.git`, `Cargo.toml`, `package.json`
6. `cwd` — **refused if it equals `$HOME` or OS root** → returns a **CRITICAL error** the LLM can act on

### 🔒 Output safety (`max_chars`)
All megatools accept an optional `max_chars` (default **8 000**). The server will **truncate inline** and append an explicit marker when the limit is hit — this prevents VS Code/Cursor-style interception that writes large tool outputs into workspace storage.


---

## 🧭 AI‑Native Workflow (Megatools on Rails)

Megatools exist to prevent **LLM decision fatigue** (too many tools to choose from) and **token bloat** (too much schema + too much output). Instead of exposing a long list of tiny tools, CortexAST exposes 4 Megatools with an `action` enum so the agent makes one decision at a time.

For any non-trivial refactor (rename/move/delete, signature change, or cross-module update), follow this sequence:

Explore (`cortex_code_explorer(action: map_overview)`) ➔
Isolate (`cortex_symbol_analyzer(action: read_source)`) ➔
Measure Impact (`find_usages` / `blast_radius`) ➔
Checkpoint (`cortex_chronos(action: save_checkpoint)`) ➔
Edit Code ➔
Verify (`run_diagnostics` + `cortex_chronos(action: compare_checkpoint)`) ➔
Cross‑Sync (`cortex_symbol_analyzer(action: propagation_checklist)`).


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

Download from [Releases](https://github.com/cortex-works/cortex-ast/releases/latest):

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
git clone https://github.com/cortex-works/cortex-ast.git
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

## ✅ Recommended Agent Rules

To maximise CortexAST's effectiveness, add the rules below to your AI assistant's instruction file. This ensures the agent always prefers CortexAST tools over basic shell commands and follows the correct workflow to minimise hallucination and token waste.

### VS Code — GitHub Copilot

**File:** `.github/copilot-instructions.md`

```markdown
## CortexAST Priority Rules (Megatool API v2.0+)

- 🔍 Explore repos/files → `cortex_code_explorer(action: map_overview)` or `(action: deep_slice)`. NEVER use ls/tree/find/cat.
- 🎯 Look up a symbol → `cortex_symbol_analyzer(action: read_source)`. NEVER use grep/rg.
- 🎯 Prefer `skeleton_only: true` for large symbols when you only need the API/signature surface.
- 🎯 Find all usages → `cortex_symbol_analyzer(action: find_usages)` before changing any symbol signature.
- 🎯 Find implementors → `cortex_symbol_analyzer(action: find_implementations)` when changing traits/interfaces.
- 🎯 Measure blast radius → `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename, move, or delete.
- 🎯 Cross-boundary propagation → `cortex_symbol_analyzer(action: propagation_checklist)` before changing a shared type/struct/interface.
- ⏳ Save a snapshot → `cortex_chronos(action: save_checkpoint)` BEFORE any non-trivial refactor. Use `namespace` to group checkpoints by session (e.g. `namespace='qa-run-1'`).
- ⏳ Verify a refactor → `cortex_chronos(action: compare_checkpoint)` AFTER editing (NEVER use git diff for this). Tip: `tag_b='__live__'` compares against the current filesystem state.
- ⏳ Clean up QC checkpoints → `cortex_chronos(action: delete_checkpoint, namespace='qa-run-1')` — omit `symbol_name`/`semantic_tag` to purge the entire namespace.
- 🚨 Check for errors → `run_diagnostics` immediately after any code edit.
- 🔒 `max_chars` (default **8000**) — the server truncates at this limit. VS Code Copilot spills responses larger than ~8 KB to workspace storage, so the 8000 default keeps output inline. Increase only if your client handles larger inline responses.
```

### Cursor

**File:** `.cursor/rules/cortexast.mdc` (or `.cursorrules`)

```markdown
---
description: CortexAST tool priority rules for AI agents
globs: ["**/*"]
alwaysApply: true
---

## CortexAST Priority Rules (Megatool API v2.0.4+)

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
## CortexAST Priority Rules (Megatool API v2.0.4+)

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
## CortexAST Priority Rules (Megatool API v2.0.4+)

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
CortexAST Priority Rules (Megatool API v2.0.4+):
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

Crafted with 🦀 by [cortex-works](https://github.com/cortex-works).