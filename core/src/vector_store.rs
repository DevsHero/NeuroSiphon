use anyhow::{Context, Result};
use arrow_array::{Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;
use lancedb::query::QueryBase;
use model2vec_rs::model::StaticModel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// NOTE: LanceDB Rust APIs have shifted across minor versions.
// This module is implemented to be compiled against the workspace's resolved `lancedb` crate.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileIndexMeta {
    size: u64,
    modified_ns: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct IndexJob {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub content: String,
}

pub struct CodebaseIndex {
    repo_root: PathBuf,
    db_dir: PathBuf,
    model: StaticModel,
    chunk_lines: usize,
    meta_path: PathBuf,
    meta: HashMap<String, FileIndexMeta>,
}

impl CodebaseIndex {
    pub fn open(repo_root: &Path, db_dir: &Path, model_id: &str, chunk_lines: usize) -> Result<Self> {
        let db_dir = if db_dir.is_absolute() {
            db_dir.to_path_buf()
        } else {
            repo_root.join(db_dir)
        };
        std::fs::create_dir_all(&db_dir).context("Failed to create vector DB dir")?;

        // Model2Vec-RS: downloads from HuggingFace Hub via hf-hub (no ONNX runtime).
        let model = StaticModel::from_pretrained(model_id, None, None, None)?;

        let meta_path = db_dir.join("index_meta.json");
        let meta = if let Ok(text) = std::fs::read_to_string(&meta_path) {
            serde_json::from_str::<HashMap<String, FileIndexMeta>>(&text).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            db_dir,
            model,
            chunk_lines: chunk_lines.clamp(1, 200),
            meta_path,
            meta,
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
        Ok(FileIndexMeta {
            size: m.len(),
            modified_ns,
        })
    }

    fn should_reindex(&self, rel_path: &str, new_meta: &FileIndexMeta) -> bool {
        let Some(old) = self.meta.get(rel_path) else {
            return true;
        };
        old.size != new_meta.size || old.modified_ns != new_meta.modified_ns
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

    fn read_file_lossy(abs_path: &Path) -> Result<String> {
        let bytes = std::fs::read(abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;
        Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string()))
    }

    /// Index a file from disk, reading content only if it changed.
    pub async fn index_file_path(&mut self, rel_path: &str, abs_path: &Path) -> Result<()> {
        let new_meta = Self::file_meta(abs_path)?;
        let rel_norm = rel_path.replace('\\', "/");
        if !self.should_reindex(&rel_norm, &new_meta) {
            return Ok(());
        }
        let content = Self::read_file_lossy(abs_path)?;
        self.index_file_with_meta(&rel_norm, &content, new_meta).await
    }

    /// Index multiple files with a single DB connection/table.
    ///
    /// Returns number of files actually re-indexed.
    pub async fn index_jobs<F>(&mut self, jobs: &[IndexJob], mut on_progress: F) -> Result<usize>
    where
        F: FnMut(),
    {
        if jobs.is_empty() {
            return Ok(0);
        }

        let db = lancedb::connect(self.db_dir.to_string_lossy().as_ref())
            .execute()
            .await?;

        let table_name = "code_files";
        let mut table = (db.open_table(table_name).execute().await).ok();

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

            let escaped = rel_norm.replace('"', "\\\"");
            let snippet = Self::snippet_by_lines(&job.content, self.chunk_lines);
            if snippet.trim().is_empty() {
                on_progress();
                continue;
            }


            let doc = format!("passage: file: {}\n{}", escaped, snippet);
            let embedding = self.model.encode_single(&doc);
            let contents = vec![snippet];
            let embeddings = vec![embedding];

            if table.is_none() {
                let batch_reader = rows_to_record_batch_reader(&escaped, &contents, &embeddings)?;
                table = Some(db.create_table(table_name, batch_reader).execute().await?);
            }

            let Some(t) = table.as_ref() else {
                on_progress();
                continue;
            };

            // Delete old rows for this file (best-effort).
            let _ = t.delete(&format!("path = \"{}\"", escaped)).await;

            // Insert rows.
            let batch_reader = rows_to_record_batch_reader(&escaped, &contents, &embeddings)?;
            t.add(batch_reader).execute().await?;

            self.meta.insert(escaped, new_meta);
            indexed += 1;
            on_progress();
        }

        self.persist_meta();
        Ok(indexed)
    }

    async fn index_file_with_meta(
        &mut self,
        rel_path: &str,
        content: &str,
        new_meta: FileIndexMeta,
    ) -> Result<()> {
        let escaped = rel_path.replace('\\', "/").replace('"', "\\\"");
        let snippet = Self::snippet_by_lines(content, self.chunk_lines);
        if snippet.trim().is_empty() {
            return Ok(());
        }

        // Embed chunks.
        let doc = format!("passage: file: {}\n{}", escaped, snippet);
        let embedding = self.model.encode_single(&doc);
        let contents = vec![snippet];
        let embeddings = vec![embedding];

        // Open/create DB + table.
        let db = lancedb::connect(self.db_dir.to_string_lossy().as_ref())
            .execute()
            .await?;

        // Create table if missing; otherwise open.
        let table_name = "code_files";
        let table = match db.open_table(table_name).execute().await {
            Ok(t) => t,
            Err(_) => {
                // Create from the first batch so the schema is set.
                let batch_reader = rows_to_record_batch_reader(&escaped, &contents, &embeddings)?;
                db.create_table(table_name, batch_reader).execute().await?
            }
        };

        // Delete old rows for this file (best-effort).
        let _ = table.delete(&format!("path = \"{}\"", escaped)).await;

        // Insert rows.
        let batch_reader = rows_to_record_batch_reader(&escaped, &contents, &embeddings)?;
        table.add(batch_reader).execute().await?;

        self.meta.insert(escaped, new_meta);
        self.persist_meta();
        Ok(())
    }

    fn persist_meta(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.meta) {
            let _ = std::fs::write(&self.meta_path, text);
        }
    }

    /// Index a file into the local vector DB.
    ///
    /// This is best-effort: if DB operations fail, we still keep meta updates conservative.
    pub async fn index_file(&mut self, rel_path: &str, content: &str) -> Result<()> {
        let abs = self.repo_root.join(rel_path);
        let new_meta = Self::file_meta(&abs)?;
        if !self.should_reindex(rel_path, &new_meta) {
            return Ok(());
        }

        self.index_file_with_meta(rel_path, content, new_meta).await
    }

    /// Search the codebase with a vector query and return unique file paths by best-match chunk.
    pub async fn search(&mut self, query: &str, limit: usize) -> Result<Vec<String>> {
        let db = lancedb::connect(self.db_dir.to_string_lossy().as_ref())
            .execute()
            .await?;
        let table = db
            .open_table("code_files")
            .execute()
            .await
            .context("Vector index not built yet")?;

        let q = format!("query: {}", query);
        let qv = self.model.encode_single(&q);

        // Request more chunks than files, then dedupe by path.
        let k = (limit.max(1) * 2).min(500);
        let mut stream = table
            .vector_search(qv)?
            .column("vector")
            .limit(k)
            .select(lancedb::query::Select::columns(&["path"]))
            .execute()
            .await?;

        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut out: Vec<String> = Vec::new();

        while let Some(batch) = stream.try_next().await? {
            let schema = batch.schema();
            let idx = schema.index_of("path").context("Missing 'path' column in search results")?;
            let col = batch.column(idx);
            let Some(arr) = col.as_any().downcast_ref::<StringArray>() else {
                continue;
            };
            for i in 0..arr.len() {
                if !arr.is_valid(i) {
                    continue;
                }
                let p = arr.value(i).replace('\\', "/");
                if seen.insert(p.clone(), ()).is_none() {
                    out.push(p);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }

        Ok(out)
    }
}

fn rows_to_record_batch_reader(
    path: &str,
    chunks: &[String],
    embeddings: &[Vec<f32>],
) -> Result<Box<dyn arrow_array::RecordBatchReader + Send>> {
    if chunks.len() != embeddings.len() {
        return Err(anyhow::anyhow!("chunks/embeddings length mismatch"));
    }

    if chunks.is_empty() {
        return Err(anyhow::anyhow!("no chunks"));
    }

    let dim = embeddings[0].len();
    if dim == 0 {
        return Err(anyhow::anyhow!("empty embedding vector"));
    }

    // Flatten vectors.
    let mut flat: Vec<f32> = Vec::with_capacity(chunks.len() * dim);
    for v in embeddings {
        if v.len() != dim {
            return Err(anyhow::anyhow!("embedding dimension mismatch"));
        }
        flat.extend_from_slice(v);
    }

    let ids: Vec<String> = (0..chunks.len()).map(|i| format!("{}::{}", path, i)).collect();
    let paths: Vec<&str> = std::iter::repeat_n(path, chunks.len()).collect();

    let id_arr: ArrayRef = Arc::new(StringArray::from(ids));
    let path_arr: ArrayRef = Arc::new(StringArray::from(paths));
    let content_arr: ArrayRef = Arc::new(StringArray::from(chunks.to_vec()));

    let values: ArrayRef = Arc::new(Float32Array::from(flat));
    let item_field = Arc::new(Field::new("item", DataType::Float32, false));
    let vector_arr: ArrayRef = Arc::new(FixedSizeListArray::try_new(
        item_field,
        dim as i32,
        values,
        None,
    )?);

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), dim as i32),
            false,
        ),
    ]));

    let batch = RecordBatch::try_new(schema.clone(), vec![id_arr, path_arr, content_arr, vector_arr])?;
    let iter = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    Ok(Box::new(iter))
}
