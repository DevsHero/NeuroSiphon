#!/usr/bin/env zsh
set -euo pipefail

BIN_PATH="${1:?usage: bench_run.zsh /path/to/cortexast <repo1> [repo2 ...]}"
shift

if [[ ! -x "$BIN_PATH" ]]; then
  echo "ERROR: binary not executable: $BIN_PATH" >&2
  exit 2
fi

output_dir_for_repo() {
  local repo="$1"
  local cfg="$repo/.cortexast.json"
  if [[ -f "$cfg" ]]; then
    if command -v python3 >/dev/null 2>&1; then
      local od
      od=$(python3 - <<'PY' "$cfg" 2>/dev/null || true
import json, sys
path = sys.argv[1]
try:
  with open(path, 'r', encoding='utf-8') as f:
    v = json.load(f)
  od = v.get('output_dir')
  if isinstance(od, str) and od.strip():
    print(od.strip())
except Exception:
  pass
PY
      )
      if [[ -n "${od:-}" ]]; then
        echo "$od"
        return 0
      fi
    else
      # Best-effort fallback parser (string only)
      local od
      od=$(grep -E '"output_dir"\s*:' "$cfg" | head -n1 | sed -E 's/.*"output_dir"\s*:\s*"([^"]+)".*/\1/' || true)
      if [[ -n "${od:-}" ]]; then
        echo "$od"
        return 0
      fi
    fi
  fi

  echo ".cortexast"
}

# Count raw bytes for a repo under a target directory, excluding common heavy dirs.
raw_bytes_for() {
  local repo="$1"
  local target_rel="$2"
  local target_abs="$repo/$target_rel"

  local -a exts
  exts=(rs toml md json yml yaml ts tsx js jsx mjs cjs py go dart java cs php)

  local -a prune_patterns
  prune_patterns=(
    '*/.git/*'
    '*/node_modules/*'
    '*/target/*'
    '*/dist/*'
    '*/build/*'
    '*/.next/*'
    '*/.nuxt/*'
    '*/coverage/*'
    '*/.cortexast/*'
    '*/.venv/*'
    '*/venv/*'
    '*/__pycache__/*'
  )

  local -a prune_args
  prune_args=()
  for p in $prune_patterns; do
    prune_args+=( -path "$p" -o )
  done
  if (( ${#prune_args[@]} > 0 )); then
    prune_args[-1]=() # drop trailing -o (avoid sparse array empty args)
  fi

  local -a name_args
  name_args=()
  for ext in $exts; do
    name_args+=( -name "*.$ext" -o )
  done
  if (( ${#name_args[@]} > 0 )); then
    name_args[-1]=()
  fi

  command find "$target_abs" \( "${prune_args[@]}" \) -prune -o -type f \( "${name_args[@]}" \) -print0 \
    | xargs -0 /usr/bin/stat -f%z 2>/dev/null \
    | awk '{s+=$1} END{print s+0}'
}

raw_files_for() {
  local repo="$1"
  local target_rel="$2"
  local target_abs="$repo/$target_rel"

  local -a exts
  exts=(rs toml md json yml yaml ts tsx js jsx mjs cjs py go dart java cs php)

  local -a prune_patterns
  prune_patterns=(
    '*/.git/*'
    '*/node_modules/*'
    '*/target/*'
    '*/dist/*'
    '*/build/*'
    '*/.next/*'
    '*/.nuxt/*'
    '*/coverage/*'
    '*/.cortexast/*'
    '*/.venv/*'
    '*/venv/*'
    '*/__pycache__/*'
  )

  local -a prune_args
  prune_args=()
  for p in $prune_patterns; do
    prune_args+=( -path "$p" -o )
  done
  if (( ${#prune_args[@]} > 0 )); then
    prune_args[-1]=()
  fi

  local -a name_args
  name_args=()
  for ext in $exts; do
    name_args+=( -name "*.$ext" -o )
  done
  if (( ${#name_args[@]} > 0 )); then
    name_args[-1]=()
  fi

  command find "$target_abs" \( "${prune_args[@]}" \) -prune -o -type f \( "${name_args[@]}" \) -print0 \
    | tr '\0' '\n' \
    | awk 'NF{c++} END{print c+0}'
}

bench_one_repo() {
  local repo="$1"
  if [[ ! -d "$repo" ]]; then
    echo "SKIP (missing dir): $repo" >&2
    return 0
  fi

  local target='.'
  if [[ -d "$repo/src" ]]; then
    target='src'
  fi

  local out_rel
  out_rel=$(output_dir_for_repo "$repo")
  local out_dir="$repo/$out_rel"

  rm -rf "$out_dir"

  local raw_bytes raw_files raw_tokens
  raw_bytes=$(raw_bytes_for "$repo" "$target")
  raw_files=$(raw_files_for "$repo" "$target")
  raw_tokens=$(( (raw_bytes + 3) / 4 ))

  # Slice run
  local t_slice
  t_slice=$(REPO="$repo" BIN="$BIN_PATH" TARGET="$target" /usr/bin/time -p zsh -c 'cd "$REPO" && "$BIN" --target "$TARGET" --budget-tokens 32000 >/dev/null' 2>&1 | awk '/^real/{print $2}' | tail -n1)

  local out_bytes out_tokens
  out_bytes=$(/usr/bin/stat -f%z "$out_dir/active_context.xml" 2>/dev/null || echo 0)
  out_tokens=$(( (out_bytes + 3) / 4 ))

  local reduction_pct
  if [[ "$raw_bytes" -gt 0 && "$out_bytes" -gt 0 ]]; then
    reduction_pct=$(( 100 - (out_bytes * 100 / raw_bytes) ))
  else
    reduction_pct=0
  fi

  # Query run (forces index rebuild because output dir was removed)
  rm -rf "$out_dir"
  local t_query_cold
  t_query_cold=$(REPO="$repo" BIN="$BIN_PATH" TARGET="$target" /usr/bin/time -p zsh -c 'cd "$REPO" && "$BIN" --target "$TARGET" --query "auth" --query-limit 20 --budget-tokens 32000 >/dev/null' 2>&1 | awk '/^real/{print $2}' | tail -n1)

  # Warm query run (index should be present)
  local t_query_warm
  t_query_warm=$(REPO="$repo" BIN="$BIN_PATH" TARGET="$target" /usr/bin/time -p zsh -c 'cd "$REPO" && "$BIN" --target "$TARGET" --query "auth" --query-limit 20 --budget-tokens 32000 >/dev/null' 2>&1 | awk '/^real/{print $2}' | tail -n1)

  echo "REPO=$repo"
  echo "TARGET=$target"
  echo "RAW_FILES=$raw_files"
  echo "RAW_BYTES=$raw_bytes"
  echo "RAW_TOKENS≈$raw_tokens"
  echo "OUT_BYTES=$out_bytes"
  echo "OUT_TOKENS≈$out_tokens"
  echo "REDUCTION_PCT≈$reduction_pct"
  echo "TIME_SLICE_S=$t_slice"
  echo "TIME_QUERY_COLD_S=$t_query_cold"
  echo "TIME_QUERY_WARM_S=$t_query_warm"
  echo "---"
}

for repo in "$@"; do
  bench_one_repo "$repo"
done
