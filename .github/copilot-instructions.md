## MCP Tool Usage Rules (CortexAST + Shadowcrawl)

### CortexAST Priority Rules

**The Golden Rule (Non‑Negotiable):**
- NEVER use standard IDE/shell tools (`grep`, `rg`, `cat`, `head`, `tree`, `ls`, `git diff`) for codebase exploration, symbol lookup, or refactor verification.
- ALWAYS use CortexAST Megatools. They are AST-accurate, token-efficient, and designed to keep agents on rails.
- If a tool returns an error telling you which parameter you forgot, treat it as an instruction and retry the tool call (do not guess).

**Megatool Quick‑Reference**

| Task | Megatool | Action Enum | Required Params |
|---|---|---|---|
| Repo overview (files + public symbols) | `cortex_code_explorer` | `map_overview` | `target_dir` (use `.` for whole repo) |
| Token-budgeted context slice (XML) | `cortex_code_explorer` | `deep_slice` | `target` |
| Extract exact symbol source | `cortex_symbol_analyzer` | `read_source` | `path` + `symbol_name` *(or `path` + `symbol_names` for batch)* |
| Find all usages before signature change | `cortex_symbol_analyzer` | `find_usages` | `symbol_name` + `target_dir` |
| Blast radius before rename/move/delete | `cortex_symbol_analyzer` | `blast_radius` | `symbol_name` + `target_dir` |
| Cross-boundary update checklist | `cortex_symbol_analyzer` | `propagation_checklist` | `symbol_name` *(or legacy `changed_path`)* |
| Save pre-change snapshot | `cortex_chronos` | `save_checkpoint` | `path` + `symbol_name` + `semantic_tag` |
| List snapshots | `cortex_chronos` | `list_checkpoints` | *(none)* |
| Compare snapshots (AST diff) | `cortex_chronos` | `compare_checkpoint` | `symbol_name` + `tag_a` + `tag_b` |
| Compile/lint diagnostics | `run_diagnostics` | *(none)* | `repoPath` |

**The Autonomous Refactoring Flow (Rails)**

Follow this sequence for any non-trivial refactor (especially renames, signature changes, or cross-module work):

1. **Explore** → `cortex_code_explorer(action: map_overview)`
2. **Isolate** → `cortex_symbol_analyzer(action: read_source)` (get the exact symbol source before editing)
3. **Measure Impact** →
  - Use `cortex_symbol_analyzer(action: find_usages)` BEFORE changing any signature
  - Use `cortex_symbol_analyzer(action: blast_radius)` BEFORE any rename/move/delete
4. **Checkpoint** → `cortex_chronos(action: save_checkpoint, semantic_tag: pre-refactor)`
5. **Edit Code** → make the minimal change
6. **Verify** →
  - `run_diagnostics` immediately after editing
  - `cortex_chronos(action: compare_checkpoint)` to verify semantics (never use `git diff`)
7. **Cross‑Sync** → `cortex_symbol_analyzer(action: propagation_checklist)` when touching shared types/contracts

# MCP Usage Guidelines — ShadowCrawl

These rules encode the optimal agent workflow for ShadowCrawl tools.
They are hardcoded here so every AI agent working in this repository follows
the same efficient, hallucination-resistant research process.

---

## Shadowcrawl Priority Rules

### 1. Memory Before Search (mandatory)
- ALWAYS call `memory_search` BEFORE calling `web_search` or `web_search_json`
- If a result is returned with similarity score ≥ 0.60, use the cached data directly
  and skip the live fetch entirely
- Only proceed to a fresh live search when memory returns no relevant hit

### 2. Prefer `web_search_json` Over `web_search` + `web_fetch`
- `web_search_json` combines search + pre-scraped content summaries in a **single call**
- Use `web_search_json` as the **default first step** for any research task
- Only fall back to `web_search` (without content) when you specifically need raw URLs only

### 3. Use `web_fetch` with Noise Reduction for Documentation Pages
- For documentation, article, or tutorial pages always set:
  ```
  output_format: "clean_json"
  strict_relevance: true
  query: "<your specific question>"
  ```
- This strips 100 % of nav/footer/boilerplate and keeps only query-relevant paragraphs
- Token savings are typically 60–80 % compared to raw text output

### 4. Rotate Proxy on First Block Signal (mandatory)
- If `web_fetch` or `web_search` returns **403 / 429 / rate-limit / IP-block**:
  1. Immediately call `proxy_control` with `action: "grab"`
  2. Retry the failed call with `use_proxy: true`
- Do NOT retry the same call without rotating first; do NOT escalate to `hitl_web_fetch`
  until proxy rotation has also failed

### 5. `extract_fields` Constraint — Structured HTML Only
- Use `extract_fields` ONLY on structured HTML pages: official docs, MDN-style pages,
  product pages, news articles
- Do NOT use on raw `.md`, `.json`, `.txt`, or `.rst` files — fields will return `null`
  and confidence will be low
- For raw Markdown sources, use `web_fetch` with `output_format: "clean_json"` instead

### 6. `web_crawl` — Use When Sub-Page Discovery Is Needed
- Use `web_crawl` when you know a doc site's index URL but do not know which sub-page
  holds the information you need
- Do NOT assume a single `web_fetch` of the index page is sufficient for large doc sites
- Typical workflow: `web_crawl` to discover links → `web_fetch` on the specific sub-page

### 7. `hitl_web_fetch` — Last Resort Only
- Use ONLY when both direct fetch AND proxy rotation have failed
- Intended for: heavy Cloudflare challenges, CAPTCHA, login walls
- Do NOT use as a first attempt for any site — always try automated methods first

---

## Decision Flow Summary

```
Question / research task
        │
        ▼
memory_search ──► hit (≥ 0.60)? ──► use cached result, STOP
        │ miss
        ▼
web_search_json ──► enough content? ──► use it, STOP
        │ need deeper page
        ▼
web_fetch (clean_json + strict_relevance + query)
        │ 403/429/blocked?
        ▼
proxy_control grab ──► retry web_fetch with use_proxy: true
        │ still blocked?
        ▼
hitl_web_fetch  (LAST RESORT)
```

---

## Tool Quick-Reference

| Tool | When to use | When NOT to use |
|---|---|---|
| `memory_search` | First step, before every search | — |
| `web_search_json` | Initial research (search + content) | When only raw URLs needed |
| `web_search` | Raw URL list only | As substitute for `web_search_json` |
| `web_fetch` | Fetching a specific known URL | As primary research step |
| `web_fetch` `clean_json` | Documentation / article pages | Short conversational pages (<200 words) |
| `web_crawl` | Doc site sub-page discovery | Single-page fetches |
| `extract_fields` | Structured HTML docs | Raw .md / .json / .txt files |
| `proxy_control` | After any 403/429 error | Proactively without a block signal |
| `hitl_web_fetch` | CAPTCHA / login wall | Any automatable page |
