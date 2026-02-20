# Changelog

All notable changes to **CortexAST** are documented here.  
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

---

## [Unreleased]

### Added
- `cortex_chronos(compare_checkpoint)` supports `tag_b="__live__"` to compare a saved snapshot (`tag_a`) against the current filesystem state (requires `path`).
- `cortex_symbol_analyzer(action=find_implementations)` to locate Rust/TypeScript implementors of a trait/interface.
- `cortex_code_explorer(action=deep_slice)` supports `skeleton_only: true` to enforce structural pruning regardless of repo config.

### Changed
- Output handling: megatool responses are now negotiated via `max_chars` (default 15 000, max 30 000) and truncated inline with an explicit marker to prevent editor-side spill/interception.
- `cortex_symbol_analyzer(action=read_source)` supports `skeleton_only: true` to return structural signatures only (token-saving mode).

### Fixed
- Chronos checkpoint path matching is normalized to repo-relative keys (consistent across save/compare/delete), reducing "No checkpoint found" mismatches caused by absolute vs relative path variants.

## [2.0.0] — Megatool API

### Breaking Changes (with shims)
- **10 standalone MCP tools consolidated into 4 Megatools** using `action` enum routing.
  Old tool names are still accepted as compatibility shims via the router but are deprecated.
  All new integrations should use the new API.

  | Old Tool Name | New API |
  |---|---|
  | `map_repo` | `cortex_code_explorer` + `action: map_overview` |
  | `get_context_slice` | `cortex_code_explorer` + `action: deep_slice` |
  | `read_symbol` | `cortex_symbol_analyzer` + `action: read_source` |
  | `find_usages` | `cortex_symbol_analyzer` + `action: find_usages` |
  | `call_hierarchy` | `cortex_symbol_analyzer` + `action: blast_radius` |
  | `propagation_checklist` | `cortex_symbol_analyzer` + `action: propagation_checklist` |
  | `save_checkpoint` | `cortex_chronos` + `action: save_checkpoint` |
  | `list_checkpoints` | `cortex_chronos` + `action: list_checkpoints` |
  | `compare_checkpoint` | `cortex_chronos` + `action: compare_checkpoint` |
  | `run_diagnostics` | `run_diagnostics` (unchanged) |

### Added
- **Production-quality tool descriptions** — all 4 megatools now carry Anthropic-best-practice-compliant descriptions with:
  - DECISION GUIDE blocks mapping user intent → action choice
  - Per-action trigger phrases (when to use, when NOT to use)
  - Required vs optional param callouts inline in enum descriptions
  - Negative anchors ("NEVER use grep/rg when this tool is available")
  - Aim for 5–8 sentences per tool, following Anthropic guideline of ≥3–4 sentences for complex tools
- **`USE_CASES.md`** — Agentic Workflow Playbook with 3 killer use cases, Mermaid flowcharts, and agent execution logs
- **README.md Agentic Workflow Playbook section** — links to USE_CASES.md
- **`.cortexast/` added to `.gitignore`** — prevents Chronos snapshot files from being committed

### Changed
- **Recommended agent rules updated** across all 5 client templates (VS Code, Cursor, Windsurf, Cline, Claude Desktop) to reference new megatool + action syntax
- **MCP Tool Reference in README.md** rewritten for megatool API with per-action parameter documentation

---

## [1.5.0] — 2026-02-20

### Added
- **`get_context_slice` inline/spill logic** — output ≤ 8 KB is returned inline (zero agent round-trip); larger output is written to `/tmp/cortexast_slice_{hash}.xml` with a `read_file` hint so agents never get a context window flood
- **`propagation_checklist` blast-radius guardrails** — hard cap of 50 files and 8 000 chars per call; overflow generates a `BLAST RADIUS WARNING` line with remaining count; deterministic sort by domain/path
- **`propagation_checklist` `ignore_gitignore` + line numbers** — `ignore_gitignore: true` bypasses `.gitignore` so generated/stubbed files (e.g. gRPC stubs) are included; AST-extracted line numbers shown per file (max 5, `…` suffix)
- **`propagation_checklist` symbol mode** — pass `symbol_name` for cross-boundary AST tracing grouped by language/domain (Proto → Rust → TS → Python → Other); legacy `changed_path` mode preserved
- **`map_repo` guardrails** — did-you-mean path recovery (`!target_dir.exists()` → lists repo-root top-level entries ≤ 30), regex-ish input warning banner, schema tip descriptions
- **`map_repo` `search_filter` OR support** — tokenises on `|`, per-token substring matching; symbol-aware fallback for repos ≤ 300 files
- **`read_symbol` DX** — "Symbol not found" error caps available symbol list at 30 (overflow line + count), appends recovery hint pointing to `find_usages` / `map_repo`

