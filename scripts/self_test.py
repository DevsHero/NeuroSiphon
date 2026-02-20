#!/usr/bin/env python3
"""Self-test runner (v2.0.0): exercises ALL CortexAST Megatools against dataset-mixer.

This is a QC harness for:
- cortex_code_explorer: map_overview + deep_slice
- cortex_symbol_analyzer: read_source + find_usages + blast_radius + propagation_checklist
- cortex_chronos: save_checkpoint + list_checkpoints + compare_checkpoint
- run_diagnostics

Notes:
- run_diagnostics may legitimately return compiler errors depending on the target repo state.
  That still counts as "tool works" (the tool ran and returned structured output).
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Any


DEFAULT_BIN = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "target",
    "release",
    "cortexast",
)
DEFAULT_REPO = "/Users/hero/Documents/GitHub/dataset-mixer"


@dataclass
class ToolCallResult:
    is_error: bool
    text: str
    raw: dict[str, Any]


def run_mcp(messages: list[dict[str, Any]], bin_path: str) -> list[dict[str, Any]]:
    stdin = "\n".join(json.dumps(m) for m in messages) + "\n"
    proc = subprocess.run(
        [bin_path, "mcp"],
        input=stdin,
        capture_output=True,
        text=True,
        timeout=120,
    )

    out: list[dict[str, Any]] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except Exception:
            # keep going; server may print non-json on rare stderr proxies
            continue
    return out


def tools_list(bin_path: str) -> list[str]:
    msgs = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05"},
        },
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    ]
    replies = run_mcp(msgs, bin_path)
    for r in replies:
        if r.get("id") == 2:
            tools = r.get("result", {}).get("tools", [])
            return [t.get("name", "") for t in tools if isinstance(t, dict)]
    return []


def call_tool(bin_path: str, name: str, args: dict[str, Any]) -> ToolCallResult:
    msgs = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05"},
        },
        {"jsonrpc": "2.0", "id": 2, "method": "initialized", "params": {}},
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        },
    ]
    replies = run_mcp(msgs, bin_path)
    for r in reversed(replies):
        if r.get("id") == 3:
            result = r.get("result", {})
            content = result.get("content", [])
            text = ""
            if isinstance(content, list) and content:
                first = content[0]
                if isinstance(first, dict):
                    text = first.get("text", "") or ""
            return ToolCallResult(bool(result.get("isError")), text, r)
    return ToolCallResult(True, f"PARSE_ERROR: no tools/call reply for {name}", {"replies": replies})


def header(title: str) -> None:
    print("\n" + "=" * 72)
    print(title)
    print("=" * 72)


def show(res: ToolCallResult, max_lines: int = 40) -> None:
    status = "ERROR" if res.is_error else "OK"
    print(f"[{status}]")
    lines = (res.text or "").splitlines()
    for l in lines[:max_lines]:
        print(" ", l)
    if len(lines) > max_lines:
        print(f"  ... ({len(lines) - max_lines} more lines)")


def extract_first_symbol(map_text: str) -> tuple[str, str] | None:
    """Parse repo_map output and return (file_path, symbol_name)."""
    current_file: str | None = None
    file_re = re.compile(r"^\s{2}([^\s].*\.(rs|py|ts|tsx|js|jsx|go|java|kt|cs))\s*$")
    sym_re = re.compile(r"^\s{4}\[[^\]]+\]\s+(.+?)\s*$")
    for line in map_text.splitlines():
        m = file_re.match(line)
        if m:
            current_file = m.group(1).strip()
            continue
        m2 = sym_re.match(line)
        if m2 and current_file:
            sym = m2.group(1).strip()
            if sym:
                return (current_file, sym)
    return None


def extract_first_file(map_text: str) -> str | None:
    """Parse repo_map output and return the first file path we can reconstruct.

    Works in both deep mode (symbols visible) and summary-first mode (symbols hidden).
    """
    dir_re = re.compile(r"^(\s*)([^\s].*?/)(?:\s*\(.+\))?\s*$")
    file_re = re.compile(r"^(\s{2,})([^\s].*\.(rs|py|ts|tsx|js|jsx|go|java|kt|cs))\s*$")

    stack: list[str] = []
    candidates: list[str] = []
    for line in map_text.splitlines():
        mdir = dir_re.match(line)
        if mdir:
            spaces = len(mdir.group(1))
            depth = spaces // 2
            name = mdir.group(2).strip()
            if not name.endswith("/"):
                continue
            name = name[:-1]
            if not name:
                continue
            stack = stack[:depth] + [name]
            continue

        mf = file_re.match(line)
        if mf:
            spaces = len(mf.group(1))
            file_depth = spaces // 2
            fname = mf.group(2).strip()
            prefix = "/".join(stack[:file_depth])
            candidates.append(f"{prefix}/{fname}" if prefix else fname)

    if not candidates:
        return None

    # Prefer common entrypoints / small glue files for symbol extraction.
    for suf in ("/main.rs", "/lib.rs", "/mod.rs"):
        for c in candidates:
            if c.lower().endswith(suf):
                return c
    return candidates[0]

    return None


def extract_symbol_from_skeleton(text: str, path: str) -> str | None:
    """Best-effort symbol extraction from skeleton/XML output."""
    ext = os.path.splitext(path)[1].lower().lstrip(".")
    if ext == "rs":
        # Prefer functions first, then types.
        for pat in [
            r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            r"\bstruct\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            r"\benum\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            r"\btrait\s+([A-Za-z_][A-Za-z0-9_]*)\b",
            r"\bimpl\s+([A-Za-z_][A-Za-z0-9_]*)\b",
        ]:
            m = re.search(pat, text)
            if m:
                return m.group(1)
        return None

    if ext == "py":
        for pat in [r"\bdef\s+([A-Za-z_][A-Za-z0-9_]*)\b", r"\bclass\s+([A-Za-z_][A-Za-z0-9_]*)\b"]:
            m = re.search(pat, text)
            if m:
                return m.group(1)
        return None

    # Generic fallback (TS/JS/etc.)
    for pat in [
        r"\bfunction\s+([A-Za-z_][A-Za-z0-9_]*)\b",
        r"\bclass\s+([A-Za-z_][A-Za-z0-9_]*)\b",
        r"\binterface\s+([A-Za-z_][A-Za-z0-9_]*)\b",
        r"\btype\s+([A-Za-z_][A-Za-z0-9_]*)\b",
    ]:
        m = re.search(pat, text)
        if m:
            return m.group(1)
    return None


def main() -> int:
    bin_path = os.environ.get("CORTEXAST_BIN", DEFAULT_BIN)
    repo = os.environ.get("CORTEXAST_REPO", DEFAULT_REPO)
    target_dir = "apps/desktop/src"

    if not os.path.exists(bin_path):
        print(f"ERROR: cortexast binary not found at: {bin_path}")
        print("Build it with: cargo build --release")
        return 2

    header("TOOLS/LIST (schema sanity)")
    names = tools_list(bin_path)
    print("Tools:", ", ".join(names))
    for required in ["cortex_code_explorer", "cortex_symbol_analyzer", "cortex_chronos", "run_diagnostics"]:
        if required not in names:
            print(f"ERROR: missing tool in tools/list: {required}")
            return 3

    # --- cortex_code_explorer.map_overview ---
    header("cortex_code_explorer(action=map_overview) — apps/desktop/src")
    map_res = call_tool(
        bin_path,
        "cortex_code_explorer",
        {"repoPath": repo, "action": "map_overview", "target_dir": target_dir},
    )
    show(map_res, 60)

    if map_res.is_error:
        print("Cannot continue QC without a repo map.")
        return 4

    picked = extract_first_symbol(map_res.text)
    picked_path: str | None = None
    picked_symbol: str | None = None

    if picked:
        picked_path, picked_symbol = picked
    else:
        # Summary-first mode hides symbols for large folders.
        # Fallback: pick a file path from the map, deep_slice it, then regex-extract a symbol.
        rel_file = extract_first_file(map_res.text)
        if not rel_file:
            print("ERROR: could not extract a file path from map_overview output")
            return 5

        # repo_map output typically prints the basename of target_dir (e.g. 'src/') as the root.
        # That root is already included in target_dir, so strip it if present.
        root_name = os.path.basename(target_dir.rstrip("/"))
        if rel_file.startswith(root_name + "/"):
            rel_file = rel_file[len(root_name) + 1 :]

        picked_path = f"{target_dir.rstrip('/')}/{rel_file}".replace("//", "/")
        header("Fallback: deep_slice picked file to discover a real symbol")
        tmp_slice = call_tool(
            bin_path,
            "cortex_code_explorer",
            {"repoPath": repo, "action": "deep_slice", "target": picked_path, "budget_tokens": 32000},
        )
        show(tmp_slice, 40)
        if tmp_slice.is_error:
            print("ERROR: deep_slice failed; cannot auto-discover symbol")
            return 6
        picked_symbol = extract_symbol_from_skeleton(tmp_slice.text, picked_path)
        if not picked_symbol:
            print("ERROR: could not extract a symbol name from deep_slice output")
            return 7

    assert picked_path and picked_symbol
    print(f"\nPicked symbol for QC: symbol_name='{picked_symbol}' in path='{picked_path}'")

    # --- cortex_code_explorer.deep_slice ---
    header("cortex_code_explorer(action=deep_slice) — slice the picked file")
    slice_res = call_tool(
        bin_path,
        "cortex_code_explorer",
        {"repoPath": repo, "action": "deep_slice", "target": picked_path, "budget_tokens": 8000},
    )
    show(slice_res, 40)

    # --- cortex_symbol_analyzer.read_source ---
    header("cortex_symbol_analyzer(action=read_source) — picked symbol")
    read_res = call_tool(
        bin_path,
        "cortex_symbol_analyzer",
        {"repoPath": repo, "action": "read_source", "path": picked_path, "symbol_name": picked_symbol},
    )
    show(read_res, 60)

    # --- cortex_symbol_analyzer.read_source (batch) ---
    header("cortex_symbol_analyzer(action=read_source) — batch mode symbol_names")
    read_batch_res = call_tool(
        bin_path,
        "cortex_symbol_analyzer",
        {"repoPath": repo, "action": "read_source", "path": picked_path, "symbol_names": [picked_symbol]},
    )
    show(read_batch_res, 40)

    # --- cortex_symbol_analyzer.find_usages ---
    header("cortex_symbol_analyzer(action=find_usages) — picked symbol")
    usages_res = call_tool(
        bin_path,
        "cortex_symbol_analyzer",
        {"repoPath": repo, "action": "find_usages", "target_dir": ".", "symbol_name": picked_symbol},
    )
    show(usages_res, 60)

    # --- cortex_symbol_analyzer.blast_radius ---
    header("cortex_symbol_analyzer(action=blast_radius) — picked symbol")
    blast_res = call_tool(
        bin_path,
        "cortex_symbol_analyzer",
        {"repoPath": repo, "action": "blast_radius", "target_dir": ".", "symbol_name": picked_symbol},
    )
    show(blast_res, 60)

    # --- cortex_symbol_analyzer.propagation_checklist ---
    header("cortex_symbol_analyzer(action=propagation_checklist) — picked symbol")
    prop_res = call_tool(
        bin_path,
        "cortex_symbol_analyzer",
        {"repoPath": repo, "action": "propagation_checklist", "target_dir": ".", "symbol_name": picked_symbol},
    )
    show(prop_res, 60)

    # --- cortex_chronos.save_checkpoint / list_checkpoints / compare_checkpoint ---
    t = int(time.time())
    tag_a = f"qc-a-{t}"
    tag_b = f"qc-b-{t}"

    header("cortex_chronos(action=save_checkpoint) — tag A")
    save_a = call_tool(
        bin_path,
        "cortex_chronos",
        {"repoPath": repo, "action": "save_checkpoint", "path": picked_path, "symbol_name": picked_symbol, "semantic_tag": tag_a},
    )
    show(save_a, 20)

    header("cortex_chronos(action=save_checkpoint) — tag B")
    save_b = call_tool(
        bin_path,
        "cortex_chronos",
        {"repoPath": repo, "action": "save_checkpoint", "path": picked_path, "symbol_name": picked_symbol, "semantic_tag": tag_b},
    )
    show(save_b, 20)

    header("cortex_chronos(action=list_checkpoints)")
    lst = call_tool(bin_path, "cortex_chronos", {"repoPath": repo, "action": "list_checkpoints"})
    show(lst, 80)

    header("cortex_chronos(action=compare_checkpoint) — tag A vs tag B")
    cmp_ok = call_tool(
        bin_path,
        "cortex_chronos",
        {
            "repoPath": repo,
            "action": "compare_checkpoint",
            "symbol_name": picked_symbol,
            "tag_a": tag_a,
            "tag_b": tag_b,
            "path": picked_path,
        },
    )
    show(cmp_ok, 80)

    header("cortex_chronos(action=compare_checkpoint) — NEGATIVE TEST (wrong tag)")
    cmp_bad = call_tool(
        bin_path,
        "cortex_chronos",
        {
            "repoPath": repo,
            "action": "compare_checkpoint",
            "symbol_name": picked_symbol,
            "tag_a": "definitely-not-a-real-tag",
            "tag_b": tag_b,
            "path": picked_path,
        },
    )
    show(cmp_bad, 40)

    header("cortex_chronos(action=delete_checkpoint) — cleanup QC tags")
    del_a = call_tool(
        bin_path,
        "cortex_chronos",
        {"repoPath": repo, "action": "delete_checkpoint", "symbol_name": picked_symbol, "semantic_tag": tag_a},
    )
    show(del_a, 20)
    del_b = call_tool(
        bin_path,
        "cortex_chronos",
        {"repoPath": repo, "action": "delete_checkpoint", "symbol_name": picked_symbol, "semantic_tag": tag_b},
    )
    show(del_b, 20)

    # --- run_diagnostics ---
    header("run_diagnostics — repo root (may return compile errors depending on repo state)")
    diag = call_tool(bin_path, "run_diagnostics", {"repoPath": repo})
    show(diag, 60)

    print("\nQC sweep complete.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
