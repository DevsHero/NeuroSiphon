use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use context_slicer::config::load_config;
use context_slicer::inspector::analyze_file;
use context_slicer::inspector::render_skeleton;
use context_slicer::mapper::{build_map_from_manifests, build_module_graph, build_repo_map, build_repo_map_scoped};
use context_slicer::server::run_stdio_server;
use context_slicer::slicer::{slice_paths_to_xml, slice_to_xml};
use context_slicer::scanner::{scan_workspace, ScanOptions};
use context_slicer::vector_store::{CodebaseIndex, IndexJob};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "context-slicer")]
#[command(version = "0.1.0")]
#[command(about = "High-performance context slicer (Rust)")]
struct Cli {
    /// Output a repo map JSON to stdout (nodes + edges)
    #[arg(long)]
    map: bool,

    /// Output a high-level module dependency graph (nodes=modules, edges=imports). Optional ROOT scopes scanning.
    #[arg(long, value_name = "ROOT", num_args = 0..=1, default_missing_value = ".")]
    graph_modules: Option<PathBuf>,

    /// Build a module graph strictly from the directories containing these manifest files.
    /// Example: --manifests apps/a/package.json libs/b/Cargo.toml
    #[arg(long, num_args = 1.., value_name = "MANIFEST_PATHS")]
    manifests: Option<Vec<PathBuf>>,

    /// Optional subdirectory path to scope mapping (only valid with --map)
    #[arg(value_name = "SUBDIR_PATH", requires = "map")]
    map_target: Option<PathBuf>,

    /// Inspect a single file and output extracted symbols as JSON
    #[arg(long, value_name = "FILE_PATH")]
    inspect: Option<PathBuf>,

    /// Output a pruned "skeleton" view of a single file (function bodies replaced with /* ... */)
    #[arg(long, value_name = "FILE_PATH")]
    skeleton: Option<PathBuf>,

    /// Target module/directory path (relative to repo root)
    #[arg(long, short = 't')]
    target: Option<PathBuf>,


    /// Vector search query; when present, runs local hybrid search and slices only the most relevant files.
    #[arg(long, value_name = "TEXT")]
    query: Option<String>,

    /// Max number of files returned from vector search (deduped by path).
    /// If omitted, a default / auto-tuned value is used.
    #[arg(long)]
    query_limit: Option<usize>,

    /// Override the embedding model repo ID (HuggingFace) used by Model2Vec-RS.
    /// Example: minishlab/potion-retrieval-32M
    #[arg(long, value_name = "MODEL_ID")]
    embed_model: Option<String>,

    /// Override snippet size (lines per file) when building the vector index.
    #[arg(long, value_name = "N")]
    chunk_lines: Option<usize>,
    /// Output XML to stdout (also writes .context-slicer/active_context.xml)
    #[arg(long)]
    xml: bool,

    /// Disable skeleton mode (emit full file contents into XML)
    #[arg(long)]
    full: bool,

    /// Token budget override
    #[arg(long, default_value_t = 32_000)]
    budget_tokens: usize,

    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start MCP stdio server
    Mcp,
}

fn auto_query_limit(budget_tokens: usize, entry_count: usize, configured_default: usize) -> usize {
    // Heuristic: with skeleton mode + aggressive cleanup, many repos can fit ~1k-2k tokens/file.
    // We use a conservative curve and then cap by scanned file count.
    let budget_based = (budget_tokens / 1_500).clamp(8, 60);
    let mut out = configured_default.min(budget_based);
    if entry_count > 0 {
        out = out.min(entry_count);
    }
    out.max(1)
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 2)
        .collect()
}

fn is_code_like_path(rel_path: &str) -> bool {
    let p = rel_path.to_ascii_lowercase();
    p.ends_with(".rs")
        || p.ends_with(".ts")
        || p.ends_with(".tsx")
        || p.ends_with(".js")
        || p.ends_with(".jsx")
        || p.ends_with(".py")
        || p.ends_with(".go")
        || p.ends_with(".java")
        || p.ends_with(".cs")
        || p.ends_with(".php")
        || p.ends_with(".kt")
        || p.ends_with(".swift")
        || p.ends_with(".c")
        || p.ends_with(".cc")
        || p.ends_with(".cpp")
        || p.ends_with(".h")
        || p.ends_with(".hpp")
        || p.ends_with(".md")
        || p.ends_with(".toml")
        || p.ends_with(".yaml")
        || p.ends_with(".yml")
        || p.ends_with(".json")
}

