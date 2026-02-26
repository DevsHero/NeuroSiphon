# Tauri UI Bridge: `active_languages` API

This document defines the communication protocol between the CortexAST Tauri backend (`cortexast` Rust library) and the CortexStudio frontend for managing which language grammars are dynamically loaded.

## Overview

CortexAST maintains an `active_languages: Vec<String>` field in [`Config`](../src/config.rs) that controls which Wasm grammars are fetched and loaded at startup. The default value is:

```json
["rust", "typescript", "python"]
```

These three languages are always statically compiled into the binary. Any additional languages require a Wasm grammar download from the CDN.

---

## Frontend → Backend: Update Active Languages

### Tauri Command

```typescript
import { invoke } from "@tauri-apps/api/core";

// Set the full list of active languages
await invoke("set_active_languages", {
  languages: ["rust", "typescript", "python", "go", "java", "dart"],
});
```

### Rust Handler (to be implemented in `src-tauri/src/commands.rs`)

```rust
#[tauri::command]
pub async fn set_active_languages(
    state: tauri::State<'_, AppState>,
    languages: Vec<String>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.active_languages = languages;
    config.save().map_err(|e| e.to_string())?;
    Ok(())
}
```

---

## Backend → Frontend: Language Status Events

When CortexAST starts or re-initialises, it emits a Tauri event with the status of each requested language:

```typescript
import { listen } from "@tauri-apps/api/event";

interface LanguageStatus {
  lang: string;
  status: "ready" | "downloading" | "failed" | "core";
}

await listen<LanguageStatus[]>("language-status", (event) => {
  console.log(event.payload);
});
```

Example payload:
```json
[
  { "lang": "rust",       "status": "core" },
  { "lang": "typescript", "status": "core" },
  { "lang": "python",     "status": "core" },
  { "lang": "go",         "status": "ready" },
  { "lang": "java",       "status": "failed" }
]
```

---

## Supported Language Slugs

| Slug         | Extensions     | CDN Path                                         |
|--------------|---------------|--------------------------------------------------|
| `rust`       | `.rs`         | *(statically linked — no download)*              |
| `typescript` | `.ts`, `.tsx` | *(statically linked — no download)*              |
| `python`     | `.py`         | *(statically linked — no download)*              |
| `go`         | `.go`         | `cdn.cortex-works.com/grammars/go.wasm`          |
| `dart`       | `.dart`       | `cdn.cortex-works.com/grammars/dart.wasm`        |
| `java`       | `.java`       | `cdn.cortex-works.com/grammars/java.wasm`        |
| `csharp`     | `.cs`         | `cdn.cortex-works.com/grammars/csharp.wasm`      |
| `php`        | `.php`        | `cdn.cortex-works.com/grammars/php.wasm`         |
| `proto`      | `.proto`      | `cdn.cortex-works.com/grammars/proto.wasm`       |
| `ruby`       | `.rb`         | `cdn.cortex-works.com/grammars/ruby.wasm`        |
| `kotlin`     | `.kt`         | `cdn.cortex-works.com/grammars/kotlin.wasm`      |
| `swift`      | `.swift`      | `cdn.cortex-works.com/grammars/swift.wasm`       |
| `lua`        | `.lua`        | `cdn.cortex-works.com/grammars/lua.wasm`         |
| `scala`      | `.scala`      | `cdn.cortex-works.com/grammars/scala.wasm`       |
| `elixir`     | `.ex`, `.exs` | `cdn.cortex-works.com/grammars/elixir.wasm`      |
| `haskell`    | `.hs`         | `cdn.cortex-works.com/grammars/haskell.wasm`     |
| `c`          | `.c`, `.h`    | `cdn.cortex-works.com/grammars/c.wasm`           |
| `cpp`        | `.cpp`, `.hpp`| `cdn.cortex-works.com/grammars/cpp.wasm`         |
| `zig`        | `.zig`        | `cdn.cortex-works.com/grammars/zig.wasm`         |

---

## Cache Directory

Wasm grammars and `.scm` queries are cached locally:

```
~/.cortex-works/grammars/
  go.wasm
  go_prune.scm
  dart.wasm
  dart_prune.scm
  ...
```

The `grammar_manager::ensure_grammar_available(lang)` function is called once per language at startup, checking the cache before initiating a network download.

---

## Config File Sync

The `active_languages` setting is stored in `~/.cortexast/config.toml`:

```toml
active_languages = ["rust", "typescript", "python", "go"]
```

On startup, `Config::load()` reads this file and the grammar manager ensures all listed grammars are available.
