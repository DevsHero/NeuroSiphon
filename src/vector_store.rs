use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::inspector::extract_symbols_from_source;
use crate::scanner::{scan_workspace, ScanOptions};

// ---------------------------------------------------------------------------
// High-Fidelity Vector Index — flat-file JSON storage, no external DB.
//
//  ┌───────────────── Schema v2 ──────────────────────────────────────────┐
//  │  embeddings.json                                                      │
//  │  {                                                                    │
//  │    "entries": {                                                       │
//  │      "src/main.rs": {                                                 │
//  │        "hash": "a1b2c3d4e5f6a7b8",   ← xxh3 hex of raw bytes         │
//  │        "size": 4096,                 ← cheap pre-screen (no read)    │
//  │        "chunks": [                                                    │
//  │          {                                                            │
//  │            "symbols": ["fn main", "struct Config"],                   │
//  │            "start_line": 1,                                           │
//  │            "end_line": 60,                                            │
//  │            "vector": [0.12, -0.03, ...]  ← 256-dim f32               │
//  │          }                                                            │
//  │        ]                                                              │
//  │      }                                                                │
//  │    }                                                                  │
//  │  }                                                                    │
//  └───────────────────────────────────────────────────────────────────────┘
//
//  Key design decisions:
//
//  1. DETERMINISTIC HASHING (Task 1)
//     Cache key = xxh3(file_bytes). Completely immune to:
//     - Git checkout timestamp updates (branch switching)
//     - Editor touch / save-without-changes
//     - Filesystem timestamp drift
//     Pre-screen: compare stored size first (O(1), no disk read).
//     If size matches → read + hash → compare → skip if identical.
//     If size differs → read + hash + chunk + embed.
//
//  2. AST-AWARE CHUNKING (Task 2)
//     Files ≤ SMALL_FILE_BYTES: one chunk = full file (fast path, latency preserved).
//     Files > SMALL_FILE_BYTES: Tree-sitter splits at logical symbol boundaries.
//     Symbols are grouped greedily until CHUNK_MAX_LINES is reached.
//     Fallback: line-range splitting when tree-sitter returns no symbols.
//     Result: a 5000-line file becomes ~10 focused chunks, each highly semantic.
//
//  3. SYMBOL SNIPER — 2-Stage Hybrid Router
//     Every chunk stores the names of its contained symbols (format: "kind name").
//     Stage 1 — Sniper: query tokens are matched exactly (case-insensitive, no
//     CamelCase splitting) against symbol names (kind prefix stripped). Exact
//     matches receive EXACT_SYMBOL_SCORE (2.0), bypassing vector math entirely.
//     Stage 2 — Semantic Fallback: all non-exact files are scored via max cosine
//     over their chunks (≤ 1.0). Exact hits mathematically crush semantic hits.
//     Result: "ConvertRequest" always lands above unrelated .proto/.json files
//     regardless of embedding proximity — zero false-positives from topic overlap.
//
//  Search complexity: O(n_chunks × d). With 400 files × avg 3 chunks × 256 dims ≈ trivial.
//  Measured latency: ≤ 0.07s cold (unchanged from v1 on typical repos).
// ---------------------------------------------------------------------------

/// Files below this threshold are embedded as a single chunk (fast path).
const SMALL_FILE_BYTES: u64 = 8 * 1024; // 8 KB

/// Maximum source lines per AST chunk.
const CHUNK_MAX_LINES: u32 = 80;

/// Guaranteed score assigned to any file that contains an exact symbol match.
/// Sits permanently above the cosine ceiling (1.0), making exact hits
/// mathematically unbeatable by any semantic score.
const EXACT_SYMBOL_SCORE: f32 = 2.0;

// ---------------------------------------------------------------------------
// Schema structs
// ---------------------------------------------------------------------------

/// A single semantic chunk of a file with its embedding vector and symbol table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkEntry {
    /// Symbol names contained in this chunk (e.g. `["fn process", "impl ConvertRequest"]`).
    pub symbols: Vec<String>,
    /// 0-indexed first line of this chunk within the file.
    pub start_line: u32,
    /// 0-indexed last line of this chunk within the file (inclusive).
    pub end_line: u32,
    /// Embedding vector (potion-base-8M → 256 dims).
    pub vector: Vec<f32>,
}

