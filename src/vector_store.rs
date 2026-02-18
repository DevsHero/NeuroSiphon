use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::scanner::{scan_workspace, ScanOptions};

// ---------------------------------------------------------------------------
// Lightweight flat-file vector index — no external database required.
//
// Storage layout:  <db_dir>/embeddings.json
//   { "entries": { "<rel_path>": { "size": u64, "modified_ns": u128|null,
//                                   "embedding": [f32, ...] } } }
//
// Search: brute-force cosine similarity (O(n × d), n ≤ 400, d ≈ 256 → trivial).
//
// JIT Incremental Indexing:
//   Call `index.refresh(scan_opts)` before every search.
//   It does a fast mtime stat-sweep (no file reads), detects add/update/delete,
//   then parallel-reads + embeds only the dirty delta, and persists once.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileIndexMeta {
    size: u64,
    modified_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    #[serde(flatten)]
    meta: FileIndexMeta,
    embedding: Vec<f32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexStore {
    entries: HashMap<String, IndexEntry>,
}

impl IndexStore {
    fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::default(), // file doesn't exist yet
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(store) => store,
            Err(_e) => {
                crate::debug_log!(
                    "[neurosiphon] index corrupted ({}), rebuilding from scratch…",
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

impl CodebaseIndex {
    pub fn open(repo_root: &Path, db_dir: &Path, model_id: &str, chunk_lines: usize) -> Result<Self> {
        let db_dir = if db_dir.is_absolute() {
            db_dir.to_path_buf()
        } else {
            repo_root.join(db_dir)
        };
        std::fs::create_dir_all(&db_dir).context("Failed to create vector DB dir")?;

        // Model2Vec: static embeddings via HuggingFace Hub (no ONNX runtime needed).
        let model = StaticModel::from_pretrained(model_id, None, None, None)?;

        let index_path = db_dir.join("embeddings.json");
        let store = IndexStore::load(&index_path);

        // Migrate: remove old separate meta file if it exists.
        let _ = std::fs::remove_file(db_dir.join("index_meta.json"));

        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            model,
            chunk_lines: chunk_lines.clamp(1, 200),
            index_path,
            store,
        })
    }

    fn file_meta(abs_path: &Path) -> Result<FileIndexMeta> {
        let m = std::fs::metadata(abs_path)
            .with_context(|| format!("Failed to stat {}", abs_path.display()))?;
        let modified_ns = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos());
        Ok(FileIndexMeta { size: m.len(), modified_ns })
    }

    fn should_reindex(&self, rel_path: &str, new_meta: &FileIndexMeta) -> bool {
        match self.store.entries.get(rel_path) {
            None => true,
            Some(e) => e.meta.size != new_meta.size || e.meta.modified_ns != new_meta.modified_ns,
        }
    }

    pub fn needs_reindex_path(&self, rel_path: &str, abs_path: &Path) -> Result<bool> {
        let rel_norm = rel_path.replace('\\', "/");
        let new_meta = Self::file_meta(abs_path)?;
        Ok(self.should_reindex(&rel_norm, &new_meta))
    }