fn score_path_for_query(rel_path: &str, terms: &[String]) -> i32 {
    let p = rel_path.to_ascii_lowercase();
    let filename = p.rsplit('/').next().unwrap_or(&p);
    let mut score = 0i32;
    for t in terms {
        if t.is_empty() {
            continue;
        }
        if filename.contains(t) {
            score += 30;
        } else if p.contains(t) {
            score += 10;
        }
    }
    if is_code_like_path(&p) {
        score += 1;
    }
    score
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if matches!(cli.cmd, Some(Command::Mcp)) {
        return run_stdio_server();
    }

    let repo_root = std::env::current_dir().context("Failed to get current dir")?;

    if let Some(manifests) = cli.manifests.as_ref() {
        let graph = build_map_from_manifests(&repo_root, manifests)?;
        println!("{}", serde_json::to_string(&graph)?);
        return Ok(());
    }

    if let Some(root) = cli.graph_modules.as_ref() {
        let graph = build_module_graph(&repo_root, root)?;
        println!("{}", serde_json::to_string(&graph)?);
        return Ok(());
    }

    if let Some(p) = cli.inspect {
        let abs = if p.is_absolute() { p } else { repo_root.join(&p) };
        let mut out = analyze_file(&abs)?;
        // Prefer repo-relative file path in JSON output.
        if let Ok(rel) = abs.strip_prefix(&repo_root) {
            out.file = rel.to_string_lossy().replace('\\', "/");
        } else {
            out.file = abs.to_string_lossy().replace('\\', "/");
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if let Some(p) = cli.skeleton {
        let abs = if p.is_absolute() { p } else { repo_root.join(&p) };
        let skel = render_skeleton(&abs)?;
        print!("{}", skel);
        return Ok(());
    }

    if cli.map {
        let map = if let Some(scope) = cli.map_target.as_ref() {
            build_repo_map_scoped(&repo_root, scope)?
        } else {
            build_repo_map(&repo_root)?
        };
        println!("{}", serde_json::to_string(&map)?);
        return Ok(());
    }

    let mut cfg = load_config(&repo_root);
    if cli.full {
        cfg.skeleton_mode = false;
    }

    // Hybrid search mode: build/update local vector index, retrieve relevant files, then slice only those.
    let (xml, target_label) = if let Some(q) = cli.query.as_ref() {
        let index_target = cli.target.clone().unwrap_or_else(|| PathBuf::from("."));
        let opts = ScanOptions {
            repo_root: repo_root.clone(),
            target: index_target.clone(),
            max_file_bytes: cfg.token_estimator.max_file_bytes,
            exclude_dir_names: vec![
                ".git".into(),
                "node_modules".into(),
                "dist".into(),
                "target".into(),
                cfg.output_dir.to_string_lossy().to_string(),
            ],
        };

        let scan_spinner = ProgressBar::new_spinner();
        scan_spinner.set_style(
            ProgressStyle::with_template("{spinner} scanning files...")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        scan_spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        let entries = scan_workspace(&opts)?;
        scan_spinner.finish_with_message(format!("scanned {} files", entries.len()));

        let db_dir = cfg.output_dir.join("db");
        let model_id = cli
            .embed_model
            .as_deref()
            .unwrap_or(cfg.vector_search.model.as_str());
        let chunk_lines = cli.chunk_lines.unwrap_or(cfg.vector_search.chunk_lines);

        let model_spinner = ProgressBar::new_spinner();
        model_spinner.set_style(
            ProgressStyle::with_template("{spinner} loading embedding model...")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        model_spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        let mut index = CodebaseIndex::open(&repo_root, &db_dir, model_id, chunk_lines)?;
        model_spinner.finish_with_message("model ready".to_string());

        // Run async indexing + search on a small runtime.
        let rt = tokio::runtime::Runtime::new()?;
        let q_owned = q.clone();
        let limit = cli
            .query_limit
            .unwrap_or_else(|| auto_query_limit(cli.budget_tokens, entries.len(), cfg.vector_search.default_query_limit));

        // Candidate cap: avoid indexing the entire repo on every query.
        // Keep it small (hundreds) for responsiveness; the index grows incrementally over time.
        let max_candidates = (limit * 12).clamp(80, 400);
        let terms = query_terms(&q_owned);

        let rel_paths: Vec<String> = rt.block_on(async move {
            // Incrementally index only changed files.
            // Step 1: decide which files need reindexing (cheap metadata checks).
            let mut to_index: Vec<(String, PathBuf)> = Vec::new();

            // Score paths and take the top-N candidates.
            let mut scored: Vec<(i32, usize)> = Vec::with_capacity(entries.len());
            for (i, e) in entries.iter().enumerate() {
                let rel = e.rel_path.to_string_lossy().replace('\\', "/");
                let s = score_path_for_query(&rel, &terms);
                scored.push((s, i));
            }
            scored.sort_by(|(sa, ia), (sb, ib)| {
                // Desc score, then smaller files first.
                sb.cmp(sa)
                    .then_with(|| entries[*ia].bytes.cmp(&entries[*ib].bytes))
                    .then_with(|| entries[*ia].rel_path.cmp(&entries[*ib].rel_path))
            });

            for (_score, idx) in scored.into_iter().take(max_candidates) {
                let e = &entries[idx];
                let rel = e.rel_path.to_string_lossy().replace('\\', "/");
                if matches!(index.needs_reindex_path(&rel, &e.abs_path), Ok(true)) {
                    to_index.push((rel, e.abs_path.clone()));
                }
            }

            // Step 2: read file contents in parallel (I/O bound), then index with one DB connection.
            let jobs: Vec<IndexJob> = to_index
                .par_iter()
                .filter_map(|(rel, abs)| {
                    let bytes = std::fs::read(abs).ok()?;
                    let content = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
                    Some(IndexJob {
                        rel_path: rel.clone(),
                        abs_path: abs.clone(),
                        content,
                    })
                })
                .collect();

            let pb = ProgressBar::new(jobs.len() as u64);
            pb.set_style(
                ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} indexed")
                    .unwrap()
                    .progress_chars("=>-"),
            );
            let pb2 = pb.clone();
            let _ = index.index_jobs(&jobs, move || pb2.inc(1)).await;
            pb.finish_and_clear();

            (index.search(&q_owned, limit).await).unwrap_or_default()
        });

        let (xml, _meta) = if rel_paths.is_empty() {
            slice_to_xml(&repo_root, &index_target, cli.budget_tokens, &cfg)?
        } else {
            slice_paths_to_xml(&repo_root, &rel_paths, cli.budget_tokens, &cfg)?
        };
        (xml, format!("query:{}", q))
    } else {
        let target = cli.target.clone().context("Missing --target (or provide --query)")?;
        let (xml, _meta) = slice_to_xml(&repo_root, &target, cli.budget_tokens, &cfg)?;
        (xml, target.to_string_lossy().to_string())
    };

    // Ensure output dir exists and write file.
    let out_dir = repo_root.join(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(out_dir.join("active_context.xml"), &xml)?;

    // Write a small meta file for UIs.
    // (Keeps format similar to legacy implementations.)
    let meta_json = json!({
        "repoRoot": repo_root.to_string_lossy(),
        "target": target_label,
        "budgetTokens": cli.budget_tokens,
        "totalTokens": (xml.len() as f64 / 4.0).ceil() as u64,
        "totalChars": xml.len()
    });
    let _ = std::fs::write(
        out_dir.join("active_context.meta.json"),
        serde_json::to_vec_pretty(&meta_json)?,
    );

    if cli.xml {
        print!("{}", xml);
    } else {
        // Default to printing JSON meta later; for now just confirm success.
        eprintln!("Wrote {} bytes to {}", xml.len(), out_dir.join("active_context.xml").display());
    }

    Ok(())
}