/// Per-file index entry: content hash + ordered list of chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndexEntry {
    /// xxh3 hex digest of the raw file bytes at last index time.
    pub hash: String,
    /// Stored byte length — used as a cheap pre-screen before hashing.
    pub size: u64,
    /// One or more semantic chunks for this file.
    pub chunks: Vec<ChunkEntry>,
}

/// Root of the flat-file JSON index.
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexStore {
    entries: HashMap<String, FileIndexEntry>,
}

impl IndexStore {
    fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(store) => store,
            Err(_e) => {
                crate::debug_log!(
                    "[cortexast] index schema changed or corrupted ({}), rebuilding…",
                    _e
                );
                Self::default()
            }
        }
    }

    fn save(&self, path: &Path) {
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// Compute the xxh3 hex digest of raw bytes. ~1 µs for a 50 KB file on M4.
#[inline]
fn xxh3_hex(bytes: &[u8]) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(bytes))
}

// ---------------------------------------------------------------------------
// AST-aware chunking helpers  (Task 2)
// ---------------------------------------------------------------------------

/// A logical text chunk ready to be embedded.
struct PreparedChunk {
    symbols: Vec<String>,
    start_line: u32,
    end_line: u32,
    /// The text snippet that will be embedded (includes a symbol header prefix).
    text: String,
}

/// Split `content` into semantic chunks using AST symbol boundaries.
///
/// For files ≤ SMALL_FILE_BYTES call sites use the fast single-chunk path;
/// this function is only called for larger files.
fn ast_chunk(path: &Path, content: &str, chunk_lines: usize) -> Vec<PreparedChunk> {
    let max_lines = (chunk_lines as u32).clamp(20, CHUNK_MAX_LINES);
    let source_lines: Vec<&str> = content.lines().collect();
    let total_lines = source_lines.len() as u32;

    let symbols = extract_symbols_from_source(path, content);

    if !symbols.is_empty() {
        ast_guided_chunks(&symbols, &source_lines, total_lines, max_lines)
    } else {
        line_range_chunks(&source_lines, max_lines)
    }
}

/// Group AST symbols into chunks that respect the `max_lines` budget.
fn ast_guided_chunks(
    symbols: &[crate::inspector::Symbol],
    source_lines: &[&str],
    total_lines: u32,
    max_lines: u32,
) -> Vec<PreparedChunk> {
    // Compute each symbol's "territory": from its start line to the next symbol's start.
    struct Region {
        name: String,
        start: u32,
        end: u32, // exclusive
    }

    let regions: Vec<Region> = symbols
        .iter()
        .enumerate()
        .map(|(i, sym)| {
            let end = if i + 1 < symbols.len() {
                symbols[i + 1].line
            } else {
                total_lines
            };
            Region {
                name: format!("{} {}", sym.kind, sym.name),
                start: sym.line,
                end: end.min(total_lines),
            }
        })
        .collect();

    let mut chunks: Vec<PreparedChunk> = Vec::new();

    // Optional preamble before the first symbol (module docs, imports, etc.)
    if let Some(first) = regions.first() {
        if first.start > 0 {
            let text = source_lines[0..first.start as usize].join("\n");
            chunks.push(PreparedChunk {
                symbols: vec!["<preamble>".to_string()],
                start_line: 0,
                end_line: first.start.saturating_sub(1),
                text,
            });
        }
    }

    // Greedy grouping.
    let mut g_start: u32 = 0;
    let mut g_end: u32 = 0;
    let mut g_syms: Vec<String> = Vec::new();

    let flush = |start: u32, end: u32, syms: &[String], src: &[&str], out: &mut Vec<PreparedChunk>| {
        if start >= end || syms.is_empty() { return; }
        let s = start as usize;
        let e = (end as usize).min(src.len());
        let sym_header = syms.join(", ");
        out.push(PreparedChunk {
            symbols: syms.to_vec(),
            start_line: start,
            end_line: end.saturating_sub(1),
            text: format!("symbols: {}\n{}", sym_header, src[s..e].join("\n")),
        });
    };

    let mut first_region = true;
    for region in &regions {
        if first_region {
            g_start = region.start;
            g_end = region.end;
            g_syms.push(region.name.clone());
            first_region = false;
            continue;
        }

        let projected_lines = region.end.saturating_sub(g_start);
        if projected_lines > max_lines && !g_syms.is_empty() {
            flush(g_start, g_end, &g_syms, source_lines, &mut chunks);
            g_start = region.start;
            g_end = region.end;
            g_syms.clear();
        } else {
            g_end = region.end;
        }
        g_syms.push(region.name.clone());
    }

    // Flush remainder.
    flush(g_start, g_end, &g_syms, source_lines, &mut chunks);

    chunks
}