    fn snippet_by_lines(content: &str, max_lines: usize) -> String {
        let max_lines = max_lines.clamp(5, 300);
        let mut out = String::new();
        for (i, line) in content.lines().take(max_lines).enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
            if out.len() >= 16_000 {
                break;
            }
        }
        out
    }

    fn read_file_lossy(abs_path: &Path) -> Result<Option<String>> {
        let bytes = std::fs::read(abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;
        // Binary detection: null bytes → skip embedding entirely.
        if bytes.contains(&0u8) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Index a file from disk, re-using the cached embedding when nothing changed.
    pub async fn index_file_path(&mut self, rel_path: &str, abs_path: &Path) -> Result<()> {
        let new_meta = Self::file_meta(abs_path)?;
        let rel_norm = rel_path.replace('\\', "/");
        if !self.should_reindex(&rel_norm, &new_meta) {
            return Ok(());
        }
        let content = match Self::read_file_lossy(abs_path)? {
            Some(c) => c,
            None => return Ok(()), // binary file — skip embedding
        };
        let snippet = Self::snippet_by_lines(&content, self.chunk_lines);
        if snippet.trim().is_empty() {
            return Ok(());
        }
        let doc = format!("passage: file: {}\n{}", rel_norm, snippet);
        let embedding = self.model.encode_single(&doc);
        self.store.entries.insert(rel_norm, IndexEntry { meta: new_meta, embedding });
        self.store.save(&self.index_path);
        Ok(())
    }

    /// Batch-index multiple files and call `on_progress` after each one.
    ///
    /// Returns the number of files that were actually re-embedded.
    pub async fn index_jobs<F>(&mut self, jobs: &[IndexJob], mut on_progress: F) -> Result<usize>
    where
        F: FnMut(),
    {
        let mut indexed = 0usize;

        for job in jobs {
            let new_meta = match Self::file_meta(&job.abs_path) {
                Ok(m) => m,
                Err(_) => {
                    on_progress();
                    continue;
                }
            };
            let rel_norm = job.rel_path.replace('\\', "/");
            if !self.should_reindex(&rel_norm, &new_meta) {
                on_progress();
                continue;
            }
            let snippet = Self::snippet_by_lines(&job.content, self.chunk_lines);
            if snippet.trim().is_empty() {
                on_progress();
                continue;
            }

            let doc = format!("passage: file: {}\n{}", rel_norm, snippet);
            let embedding = self.model.encode_single(&doc);
            self.store.entries.insert(rel_norm, IndexEntry { meta: new_meta, embedding });
            indexed += 1;
            on_progress();
        }

        self.store.save(&self.index_path);
        Ok(indexed)
    }

    /// **JIT Incremental Indexing** — run this once right before every search.
    ///
    /// Algorithm (all I/O is parallelized via rayon):
    ///  1. Fast stat-sweep: walk `scan_opts` target, collect mtime/size for every file.
    ///     No file *reads* in this phase — just OS metadata (≈10-50 ms for 10k files).
    ///  2. Delta detection:
    ///     - ADD   : file on disk, NOT in index
    ///     - UPDATE: file on disk, mtime or size differs from index entry
    ///     - DELETE: rel_path in index, file no longer on disk
    ///  3. Parallel read + embed dirty files (rayon par_iter).
    ///  4. Apply deletes, apply upserts, persist to disk ONCE.
    ///
    /// Returns `(added, updated, deleted)` counts for display.
    pub fn refresh(&mut self, scan_opts: &ScanOptions) -> Result<(usize, usize, usize)> {
        // ── Phase 1: fast stat sweep ─────────────────────────────────────
        let entries = scan_workspace(scan_opts)?;

        // Build a set of all current rel_paths on disk.
        let mut disk_files: HashMap<String, (PathBuf, FileIndexMeta)> =
            HashMap::with_capacity(entries.len());

        for e in &entries {
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");
            let meta = match Self::file_meta(&e.abs_path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            disk_files.insert(rel, (e.abs_path.clone(), meta));
        }

        // ── Phase 2: delta detection ─────────────────────────────────────
        let mut to_add: Vec<(String, PathBuf)> = Vec::new();
        let mut to_update: Vec<(String, PathBuf)> = Vec::new();
        let mut to_delete: Vec<String> = Vec::new();

        // Add / update
        for (rel, (abs, new_meta)) in &disk_files {
            if self.should_reindex(rel, new_meta) {
                if self.store.entries.contains_key(rel.as_str()) {
                    to_update.push((rel.clone(), abs.clone()));
                } else {
                    to_add.push((rel.clone(), abs.clone()));
                }
            }
        }

        // Delete (in index but gone from disk)
        let index_keys: HashSet<String> = self.store.entries.keys().cloned().collect();
        for key in &index_keys {
            if !disk_files.contains_key(key.as_str()) {
                to_delete.push(key.clone());
            }
        }

        let added = to_add.len();
        let updated = to_update.len();
        let deleted = to_delete.len();

        // Early exit when nothing changed.
        if added == 0 && updated == 0 && deleted == 0 {
            return Ok((0, 0, 0));
        }

        // ── Phase 3: parallel read dirty files ───────────────────────────
        let dirty: Vec<(String, PathBuf)> = to_add.into_iter().chain(to_update).collect();

        let read_results: Vec<(String, PathBuf, String)> = dirty
            .par_iter()
            .filter_map(|(rel, abs)| {
                let content = Self::read_file_lossy(abs).ok()??; // None = binary, skip
                Some((rel.clone(), abs.clone(), content))
            })
            .collect();

        // ── Phase 4: embed + upsert (sequential, model is not Send) ──────
        for (rel, abs, content) in read_results {
            let new_meta = match Self::file_meta(&abs) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let snippet = Self::snippet_by_lines(&content, self.chunk_lines);
            if snippet.trim().is_empty() {
                continue;
            }
            let doc = format!("passage: file: {}\n{}", rel, snippet);
            let embedding = self.model.encode_single(&doc);
            self.store.entries.insert(rel, IndexEntry { meta: new_meta, embedding });
        }

        // Apply deletes.
        for key in &to_delete {
            self.store.entries.remove(key);
        }

        // Persist once.
        self.store.save(&self.index_path);

        Ok((added, updated, deleted))
    }

    /// Index a single file given its content (used for incremental updates).
    pub async fn index_file(&mut self, rel_path: &str, content: &str) -> Result<()> {
        let abs = self.repo_root.join(rel_path);
        let new_meta = Self::file_meta(&abs)?;
        if !self.should_reindex(rel_path, &new_meta) {
            return Ok(());
        }
        let snippet = Self::snippet_by_lines(content, self.chunk_lines);
        if snippet.trim().is_empty() {
            return Ok(());
        }
        let doc = format!("passage: file: {}\n{}", rel_path, snippet);
        let embedding = self.model.encode_single(&doc);
        self.store.entries.insert(rel_path.to_string(), IndexEntry { meta: new_meta, embedding });
        self.store.save(&self.index_path);
        Ok(())
    }

    /// Vector search: returns up to `limit` file paths sorted by cosine similarity.
    pub async fn search(&mut self, query: &str, limit: usize) -> Result<Vec<String>> {
        if self.store.entries.is_empty() {
            return Ok(vec![]);
        }
        let q = format!("query: {}", query);
        let qv = self.model.encode_single(&q);

        let mut scores: Vec<(f32, &str)> = self
            .store
            .entries
            .iter()
            .map(|(path, entry)| (cosine_similarity(&qv, &entry.embedding), path.as_str()))
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
// Math helpers
// ---------------------------------------------------------------------------

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
