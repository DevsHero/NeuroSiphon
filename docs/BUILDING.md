# Building CortexAST

This document covers how to build `cortexast` from source for every supported platform — including cross-compilation.

---

## Prerequisites

1. **Rust stable toolchain** (≥ 1.75)

   ```bash
   curl https://sh.rustup.rs -sSf | sh
   rustup update stable
   ```

2. **C compiler** (for tree-sitter grammar compilation)
   - **macOS**: Xcode Command Line Tools — `xcode-select --install`
   - **Linux**: `gcc` / `clang` from your distro (`apt install build-essential`)
   - **Windows**: Visual Studio Build Tools (MSVC), installed automatically when you select the `x86_64-pc-windows-msvc` rustup toolchain

---

## Build (native platform)

```bash
# Clone
git clone https://github.com/DevsHero/CortexAST.git
cd CortexAST

# Debug build (fast compile, not for distribution)
cargo build

# Release build (optimised, stripped binary)
cargo build --release

# Binary location
./target/release/cortexast --help
```

---

## Platform-specific notes

### macOS

```bash
# Intel Mac
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin
# → target/x86_64-apple-darwin/release/cortexast

# Apple Silicon (M1/M2/M3) — native
cargo build --release
# → target/release/cortexast

# Universal binary (Intel + Apple Silicon)
rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
lipo -create -output cortexast \
  target/x86_64-apple-darwin/release/cortexast \
  target/aarch64-apple-darwin/release/cortexast
```

### Linux

```bash
# x86_64 (native)
cargo build --release
# → target/release/cortexast

# ARM64 (cross-compile — see Cross-Compilation section below)
```

### Windows

```powershell
# Requires MSVC toolchain
rustup toolchain install stable-x86_64-pc-windows-msvc
cargo build --release
# → target\release\cortexast.exe
```

---

## Cross-Compilation (Linux ARM64)

The recommended tool is [`cross`](https://github.com/cross-rs/cross), which uses Docker containers with pre-installed cross-linkers and sysroots.

### Install cross

```bash
cargo install cross --git https://github.com/cross-rs/cross
```

Docker must be running.

### Build for Linux ARM64

```bash
cross build --release --target aarch64-unknown-linux-gnu
# → target/aarch64-unknown-linux-gnu/release/cortexast
```

---

## Alternative: cargo-zigbuild (no Docker required)

[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) uses `zig` as a universal cross-linker.

```bash
# Install zig
brew install zig        # macOS
apt install zig         # Ubuntu 24.04+

# Install cargo-zigbuild
cargo install cargo-zigbuild

# Cross-compile to Linux ARM64 from any host
rustup target add aarch64-unknown-linux-gnu
cargo zigbuild --release --target aarch64-unknown-linux-gnu
```

---

## Pre-built Binaries

You can download ready-to-use binaries from [GitHub Releases](https://github.com/DevsHero/CortexAST/releases/latest).

Verify with `sha256sums.txt` provided in each release.

```bash
# Linux / macOS — verify
sha256sum -c sha256sums.txt

# Make executable (Linux / macOS)
chmod +x cortexast-*
```

---

## CI / Automated Releases

The project uses GitHub Actions (`.github/workflows/release.yml`) to automatically:

1. Detect `[build]` in any commit message pushed to `main`/`master`
2. Build binaries for all targets in parallel
3. Create a GitHub Release tagged `v{version}` (from `Cargo.toml`)
4. Attach all binaries + `sha256sums.txt` + changelog