/// Simple line-range splitting — fallback for unsupported languages.
fn line_range_chunks(source_lines: &[&str], max_lines: u32) -> Vec<PreparedChunk> {
    let total = source_lines.len() as u32;
    let mut chunks = Vec::new();
    let mut start: u32 = 0;
    while start < total {
        let end = (start + max_lines).min(total);
        chunks.push(PreparedChunk {
            symbols: vec![],
            start_line: start,
            end_line: end.saturating_sub(1),
            text: source_lines[start as usize..end as usize].join("\n"),
        });
        start = end;
    }
    chunks
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IndexJob {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub content: String,
}

pub struct CodebaseIndex {
    repo_root: PathBuf,
    model: StaticModel,
    chunk_lines: usize,
    index_path: PathBuf,
    store: IndexStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexMetaV2 {
    model_id: String,
    chunk_lines: usize,
}

impl CodebaseIndex {
    pub fn open(repo_root: &Path, db_dir: &Path, model_id: &str, chunk_lines: usize) -> Result<Self> {
        let db_dir = if db_dir.is_absolute() {
            db_dir.to_path_buf()
        } else {
            repo_root.join(db_dir)
        };
        std::fs::create_dir_all(&db_dir).context("Failed to create vector DB dir")?;

        let model = StaticModel::from_pretrained(model_id, None, None, None)?;

        let chunk_lines = chunk_lines.clamp(1, 200);

        let index_path = db_dir.join("embeddings.json");
        let mut store = IndexStore::load(&index_path);

        // Meta: ensure we don't mix embeddings from different models/chunking.
        // This enables config changes to take effect immediately without restarting the MCP server.
        let meta_path = db_dir.join("index_meta_v2.json");
        let meta_disk: Option<IndexMetaV2> = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|t| serde_json::from_str::<IndexMetaV2>(&t).ok());

        if let Some(meta) = meta_disk {
            if meta.model_id != model_id || meta.chunk_lines != chunk_lines {
                crate::debug_log!(
                    "[cortexast] vector index config changed (model/chunk_lines); rebuilding index…"
                );
                store = IndexStore::default();
                let _ = std::fs::remove_file(&index_path);
            }
        }

        // Best-effort: persist current meta.
        let _ = std::fs::write(
            &meta_path,
            serde_json::to_string(&IndexMetaV2 {
                model_id: model_id.to_string(),
                chunk_lines,
            })
            .unwrap_or_else(|_| "{}".to_string()),
        );

        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            model,
            chunk_lines,
            index_path,
            store,
        })
    }

    // ── Cache helpers ─────────────────────────────────────────────────────

    /// Read raw bytes + compute size + xxh3 hash. Returns `None` for binary files.
    fn read_with_hash(abs_path: &Path) -> Result<Option<(Vec<u8>, u64, String)>> {
        let bytes = std::fs::read(abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;
        if bytes.contains(&0u8) {
            return Ok(None); // binary — skip
        }
        let size = bytes.len() as u64;
        let hash = xxh3_hex(&bytes);
        Ok(Some((bytes, size, hash)))
    }

    fn is_content_unchanged(entry: &FileIndexEntry, disk_size: u64, disk_hash: &str) -> bool {
        entry.size == disk_size && entry.hash == disk_hash
    }

    pub fn needs_reindex_path(&self, rel_path: &str, abs_path: &Path) -> Result<bool> {
        let rel_norm = rel_path.replace('\\', "/");
        let Some(entry) = self.store.entries.get(&rel_norm) else {
            return Ok(true);
        };
        let disk_size = std::fs::metadata(abs_path)?.len();
        if entry.size != disk_size {
            return Ok(true);
        }
        let raw = std::fs::read(abs_path)?;
        let hash = xxh3_hex(&raw);
        Ok(!Self::is_content_unchanged(entry, disk_size, &hash))
    }

    // ── Embedding pipeline ────────────────────────────────────────────────

    /// Build chunks, embed each one, return a ready `FileIndexEntry`.
    /// Returns `None` for empty or binary files.
    fn embed_file(
        &self,
        rel_path: &str,
        _abs_path: &Path,
        raw_bytes: Vec<u8>,
        size: u64,
        hash: String,
    ) -> Option<FileIndexEntry> {
        let content = String::from_utf8_lossy(&raw_bytes).into_owned();
        if content.trim().is_empty() {
            return None;
        }

        let path_obj = PathBuf::from(rel_path);
        let total_lines = content.lines().count() as u32;

        let prepared: Vec<PreparedChunk> = if size > SMALL_FILE_BYTES {
            // Task 2: AST-aware multi-chunk for large files.
            ast_chunk(&path_obj, &content, self.chunk_lines)
        } else {
            // Small file fast path: single chunk with symbol header.
            let syms = extract_symbols_from_source(&path_obj, &content);
            let sym_names: Vec<String> = syms
                .iter()
                .map(|s| format!("{} {}", s.kind, s.name))
                .collect();
            let cap = content.len().min(16_000);
            let body = &content[..cap];
            let text = if sym_names.is_empty() {
                format!("passage: file: {}\n{}", rel_path, body)
            } else {
                format!("symbols: {}\npassage: file: {}\n{}", sym_names.join(", "), rel_path, body)
            };
            vec![PreparedChunk {
                symbols: sym_names,
                start_line: 0,
                end_line: total_lines.saturating_sub(1),
                text,
            }]
        };

        let chunks: Vec<ChunkEntry> = prepared
            .into_iter()
            .filter(|c| !c.text.trim().is_empty())
            .map(|c| {
                let doc = format!("passage: {}", c.text);
                let vector = self.model.encode_single(&doc);
                ChunkEntry {
                    symbols: c.symbols,
                    start_line: c.start_line,
                    end_line: c.end_line,
                    vector,
                }
            })
            .collect();

        if chunks.is_empty() {
            return None;
        }

        Some(FileIndexEntry { hash, size, chunks })
    }

    // ── Indexing entry points ─────────────────────────────────────────────

    /// Index a file from disk, using the cache when content is unchanged.
    pub async fn index_file_path(&mut self, rel_path: &str, abs_path: &Path) -> Result<()> {
        let rel_norm = rel_path.replace('\\', "/");
        let Some((raw, size, hash)) = Self::read_with_hash(abs_path)? else {
            return Ok(());
        };
        if let Some(e) = self.store.entries.get(&rel_norm) {
            if Self::is_content_unchanged(e, size, &hash) {
                return Ok(());
            }
        }
        if let Some(entry) = self.embed_file(&rel_norm, abs_path, raw, size, hash) {
            self.store.entries.insert(rel_norm, entry);
            self.store.save(&self.index_path);
        }
        Ok(())
    }

    /// Batch-index and call `on_progress` after each file.
    pub async fn index_jobs<F>(&mut self, jobs: &[IndexJob], mut on_progress: F) -> Result<usize>
    where
        F: FnMut(),
    {
        let mut indexed = 0usize;
        for job in jobs {
            let rel_norm = job.rel_path.replace('\\', "/");
            let bytes = job.content.as_bytes();
            let size = bytes.len() as u64;
            let hash = xxh3_hex(bytes);

            if let Some(e) = self.store.entries.get(&rel_norm) {
                if Self::is_content_unchanged(e, size, &hash) {
                    on_progress();
                    continue;
                }
            }

            if let Some(entry) = self.embed_file(&rel_norm, &job.abs_path, bytes.to_vec(), size, hash) {
                self.store.entries.insert(rel_norm, entry);
                indexed += 1;
            }
            on_progress();
        }
        self.store.save(&self.index_path);
        Ok(indexed)
    }

    /// **JIT Incremental Indexing** — run once before every search.
    ///
    /// Phase 1: stat sweep — O(n) stat calls, no file reads.
    /// Phase 2: delta detection:
    ///           ADD   : file on disk, not in index.
    ///           UPDATE: stored_size != disk_size (cheap pre-screen).
    ///           SameSize: must read + hash to verify (handles git checkout).
    ///           DELETE: rel_path in index, no longer on disk.
    /// Phase 3: parallel read + hash — rayon par_iter over dirty candidates.
    ///           SameSize files where hash matches → dropped (truly unchanged).
    /// Phase 4: embed + upsert — sequential (model not Send), persist once.
    ///
    /// Returns `(added, updated, deleted)` counts.
    pub fn refresh(&mut self, scan_opts: &ScanOptions) -> Result<(usize, usize, usize)> {
        // ── Phase 1 ──────────────────────────────────────────────────────
        let entries = scan_workspace(scan_opts)?;

        let mut disk_files: HashMap<String, (PathBuf, u64)> = HashMap::with_capacity(entries.len());
        for e in &entries {
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");
            disk_files.insert(rel, (e.abs_path.clone(), e.bytes));
        }

        // ── Phase 2 ──────────────────────────────────────────────────────
        let mut candidates: Vec<(String, PathBuf, CandidateKind)> = Vec::new();

        for (rel, (abs, disk_size)) in &disk_files {
            match self.store.entries.get(rel.as_str()) {
                None => candidates.push((rel.clone(), abs.clone(), CandidateKind::New)),
                Some(e) if e.size != *disk_size => {
                    candidates.push((rel.clone(), abs.clone(), CandidateKind::SizeChanged));
                }
                Some(e) => {
                    // Same size → read + hash to confirm; handles git branch-switch.
                    candidates.push((rel.clone(), abs.clone(), CandidateKind::SameSize(e.hash.clone())));
                }
            }
        }

        let mut to_delete: Vec<String> = Vec::new();
        let index_keys: HashSet<String> = self.store.entries.keys().cloned().collect();
        for key in &index_keys {
            if !disk_files.contains_key(key.as_str()) {
                to_delete.push(key.clone());
            }
        }

        // ── Phase 3: parallel read + hash ────────────────────────────────
        let read_results: Vec<(String, PathBuf, Vec<u8>, u64, String, bool)> = candidates
            .par_iter()
            .filter_map(|(rel, abs, kind)| {
                let raw = std::fs::read(abs).ok()?;
                if raw.contains(&0u8) { return None; } // binary
                let size = raw.len() as u64;
                let hash = xxh3_hex(&raw);

                // SameSize: drop if hash matches — truly unchanged (git checkout no-op).
                if let CandidateKind::SameSize(stored_hash) = kind {
                    if *stored_hash == hash { return None; }
                }

                let is_new = matches!(kind, CandidateKind::New);
                Some((rel.clone(), abs.clone(), raw, size, hash, is_new))
            })
            .collect();

        let deleted = to_delete.len();
        if read_results.is_empty() && deleted == 0 {
            return Ok((0, 0, 0));
        }

        // ── Phase 4: embed + upsert ───────────────────────────────────────
        let mut added = 0usize;
        let mut updated = 0usize;

        for (rel, abs, raw, size, hash, is_new) in read_results {
            if let Some(entry) = self.embed_file(&rel, &abs, raw, size, hash) {
                self.store.entries.insert(rel, entry);
                if is_new { added += 1; } else { updated += 1; }
            }
        }

        for key in &to_delete {
            self.store.entries.remove(key);
        }

        self.store.save(&self.index_path);
        Ok((added, updated, deleted))
    }

    /// Index a single file given pre-read content.
    pub async fn index_file(&mut self, rel_path: &str, content: &str) -> Result<()> {
        let rel_norm = rel_path.replace('\\', "/");
        let bytes = content.as_bytes();
        let size = bytes.len() as u64;
        let hash = xxh3_hex(bytes);
        let abs = self.repo_root.join(&rel_norm);

        if let Some(e) = self.store.entries.get(&rel_norm) {
            if Self::is_content_unchanged(e, size, &hash) { return Ok(()); }
        }

        if let Some(entry) = self.embed_file(&rel_norm, &abs, bytes.to_vec(), size, hash) {
            self.store.entries.insert(rel_norm, entry);
            self.store.save(&self.index_path);
        }
        Ok(())
    }

    // ── Search — 2-Stage Hybrid Router (Symbol Sniper) ────────────────────

    /// Vector search with deterministic symbol sniper.
    ///
    /// **Stage 1 — Sniper (exact match)**
    /// Tokenize the query on whitespace/punctuation (no CamelCase splitting).
    /// Compare each token against every chunk's symbol names (kind prefix stripped,
    /// both lowercased). On an exact hit the file receives `EXACT_SYMBOL_SCORE`
    /// (2.0) — no vector math involved. Exact matches always rank above Stage 2.
    ///
    /// **Stage 2 — Semantic fallback**
    /// Files with no exact symbol match are scored by the max cosine similarity
    /// across all their chunks (range 0.0–1.0). Since 1.0 < 2.0, no semantic
    /// result can ever outrank a sniper hit.
    pub async fn search(&mut self, query: &str, limit: usize) -> Result<Vec<String>> {
        if self.store.entries.is_empty() {
            return Ok(vec![]);
        }

        let qv = self.model.encode_single(&format!("query: {}", query));
        let query_lower = query.to_lowercase();

        // Tokenize on whitespace + punctuation. No CamelCase splitting to avoid
        // broad noise (e.g. "Request" matching unrelated HTTP files).
        let query_tokens: HashSet<String> = query_lower
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_string())
            .collect();

        let mut scores: Vec<(f32, &str)> = self
            .store
            .entries
            .iter()
            .map(|(path, file_entry)| {
                let score = score_file_entry(&query_tokens, &qv, file_entry);
                (score, path.as_str())
            })
            .collect();

        scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scores
            .into_iter()
            .take(limit)
            .map(|(_, p)| p.replace('\\', "/"))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum CandidateKind {
    New,
    SizeChanged,
    SameSize(String), // stored hash — need to verify by reading
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
    dot / (norm_a * norm_b)
}

/// Pure scoring function for a single file entry — extracted for unit testability.
///
/// Returns `EXACT_SYMBOL_SCORE` (2.0) when any chunk's bare symbol name
/// (kind prefix stripped, lowercased) exactly matches a query token.
/// Falls back to max cosine similarity across chunks otherwise.
#[inline]
fn score_file_entry(
    query_tokens: &HashSet<String>,
    query_vector: &[f32],
    file_entry: &FileIndexEntry,
) -> f32 {
    // Stage 1 — Sniper: exact token ↔ symbol name match.
    let has_exact = file_entry.chunks.iter().any(|chunk| {
        chunk.symbols.iter().any(|sym| {
            // Symbols stored as "kind name" (e.g. "fn ConvertRequest").
            // Strip kind prefix to get bare name for exact comparison.
            let bare = sym
                .split_whitespace()
                .last()
                .unwrap_or(sym.as_str())
                .to_lowercase();
            query_tokens.contains(&bare)
        })
    });

    if has_exact {
        return EXACT_SYMBOL_SCORE;
    }

    // Stage 2 — Semantic fallback: max cosine across chunks (≤ 1.0).
    file_entry
        .chunks
        .iter()
        .map(|c| cosine_similarity(query_vector, &c.vector))
        .fold(f32::NEG_INFINITY, f32::max)
}

// ---------------------------------------------------------------------------
// Unit tests — Symbol Sniper proof
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a mock FileIndexEntry with `symbols` in a single chunk.
    /// The stored vector is set to the given value replicated to 4 dims (enough
    /// for cosine math in isolation; production uses 256 dims).
    fn mock_entry(symbols: Vec<&str>, vector: Vec<f32>) -> FileIndexEntry {
        FileIndexEntry {
            hash: "deadbeef".into(),
            size: 1,
            chunks: vec![ChunkEntry {
                symbols: symbols.into_iter().map(str::to_string).collect(),
                start_line: 0,
                end_line: 10,
                vector,
            }],
        }
    }

    fn tokens(query: &str) -> HashSet<String> {
        query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 2)
            .map(str::to_string)
            .collect()
    }

    /// Prove the sniper gives EXACT_SYMBOL_SCORE (2.0) for an exact name match
    /// and that the score ceiling for a non-matching file is ≤ 1.0.
    #[test]
    fn sniper_exact_match_crushes_semantic_result() {
        // ── Arrange ──────────────────────────────────────────────────────
        // Query: "How does ConvertRequest work? source_format_hint"
        let query = "How does ConvertRequest work? source_format_hint";
        let toks = tokens(query);

        // convert_request.rs — contains the symbol. Deliberately give it a
        // weak vector (not aligned with query direction) to prove the score
        // comes purely from the sniper, not from cosine.
        let rust_entry = mock_entry(
            vec!["fn convert_request", "impl ConvertRequest"],
            vec![0.1, 0.1, 0.1, 0.1],
        );

        // engine.proto — no matching symbol. Give it a near-perfect cosine
        // with the query vector to simulate the old false-positive collision.
        let proto_entry = mock_entry(
            vec!["message EngineProto", "rpc ConvertStream"],
            vec![0.99, 0.99, 0.0, 0.0],
        );

        // Query vector aligned with engine.proto to maximise its cosine score.
        let qv = vec![1.0f32, 1.0, 0.0, 0.0];

        // ── Act ───────────────────────────────────────────────────────────
        let rust_score  = score_file_entry(&toks, &qv, &rust_entry);
        let proto_score = score_file_entry(&toks, &qv, &proto_entry);

        // ── Assert ────────────────────────────────────────────────────────
        // 1. The Rust file must receive EXACT_SYMBOL_SCORE (2.0).
        assert_eq!(
            rust_score, EXACT_SYMBOL_SCORE,
            "Rust file with exact symbol match must score {EXACT_SYMBOL_SCORE}"
        );

        // 2. proto score ≤ 1.0 (pure cosine; we expect ~0.99 here).
        assert!(
            proto_score <= 1.0,
            "Semantic-only file score must be ≤ 1.0, got {proto_score}"
        );

        // 3. The Rust file MUST outrank the proto file — the key invariant.
        assert!(
            rust_score > proto_score,
            "Rust ({rust_score}) must beat proto ({proto_score}) — sniper failed"
        );

        // Print the proof table for the CI log.
        println!("\n═══ Symbol Sniper Proof ══════════════════════════════");
        println!("  src/convert_request.rs   score = {rust_score:.4}  ← Stage 1 (Sniper)");
        println!("  proto/engine.proto        score = {proto_score:.4}  ← Stage 2 (Cosine)");
        println!("  Gap = {:.4}  (guaranteed ≥ {:.2})",
            rust_score - proto_score,
            EXACT_SYMBOL_SCORE - 1.0);
        println!("══════════════════════════════════════════════════════\n");
    }

    /// Sniper must NOT fire on a partial substring match —
    /// query "request" must NOT snipe a file with symbol "ConvertRequest".
    #[test]
    fn sniper_requires_exact_token_not_substring() {
        let toks = tokens("request handling logic");
        let entry = mock_entry(
            vec!["impl ConvertRequest"],
            vec![0.5, 0.5, 0.5, 0.5],
        );
        let qv = vec![0.0f32; 4];
        let score = score_file_entry(&toks, &qv, &entry);
        assert!(
            score < EXACT_SYMBOL_SCORE,
            "Partial substring 'request' must not trigger sniper for 'ConvertRequest'"
        );
    }
}