### Changed
- **Tool descriptions hardened** (addresses QA "Why underused" root causes):
  - `call_hierarchy` — trigger text changed to "USE BEFORE ANY FUNCTION RENAME, MOVE, OR DELETE"
  - `save_checkpoint` — "USE THIS before any non-trivial edit or refactor"
  - `propagation_checklist` — "Also USE THIS before changing any shared type, struct, or interface — strictly better than manually searching usages file-by-file"
  - `get_context_slice` — documents inline vs spill behaviour in schema description

### Fixed (chore)
- Renamed all remaining legacy `neurosiphon` identifiers in source code to `cortexast`:
  - `#[command(name)]` in `main.rs`
  - XML root element tag in `xml_builder.rs`
  - Debug log prefixes in `vector_store.rs` and `scanner.rs`
- Updated `docs/BUILDING.md` to reflect `cortexast` binary name and current repository URL

---

## [1.4.1] — 2026-02-17

### Added
- `map_repo` dropped-file diagnostics — shows which files matched the filter, which were dropped, and why
- Strict summary-first threshold (`STRICT_SUMMARY_THRESHOLD = 50`) — repos above this size always emit a summary header before per-file detail
- Hard 8 000-char output cap on `map_repo` with UTF-8-safe truncation marker

### Changed
- `map_repo` progressive disclosure output format — one-liner skeleton for small files, full symbol list for large ones
- Improved 0-file diagnostics: lists supported extensions and worked example

---

## [1.4.0] — 2026-02-15

### Added
- **Rebrand**: NeuroSiphon → **CortexAST** v1.4.0; all tool names shortened (`cortex_map` → `map_repo`, etc.)
- **Chronos AST Time Machine**: `save_checkpoint`, `list_checkpoints`, `compare_checkpoint` — disk-backed semantic symbol snapshots; structural diff ignores whitespace/line-number noise
- **`propagation_checklist`** (initial): given a changed file path, generates a cross-language propagation checklist
- **`find_usages`** categories: Calls / Type References / Field Initializations
- **`read_symbol`** batch mode via `symbol_names: [...]`
- `.proto` file support in `map_repo`, `read_symbol`, `find_usages` via ProtoDriver
- `const`, `static`, `type` alias support for Rust, TypeScript, Go extractions

### Changed
- Aggressive prompt-injection descriptions to steer agent tool preference
- `map_repo` `search_filter` parameter added

---

## [1.3.1] — 2026-02-10

### Fixed
- Show all Rust symbols (not just first pass)
- Better Python attribute call detection in `call_hierarchy`
- Reduce outgoing-call noise for push_str / trivial intrinsics

### Changed
- Default embedding model updated for better semantic recall
- 2-stage hybrid router with deterministic exact-match ranking ("Symbol Sniper")

---

## [1.3.0] — 2026-02-05

### Added
- `repo_map`, `call_hierarchy`, `run_diagnostics` MCP tools
- Auto-detect project type for `run_diagnostics` (`cargo check` / `tsc --noEmit`)
- Compiler errors mapped back to 1-line AST source context

---

## [1.2.0] — 2026-01-28

### Added
- **AST X-Ray** (`read_symbol`) and **`find_usages`** tracer tools
- v2 vector index with `xxh3` content hashing for JIT incremental updates

---

## [1.1.0] — 2026-01-20

### Added
- Monorepo / nested workspace support — auto-discovers `Cargo.toml`, `package.json`, `pyproject.toml` manifests
- Enterprise workspace engine with cross-budget routing

---

## [1.0.0] — 2026-01-10

### Added
- Initial public release
- `get_context_slice` with `model2vec-rs` hybrid vector search (pure Rust, < 100 MB RAM)
- Nuclear skeletonization — function bodies pruned to signatures, imports collapsed
- Multi-language grammar support: Rust, TypeScript/JavaScript, Python, Go
- Cross-platform pre-built binaries (macOS, Linux, Windows)
- MCP stdio server (`cortexast mcp`)
- Chaos resilience: binary skip, UTF-8 lossy, index auto-repair, 1 MB file cap
