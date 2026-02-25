#!/usr/bin/env bash
# =============================================================================
# release.sh — Local Release Script (macOS Apple Silicon) for CortexAST
# =============================================================================
set -euo pipefail
export CARGO_TERM_COLOR=always

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$REPO_ROOT"

APP_NAME="cortex-ast"
BIN_NAME="cortexast"

VERSION=$(grep '^version' "Cargo.toml" | head -1 | cut -d '"' -f2)
TAG="v$VERSION"

pass()  { printf "\033[32m✅  %s\033[0m\n" "$*"; }
info()  { printf "\033[34m──  %s\033[0m\n" "$*"; }
warn()  { printf "\033[33m⚠️   %s\033[0m\n" "$*"; }
banner(){ printf "\n\033[1;36m=== %s ===\033[0m\n" "$*"; }
die()   { printf "\033[31m❌  %s\033[0m\n" "$*" >&2; exit 1; }

extract_unreleased_notes() {
  awk '/^## \[Unreleased\][[:space:]]*$/ {in_unreleased=1; next} /^## \[/ {if (in_unreleased) exit} {if (in_unreleased) print}' CHANGELOG.md 2>/dev/null || true
}

repo_slug() {
  local origin="$(git remote get-url origin 2>/dev/null || true)"
  [[ -n "$origin" ]] || return 1
  origin="${origin%.git}"; origin="${origin#https://github.com/}"; origin="${origin#git@github.com:}"
  printf '%s' "$origin"
}

banner "$APP_NAME $TAG — Local Release"

for cmd in cargo cargo-zigbuild zig rustup gh; do
  if ! command -v "$cmd" &>/dev/null; then die "Missing: $cmd"; fi
done

if ! $DRY_RUN; then
  [[ -z "$(git status --porcelain)" ]] || die "Working tree dirty. Commit first."
fi

REPO_SLUG="$(repo_slug || true)"
[[ -n "$REPO_SLUG" ]] || die "Could not parse GitHub repo"
info "GitHub Repo: $REPO_SLUG"

UNRELEASED_NOTES="$(extract_unreleased_notes | sed -e 's/[[:space:]]\+$//')"
if [[ -z "${UNRELEASED_NOTES//[[:space:]]/}" ]]; then
  RELEASE_NOTES="Built locally from macOS (Apple Silicon)."
else
  RELEASE_NOTES="$(printf "## Release Notes\n\n%s\n\n---\n\nBuilt locally from macOS." "$UNRELEASED_NOTES")"
fi

banner "Building Targets"
TARGETS=("aarch64-apple-darwin" "aarch64-unknown-linux-gnu" "x86_64-pc-windows-gnullvm")

for target in "${TARGETS[@]}"; do
  info "Building $target ..."
  if [[ "$target" == "aarch64-apple-darwin" ]]; then
    cargo build --release --locked --target "$target"
  else
    cargo zigbuild --release --locked --target "$target"
  fi
  pass "Built $target"
done

banner "Packaging"
DIST="$REPO_ROOT/dist"
rm -rf "$DIST" && mkdir -p "$DIST"

package() {
  local target="$1" platform="$2" ext="$3"
  local src="target/$target/release"
  local dir="$DIST/$APP_NAME-$VERSION-$platform"
  mkdir -p "$dir"
  cp "$src/$BIN_NAME$ext" "$dir/"
  cp LICENSE README.md "$dir/" 2>/dev/null || true
  
  if [[ "$platform" == *"windows"* ]]; then
    (cd "$dir" && zip -qr "$DIST/$APP_NAME-$VERSION-$platform.zip" .)
  else
    tar -C "$dir" -czf "$DIST/$APP_NAME-$VERSION-$platform.tar.gz" .
  fi
  rm -rf "$dir"
}

package "aarch64-apple-darwin"      "macos-arm64"   ""
package "aarch64-unknown-linux-gnu" "linux-arm64"   ""
package "x86_64-pc-windows-gnullvm" "windows-x64"   ".exe"

if $DRY_RUN; then warn "Dry-run complete."; exit 0; fi

banner "Uploading"
gh release delete "$TAG" --repo "$REPO_SLUG" --yes 2>/dev/null || true
NOTES_FILE="$(mktemp)"
printf "%s\n" "$RELEASE_NOTES" > "$NOTES_FILE"
gh release create "$TAG" "$DIST"/*.tar.gz "$DIST"/*.zip --title "$APP_NAME $TAG" --notes-file "$NOTES_FILE" --repo "$REPO_SLUG"
rm -f "$NOTES_FILE" || true
pass "Release $TAG live!"
