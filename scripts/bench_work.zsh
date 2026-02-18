#!/usr/bin/env zsh
set -euo pipefail

BIN_PATH="${1:?usage: bench_work.zsh /path/to/neurosiphon <work_root>}"
WORK_ROOT="${2:-/Users/hero/Documents/work}"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "ERROR: binary not executable: $BIN_PATH" >&2
  exit 2
fi
if [[ ! -d "$WORK_ROOT" ]]; then
  echo "ERROR: work root not found: $WORK_ROOT" >&2
  exit 2
fi

# 1) Wipe old scan outputs (best-effort default output dir)
find "$WORK_ROOT" -maxdepth 4 -type d -name .neurosiphon -prune -print | while IFS= read -r d; do
  rm -rf "$d"
done

# 2) Run slice and query passes across top-level dirs
ok_slice=0; fail_slice=0; tested=0
ok_query=0; fail_query=0

echo "== WORK_SLICE_PASS =="
for d in "$WORK_ROOT"/*; do
  [[ -d "$d" ]] || continue
  name=$(basename "$d")
  target='.'
  [[ -d "$d/src" ]] && target='src'

  time_out=$(REPO="$d" BIN="$BIN_PATH" TARGET="$target" /usr/bin/time -p zsh -c 'cd "$REPO" && "$BIN" --target "$TARGET" --budget-tokens 32000 >/dev/null' 2>&1)
  ec=$?
  t=$(echo "$time_out" | awk '/^real/{print $2}' | tail -n1)
  if [[ $ec -eq 0 ]]; then
    ok_slice=$((ok_slice+1))
  else
    fail_slice=$((fail_slice+1))
  fi
  tested=$((tested+1))
  echo "$name ec=$ec real=${t}s"
done

echo "SUMMARY_SLICE ok=$ok_slice fail=$fail_slice tested=$tested"

echo "== WORK_QUERY_PASS (cold rebuild per repo) =="
for d in "$WORK_ROOT"/*; do
  [[ -d "$d" ]] || continue
  rm -rf "$d/.neurosiphon"

  name=$(basename "$d")
  target='.'
  [[ -d "$d/src" ]] && target='src'

  time_out=$(REPO="$d" BIN="$BIN_PATH" TARGET="$target" /usr/bin/time -p zsh -c 'cd "$REPO" && "$BIN" --target "$TARGET" --query "auth" --query-limit 20 --budget-tokens 32000 >/dev/null' 2>&1)
  ec=$?
  t=$(echo "$time_out" | awk '/^real/{print $2}' | tail -n1)
  if [[ $ec -eq 0 ]]; then
    ok_query=$((ok_query+1))
  else
    fail_query=$((fail_query+1))
  fi
  echo "$name ec=$ec real=${t}s"
done

echo "SUMMARY_QUERY ok=$ok_query fail=$fail_query tested=$tested"
