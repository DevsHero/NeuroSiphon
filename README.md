# 🧠 NeuroSiphon

**Extract the Signal. Discard the Noise.**
_The God-Tier Context Optimizer for LLMs. Pure Rust. Native MCP._

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)
[![MCP Ready](https://img.shields.io/badge/MCP-Ready-blue)](https://modelcontextprotocol.io/)
[![Build & Release](https://github.com/DevsHero/NeuroSiphon/actions/workflows/release.yml/badge.svg)](https://github.com/DevsHero/NeuroSiphon/actions/workflows/release.yml)

---

## ⚡️ What is NeuroSiphon?

**NeuroSiphon** is not just another file reader for AI. It is a **Context Refinery**.

When you feed code to an LLM (Claude, ChatGPT), a huge chunk of your tokens are wasted on:

- ❌ Massive import lists (`import { a, b, c, ... }`)
- ❌ Boilerplate comments
- ❌ Indentation / whitespace
- ❌ Implementation details irrelevant to your question

**NeuroSiphon fixes this via “Nuclear Optimization”.**
It parses your code (AST), understands structure, nukes the fat, and feeds the LLM only the **pure logic marrow** it needs to reason.

---

## 👑 Why It’s the King of Context Efficiency

| Feature | Standard Tools (copy/paste) | 🧠 NeuroSiphon |
| :--- | :--- | :--- |
| **Optimization** | None (full text) | **Nuclear** (AST skeleton + import nuking) |
| **Search** | Grep / filename | **Hybrid** (vector semantics + graph ranking) |
| **Token Usage** | Bloated | **Aggressively reduced** (often 40–60% less noise) |
| **Speed** | Overhead-heavy | **Pure Rust** (fast scan + fast slice) |
| **Privacy** | Often cloud-dependent | **100% local** (local embeddings + local index) |

### 🛠️ Key Technologies

1. **Nuclear Skeletonization**
   - Functions collapse to signatures: `fn complex_logic() { /* ... */ }`
   - Imports collapse to one line: `// ... (imports nuked)`
   - Indentation is flattened to save whitespace tokens

2. **Lightweight Hybrid Search**
   - Uses **Model2Vec** (pure Rust, no ONNX runtime) for fast embeddings
   - Uses a **flat-file JSON index** + brute-force cosine (trivial for typical repo sizes)
   - Ranked against a dependency graph so you get **core logic first**

3. **Native MCP Server**
   - Built for **Claude Desktop** / **Cursor** / any MCP client
   - No plugins, no API keys, no cloud required

---

## 📦 Installation

### Option A — Pre-built Binary (fastest)

Download the latest binary from [Releases](https://github.com/DevsHero/NeuroSiphon/releases/latest):

| Platform | Download |
|---|---|
| Linux x86_64 | `neurosiphon-linux-x86_64` |
| Linux ARM64 | `neurosiphon-linux-aarch64` |
| macOS Intel | `neurosiphon-macos-x86_64` |
| macOS Apple Silicon | `neurosiphon-macos-aarch64` |
| Windows x86_64 | `neurosiphon-windows-x86_64.exe` |

```bash
# macOS / Linux — make executable
chmod +x neurosiphon-*
./neurosiphon-macos-aarch64 --help
```

### Option B — Build from Source

```bash
git clone https://github.com/DevsHero/NeuroSiphon.git
cd NeuroSiphon
cargo build --release
# Binary: ./target/release/neurosiphon
```

See [BUILDING.md](docs/BUILDING.md) for cross-compilation and platform-specific instructions.

---

## 🔌 MCP Setup

Add to your MCP client config (example uses Claude Desktop style JSON):

```json
{
  "mcpServers": {
    "neurosiphon": {
      "command": "/absolute/path/to/neurosiphon",
      "args": ["mcp"]
    }
  }
}
```

Restart your MCP client. That’s it.

See [MCP_SETUP.md](docs/MCP_SETUP.md)

---

## 🎮 Usage

### Automatic (via Chat)

Example:

> “@neurosiphon What is the authentication flow in this project?”

NeuroSiphon will:

1. Vector-search for “authentication”
2. Graph-rank the results (core logic > tests)
3. Skeletonize and nuke imports to fit your token budget
4. Return an optimized context slice

### Manual (CLI)

```bash
# Optimized slice of the 'src' folder
neurosiphon --target src --budget-tokens 32000 --xml

# Semantic search for specific concepts
neurosiphon --target . --query "database connection" --xml
```

---

## 🏗️ Architecture

NeuroSiphon drops heavy infra in favor of a compact, custom-built engine:

- **Vector Store**: flat-file JSON index + brute-force cosine similarity
- **Parser**: tree-sitter + safe fallbacks for broad language coverage
- **Walker**: `ignore` crate that respects `.gitignore` and auto-skips high-noise dirs (`node_modules`, `target`, `.venv`, etc.)

---

## 🤝 Contributing

PRs welcome.

- Core: Rust (Tokio, Rayon, Model2Vec)
- Focus: performance, compression ratio, multi-language correctness

Crafted with 🦀 by DevsHero.