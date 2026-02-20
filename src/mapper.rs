use anyhow::Result;
use ignore::WalkBuilder;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::inspector::analyze_file;

#[derive(Debug, Clone, Serialize)]
pub struct MapNode {
    pub id: String,
    pub label: String,
    pub path: String,
    pub kind: String,
    pub size_class: String,
    pub bytes: u64,
    pub est_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapEdge {
    pub id: String,
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoMap {
    pub nodes: Vec<MapNode>,
    pub edges: Vec<MapEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleNode {
    pub id: String,
    pub label: String,
    pub path: String,
    pub file_count: u64,
    pub bytes: u64,
    pub est_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub weight: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleGraph {
    pub nodes: Vec<ModuleNode>,
    pub edges: Vec<ModuleEdge>,
}

fn is_known_manifest_file(name: &str) -> bool {
    matches!(
        name,
        "package.json" | "Cargo.toml" | "pubspec.yaml" | "go.mod"
    )
}

fn read_package_json_name(package_json: &Path) -> Option<String> {
    let text = std::fs::read_to_string(package_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

fn read_pubspec_name(pubspec_yaml: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pubspec_yaml).ok()?;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') || l.is_empty() {
            continue;
        }
        if let Some(rest) = l.strip_prefix("name:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn read_go_module_name(go_mod: &Path) -> Option<String> {
    let text = std::fs::read_to_string(go_mod).ok()?;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("module ") {
            let module_path = l.trim_start_matches("module ").trim();
            if module_path.is_empty() {
                return None;
            }
            // Prefer a short label: last segment of module path.
            let short = module_path
                .split('/')
                .filter(|s| !s.is_empty())
                .next_back()
                .unwrap_or(module_path);
            return Some(short.to_string());
        }
    }
    None
}

fn read_cargo_package_name(cargo_toml: &Path) -> Option<String> {
    let text = std::fs::read_to_string(cargo_toml).ok()?;
    let value: toml::Value = text.parse().ok()?;
    value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

fn read_cargo_lib_name(cargo_toml: &Path) -> Option<String> {
    let text = std::fs::read_to_string(cargo_toml).ok()?;
    let value: toml::Value = text.parse().ok()?;
    value
        .get("lib")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Read path-based dependencies from Cargo.toml.
/// Returns Vec<(dependency_name, relative_path_to_dependency)>.
fn read_cargo_dependencies(cargo_toml: &Path) -> Vec<(String, String)> {
    let text = match std::fs::read_to_string(cargo_toml) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let value: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut result = Vec::new();

    // Check both [dependencies] and [dev-dependencies]
    for section in &["dependencies", "dev-dependencies"] {
        if let Some(deps) = value.get(section).and_then(|d| d.as_table()) {
            for (dep_name, dep_value) in deps {
                // Check if it's a path dependency: { path = "../some-service" }
                if let Some(path_str) = dep_value
                    .as_table()
                    .and_then(|t| t.get("path"))
                    .and_then(|p| p.as_str())
                {
                    result.push((dep_name.clone(), path_str.to_string()));
                }
            }
        }
    }

    result
}

fn module_id_for_rel_path(file_rel: &str, module_roots: &[(String, String)]) -> Option<String> {
    // module_roots: (dir_rel, module_id)
    let mut best: Option<(&String, usize)> = None;
    for (root, id) in module_roots {
        if root.is_empty() || root == "." {
            // Root module matches everything, but prefer more specific roots.
            let depth = 0;
            match best {
                None => best = Some((id, depth)),
                Some((_, best_depth)) if depth > best_depth => best = Some((id, depth)),
                _ => {}
            }
            continue;
        }

        if file_rel == root {
            let depth = root.len();
            match best {
                None => best = Some((id, depth)),
                Some((_, best_depth)) if depth > best_depth => best = Some((id, depth)),
                _ => {}
            }
            continue;
        }

        if file_rel.starts_with(root) {
            let bytes = file_rel.as_bytes();
            if bytes.get(root.len()) == Some(&b'/') {
                let depth = root.len();
                match best {
                    None => best = Some((id, depth)),
                    Some((_, best_depth)) if depth > best_depth => best = Some((id, depth)),
                    _ => {}
                }
            }
        }
    }
    best.map(|(id, _)| id.clone())
}

/// Build a dependency graph strictly from the provided manifest files.
///
/// Contract:
/// - Creates exactly 1 module node per manifest's parent directory.
/// - Only scans files inside those module directories.
/// - Does not descend into nested selected modules.
/// - Creates edges only when an import/usage resolves to another selected module.
pub fn build_map_from_manifests(repo_root: &Path, manifests: &[PathBuf]) -> Result<ModuleGraph> {
    // 1) Normalize module directories.
    #[derive(Clone)]
    struct ModuleSpec {
        dir_abs: PathBuf,
        dir_rel: String,
        id: String,
        label: String,
        cargo_name: Option<String>,
        cargo_lib_name: Option<String>,
    }

    let mut specs: Vec<ModuleSpec> = Vec::new();
    let mut seen_dirs: BTreeSet<PathBuf> = BTreeSet::new();

    for m in manifests {
        // Normalize separators for relative paths (helps when external tooling passes Windows-style paths).
        let m_norm = if m.is_absolute() {
            m.clone()
        } else {
            PathBuf::from(normalize_slash(m.as_ref()))
        };

        let abs = if m_norm.is_absolute() {
            m_norm.clone()
        } else {
            repo_root.join(&m_norm)
        };
        let abs = abs.canonicalize().unwrap_or(abs);

        let name = abs.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !is_known_manifest_file(name) {
            continue;
        }
        if !abs.exists() {
            anyhow::bail!("Manifest not found: {}", abs.display());
        }
        let Some(parent) = abs.parent() else {
            continue;
        };
        if path_has_forbidden_component(parent) {
            continue;
        }

        let dir_abs = parent.to_path_buf();
        if seen_dirs.contains(&dir_abs) {
            continue;
        }
        seen_dirs.insert(dir_abs.clone());

        let dir_rel = rel_str(repo_root, &dir_abs).unwrap_or_else(|| normalize_slash(&dir_abs));
        let id = normalize_module_id(&dir_rel);

        let folder_fallback = dir_abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("module")
            .to_string();

        let mut label = folder_fallback.clone();
        let mut cargo_name: Option<String> = None;
        let mut cargo_lib_name: Option<String> = None;

        match name {
            "package.json" => {
                if let Some(n) = read_package_json_name(&abs) {
                    label = n;
                }
            }
            "Cargo.toml" => {
                cargo_name = read_cargo_package_name(&abs);
                cargo_lib_name = read_cargo_lib_name(&abs);
                // Prefer lib.name for display (often matches Rust import crate name), fallback to package.name.
                if let Some(n) = cargo_lib_name.clone().or_else(|| cargo_name.clone()) {
                    label = n;
                }
            }
            "pubspec.yaml" => {
                if let Some(n) = read_pubspec_name(&abs) {
                    label = n;
                }
            }
            "go.mod" => {
                if let Some(n) = read_go_module_name(&abs) {
                    label = n;
                }
            }
            _ => {}
        }

        specs.push(ModuleSpec {
            dir_abs,
            dir_rel,
            id,
            label,
            cargo_name,
            cargo_lib_name,
        });
    }

    specs.sort_by(|a, b| a.id.cmp(&b.id));
    if specs.is_empty() {
        return Ok(ModuleGraph {
            nodes: vec![],
            edges: vec![],
        });
    }

    // 2) Build node list + metadata.
    #[derive(Clone, Default)]
    struct Acc {
        bytes: u64,
        file_count: u64,
        files: Vec<PathBuf>,
    }

    let module_dir_rel_set: BTreeSet<String> = specs.iter().map(|s| s.dir_rel.clone()).collect();
    let repo_root_owned = repo_root.to_path_buf();
    let module_roots_rel: Vec<(String, String)> = specs
        .iter()
        .map(|s| (s.dir_rel.clone(), s.id.clone()))
        .collect();

    let mut acc_by_dir: BTreeMap<PathBuf, Acc> = BTreeMap::new();
    for s in &specs {
        acc_by_dir.entry(s.dir_abs.clone()).or_default();
    }

    // 3) Scan files inside each module dir only (and don't descend into nested selected modules).
    for s in &specs {
        let d = &s.dir_abs;
        let repo_root_owned = repo_root_owned.clone();
        let module_dir_rel_set = module_dir_rel_set.clone();
        let walker = WalkBuilder::new(d)
            .standard_filters(true)
            .hidden(false)
            .max_depth(Some(25))
            .filter_entry(move |entry| {
                let name = entry.file_name().to_str().unwrap_or("");
                if should_skip_dir_name(name) {
                    return false;
                }
                if path_has_forbidden_component(entry.path()) {
                    return false;
                }

                // Do not descend into other selected modules if nested.
                if entry.depth() > 0 {
                    if let Some(rel) = rel_str(&repo_root_owned, entry.path()) {
                        if module_dir_rel_set.contains(&rel) {
                            return false;
                        }
                    }
                }
                true
            })
            .build();

        for ent in walker {
            let Ok(ent) = ent else { continue };
            if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let p = ent.path();
            if !is_allowed_source_ext(p) {
                continue;
            }
            if path_has_forbidden_component(p) {
                continue;
            }
            let sz = ent.metadata().map(|m| m.len()).unwrap_or(0);
            let a = acc_by_dir.get_mut(d).unwrap();
            a.bytes += sz;
            a.file_count += 1;
            a.files.push(p.to_path_buf());
        }
    }

    // Push counts into nodes.
    let mut nodes: Vec<ModuleNode> = Vec::new();
    for s in &specs {
        let a = acc_by_dir.get(&s.dir_abs).cloned().unwrap_or_default();
        nodes.push(ModuleNode {
            id: s.id.clone(),
            label: s.label.clone(),
            path: s.id.clone(),
            file_count: a.file_count,
            bytes: a.bytes,
            est_tokens: est_tokens_from_bytes(a.bytes),
        });
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    // Build mapping for Rust crate name -> module id.
    let mut crate_to_module: BTreeMap<String, String> = BTreeMap::new();
    for s in &specs {
        if let Some(name) = s.cargo_name.clone() {
            crate_to_module.insert(name.clone(), s.id.clone());
            // Cargo package names may contain '-' but Rust `use` paths use '_' for crate names.
            let underscored = name.replace('-', "_");
            if underscored != name {
                crate_to_module.insert(underscored, s.id.clone());
            }
        }
        if let Some(name) = s.cargo_lib_name.clone() {
            crate_to_module.insert(name.clone(), s.id.clone());
            let underscored = name.replace('-', "_");
            if underscored != name {
                crate_to_module.insert(underscored, s.id.clone());
            }
        }
    }

    // Build map of relative path (normalized) -> module id for path-based dependency resolution.
    let mut path_to_module: BTreeMap<String, String> = BTreeMap::new();
    for s in &specs {
        path_to_module.insert(s.dir_rel.clone(), s.id.clone());
    }

    // 3.5) Scan Cargo.toml dependencies to create edges.
    let mut weights: BTreeMap<(String, String), u64> = BTreeMap::new();
    for s in &specs {
        // Look for Cargo.toml in this module's directory
        let cargo_toml_path = s.dir_abs.join("Cargo.toml");
        if !cargo_toml_path.exists() {
            continue;
        }

        let deps = read_cargo_dependencies(&cargo_toml_path);

        for (_dep_name, dep_path) in deps {
            // Resolve the relative path from this module's directory
            let dep_abs = s.dir_abs.join(&dep_path);
            let dep_abs = dep_abs.canonicalize().unwrap_or(dep_abs);

            // Convert to repo-relative path
            let dep_rel = match rel_str(repo_root, &dep_abs) {
                Some(r) => r,
                None => continue,
            };

            // Find the module that corresponds to this path
            if let Some(dst_mod_id) = path_to_module.get(&dep_rel) {
                if dst_mod_id != &s.id {
                    // Add weight 5 for explicit dependency (higher than single import)
                    *weights
                        .entry((s.id.clone(), dst_mod_id.clone()))
                        .or_insert(0) += 5;
                }
            }
        }
    }

    // 4) Resolve imports into edges between selected modules.

    let module_ids: Vec<(PathBuf, String)> = specs
        .iter()
        .map(|s| (s.dir_abs.clone(), s.id.clone()))
        .collect();

    for (dir, src_mod_id) in &module_ids {
        let a = acc_by_dir.get(dir).cloned().unwrap_or_default();

        for file_abs in &a.files {
            let analyzed = match analyze_file(file_abs) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let ext = file_abs
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let is_rust = ext == "rs";

            for imp in analyzed.imports {
                if is_rust {
                    // Rust: `use foo::bar::Baz;` -> crate `foo`
                    let first = imp.split("::").next().unwrap_or("").trim();
                    if first.is_empty() {
                        continue;
                    }
                    if let Some(dst_mod_id) = crate_to_module.get(first) {
                        if dst_mod_id != src_mod_id {
                            *weights
                                .entry((src_mod_id.clone(), dst_mod_id.clone()))
                                .or_insert(0) += 1;
                        }
                    }
                    continue;
                }

                // TS/JS: resolve relative import to a file, then map to a selected module by prefix.
                let Some(dst_file_abs) = resolve_ts_import(repo_root, file_abs, &imp) else {
                    continue;
                };
                let dst_file_abs = dst_file_abs.canonicalize().unwrap_or(dst_file_abs);

                // Compare using repo-relative forward-slash paths to avoid OS separator mismatches.
                let Some(dst_rel) = rel_str(repo_root, &dst_file_abs) else {
                    continue;
                };
                let Some(dst_mod_id) = module_id_for_rel_path(&dst_rel, &module_roots_rel) else {
                    continue;
                };
                if dst_mod_id != *src_mod_id {
                    *weights.entry((src_mod_id.clone(), dst_mod_id)).or_insert(0) += 1;
                }
            }
        }
    }

    let mut edges: Vec<ModuleEdge> = Vec::new();
    for ((s, t), w) in weights {
        edges.push(ModuleEdge {
            id: format!("{}->{}", s, t),
            source: s,
            target: t,
            weight: w,
        });
    }
    edges.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(ModuleGraph { nodes, edges })
}

// Backward-compatible alias (older clients may still reference this name).
pub fn build_graph_from_manifests(repo_root: &Path, manifests: &[PathBuf]) -> Result<ModuleGraph> {
    build_map_from_manifests(repo_root, manifests)
}

fn size_class_from_bytes(bytes: u64) -> String {
    if bytes < 200_000 {
        "small".to_string()
    } else if bytes < 1_500_000 {
        "medium".to_string()
    } else {
        "large".to_string()
    }
}

fn est_tokens_from_bytes(bytes: u64) -> u64 {
    // Match the simple heuristic used elsewhere: ~4 chars per token.
    ((bytes as f64) / 4.0).ceil() as u64
}

fn is_module_marker_file(name: &str) -> bool {
    matches!(
        name,
        "package.json"
            | "index.ts"
            | "index.tsx"
            | "index.js"
            | "index.jsx"
            | "mod.rs"
    )
        // Practical Rust crate roots (often no mod.rs at root)
        || matches!(name, "lib.rs" | "main.rs")
}

fn module_label(repo_root: &Path, module_abs: &Path) -> String {
    if module_abs == repo_root {
        return repo_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("root")
            .to_string();
    }
    module_abs
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string()
}

fn resolve_ts_import(repo_root: &Path, from_file_abs: &Path, imp: &str) -> Option<PathBuf> {
    let imp = imp.trim();
    if !imp.starts_with('.') {
        return None;
    }

    let base_dir = from_file_abs.parent()?;

    let exts = [
        "ts", "tsx", "js", "jsx", "json", "md", "toml", "css", "html",
    ];
    let mut candidates: Vec<PathBuf> = Vec::new();

    candidates.push(base_dir.join(imp));
    for e in exts {
        candidates.push(base_dir.join(format!("{}.{}", imp, e)));
    }
    for e in ["ts", "tsx", "js", "jsx"] {
        candidates.push(base_dir.join(imp).join(format!("index.{}", e)));
    }

    for cand in candidates {
        if !cand.exists() {
            continue;
        }
        let cand_abs = cand.canonicalize().unwrap_or(cand);
        if cand_abs.strip_prefix(repo_root).is_ok() {
            return Some(cand_abs);
        }
    }

    None
}

fn find_owner_module(
    mut dir: &Path,
    stop_at: &Path,
    module_roots: &BTreeSet<PathBuf>,
) -> Option<PathBuf> {
    loop {
        if module_roots.contains(dir) {
            return Some(dir.to_path_buf());
        }
        if dir == stop_at {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// High-level architecture graph: nodes are module roots; edges are weighted imports between modules.
pub fn build_module_graph(repo_root: &Path, root: &Path) -> Result<ModuleGraph> {
    let root_abs = if root.is_absolute() {
        root.to_path_buf()
    } else {
        repo_root.join(root)
    }
    .canonicalize()
    .unwrap_or_else(|_| repo_root.join(root));

    if !root_abs.exists() {
        anyhow::bail!("Graph root not found: {}", root_abs.display());
    }
    if !root_abs.is_dir() {
        anyhow::bail!("Graph root is not a directory: {}", root_abs.display());
    }

    // 1) Discover module roots (directories containing marker files).
    let mut module_roots: BTreeSet<PathBuf> = BTreeSet::new();
    module_roots.insert(root_abs.clone());

    let walker = WalkBuilder::new(&root_abs)
        .standard_filters(true)
        .hidden(false)
        .max_depth(Some(25))
        .filter_entry(|entry| {
            let name = entry.file_name().to_str().unwrap_or("");
            if should_skip_dir_name(name) {
                return false;
            }
            if path_has_forbidden_component(entry.path()) {
                return false;
            }
            true
        })
        .build();

    for ent in walker {
        let Ok(ent) = ent else { continue };
        if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let p = ent.path();
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_module_marker_file(name) {
            continue;
        }
        let Some(parent) = p.parent() else { continue };
        module_roots.insert(parent.to_path_buf());
    }

    // 2) Assign files to their owning module (nearest ancestor module root).
    #[derive(Default)]
    struct ModuleAcc {
        bytes: u64,
        file_count: u64,
        files: Vec<PathBuf>,
    }

    let mut modules: BTreeMap<PathBuf, ModuleAcc> = BTreeMap::new();
    for r in &module_roots {
        modules.entry(r.clone()).or_default();
    }

    let walker2 = WalkBuilder::new(&root_abs)
        .standard_filters(true)
        .hidden(false)
        .max_depth(Some(25))
        .filter_entry(|entry| {
            let name = entry.file_name().to_str().unwrap_or("");
            if should_skip_dir_name(name) {
                return false;
            }
            if path_has_forbidden_component(entry.path()) {
                return false;
            }
            true
        })
        .build();

    for ent in walker2 {
        let Ok(ent) = ent else { continue };
        if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let p = ent.path();
        if path_has_forbidden_component(p) {
            continue;
        }
        if !is_allowed_ext(p) {
            continue;
        }
        let Some(parent) = p.parent() else { continue };
        let owner =
            find_owner_module(parent, &root_abs, &module_roots).unwrap_or_else(|| root_abs.clone());
        let acc = modules.entry(owner).or_default();
        let sz = ent.metadata().map(|m| m.len()).unwrap_or(0);
        acc.bytes += sz;
        acc.file_count += 1;
        acc.files.push(p.to_path_buf());
    }

    // 3) Build nodes.
    let mut nodes: Vec<ModuleNode> = Vec::new();
    let mut module_id_by_abs: BTreeMap<PathBuf, String> = BTreeMap::new();

    for (abs, acc) in &modules {
        let rel = abs
            .strip_prefix(repo_root)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"));
        let id = normalize_module_id(rel.as_deref().unwrap_or("."));
        module_id_by_abs.insert(abs.clone(), id.clone());
        nodes.push(ModuleNode {
            id: id.clone(),
            label: module_label(repo_root, abs),
            path: id,
            file_count: acc.file_count,
            bytes: acc.bytes,
            est_tokens: est_tokens_from_bytes(acc.bytes),
        });
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    // 4) Edges: file imports -> module imports, weighted.
    let mut weights: BTreeMap<(String, String), u64> = BTreeMap::new();

    for (module_abs, acc) in &modules {
        let Some(src_mod_id) = module_id_by_abs.get(module_abs).cloned() else {
            continue;
        };
        for file_abs in &acc.files {
            let analyzed = match analyze_file(file_abs) {
                Ok(v) => v,
                Err(_) => continue,
            };

            for imp in analyzed.imports {
                let Some(dst_file_abs) = resolve_ts_import(repo_root, file_abs, &imp) else {
                    continue;
                };
                let Some(dst_parent) = dst_file_abs.parent() else {
                    continue;
                };
                let dst_owner = find_owner_module(dst_parent, &root_abs, &module_roots)
                    .unwrap_or_else(|| root_abs.clone());
                let Some(dst_mod_id) = module_id_by_abs.get(&dst_owner).cloned() else {
                    continue;
                };
                if dst_mod_id == src_mod_id {
                    continue;
                }
                *weights.entry((src_mod_id.clone(), dst_mod_id)).or_insert(0) += 1;
            }
        }
    }

    let mut edges: Vec<ModuleEdge> = Vec::new();
    for ((s, t), w) in weights {
        edges.push(ModuleEdge {
            id: format!("{}->{}", s, t),
            source: s,
            target: t,
            weight: w,
        });
    }
    edges.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(ModuleGraph { nodes, edges })
}

/// Core path normalization helper: ALWAYS converts backslashes to forward slashes.
/// This ensures cross-platform consistency (Windows \ vs Unix /).
fn normalize_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn rel_str(repo_root: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(repo_root).ok().map(normalize_slash)
}

fn normalize_module_id(rel: &str) -> String {
    // In single-package repos, the module can be the repository root.
    // rel_str(repo_root, repo_root) yields ""; normalize that to "." so the frontend can handle it.
    if rel.is_empty() {
        ".".to_string()
    } else {
        rel.to_string()
    }
}

fn clamp_label(name: &str) -> String {
    if name.is_empty() {
        return "(unnamed)".to_string();
    }
    name.to_string()
}

fn should_skip_dir_name(name: &str) -> bool {
    matches!(
        name,
        // VCS / editor
        ".git" | ".vscode" | ".idea" | ".vs"
        // JS / Node
        | "node_modules" | "dist" | "build" | ".next" | ".nuxt" | ".svelte-kit" | ".turbo"
        // Rust
        | "target" | ".cargo"
        // Python
        | "__pycache__" | ".venv" | "venv" | ".env" | "env" | ".tox"
        | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | "htmlcov"
        | ".hypothesis" | "site-packages"
        // Dart / Flutter
        | ".dart_tool" | ".pub" | ".pub-cache" | ".flutter-plugins"
        | ".flutter-plugins-dependencies"
        // Go
        | "vendor"
        // Ruby
        | ".bundle"
        // Java / Kotlin
        | ".gradle" | ".m2"
        // Infra
        | ".terraform" | ".serverless"
        // Generic junk
        | "tmp" | "temp" | "logs" | ".cache" | ".cortextast"
    )
}

fn path_has_forbidden_component(path: &Path) -> bool {
    for comp in path.components() {
        let std::path::Component::Normal(os) = comp else {
            continue;
        };
        let Some(s) = os.to_str() else {
            continue;
        };
        if should_skip_dir_name(s) {
            return true;
        }
    }
    false
}

fn is_allowed_ext(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext,
        // Rust / JS / TS source
        "rs" | "ts" | "tsx" | "js" | "jsx" |
        // Config / docs
        "json" | "md" | "toml" |
        // Web / styles (small allowlist, safe to count)
        "css" | "scss" | "sass" | "html"
    )
}

fn is_allowed_source_ext(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext,
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "dart"
    )
}

pub fn build_repo_map(repo_root: &Path) -> Result<RepoMap> {
    build_repo_map_scoped(repo_root, repo_root)
}

/// Build a scoped repo map for a specific subdirectory.
///
/// Contract for folder expansion UIs:
/// - Only returns the *immediate children* (files + folders) of the scoped directory.
/// - Hard-excludes forbidden folders (node_modules, .git, target, dist, build, etc).
/// - File nodes are only included for allowlisted text/source extensions.
/// - Edges connect `parent_id -> child_id`.
pub fn build_repo_map_scoped(repo_root: &Path, scope: &Path) -> Result<RepoMap> {
    let scope_abs = if scope.is_absolute() {
        scope.to_path_buf()
    } else {
        repo_root.join(scope)
    };

    let scope_abs = scope_abs.canonicalize().unwrap_or(scope_abs);

    if !scope_abs.exists() {
        anyhow::bail!("Scope path not found: {}", scope_abs.display());
    }
    if !scope_abs.is_dir() {
        anyhow::bail!("Scope path is not a directory: {}", scope_abs.display());
    }

    // Parent id is the repo-relative directory path.
    let parent_rel =
        rel_str(repo_root, &scope_abs).unwrap_or_else(|| scope.to_string_lossy().to_string());
    let parent_id = normalize_module_id(&parent_rel);

    // Include the container node itself so the frontend can treat it as a stable "card".
    let parent_label = if parent_id == "." {
        repo_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("root")
            .to_string()
    } else {
        scope_abs
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&parent_id)
            .to_string()
    };

    let mut nodes: Vec<MapNode> = Vec::new();
    let mut edges: Vec<MapEdge> = Vec::new();

    nodes.push(MapNode {
        id: parent_id.clone(),
        label: parent_label,
        path: parent_id.clone(),
        kind: "directory".to_string(),
        size_class: "small".to_string(),
        bytes: 0,
        est_tokens: 0,
    });

    let rd = std::fs::read_dir(&scope_abs)?;
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // HARD DENY by immediate name.
        if should_skip_dir_name(&name) {
            continue;
        }

        // HARD DENY by path component.
        if path_has_forbidden_component(&path) {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };

        if ft.is_dir() {
            // Include folder nodes.
            let rel = rel_str(repo_root, &path).unwrap_or_else(|| name.clone());
            let id = normalize_module_id(&rel);
            let label = clamp_label(&name);

            nodes.push(MapNode {
                id: id.clone(),
                label,
                path: id.clone(),
                kind: "directory".to_string(),
                size_class: "small".to_string(),
                bytes: 0,
                est_tokens: 0,
            });

            edges.push(MapEdge {
                id: format!("{}->{}", parent_id, id),
                source: parent_id.clone(),
                target: id,
            });

            continue;
        }

        if ft.is_file() {
            // Only keep allowlisted file types.
            if !is_allowed_ext(&path) {
                continue;
            }

            let rel = rel_str(repo_root, &path).unwrap_or_else(|| name.clone());
            let id = normalize_module_id(&rel);
            let label = clamp_label(&name);
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let size_class = size_class_from_bytes(bytes);
            let est_tokens = est_tokens_from_bytes(bytes);

            nodes.push(MapNode {
                id: id.clone(),
                label,
                path: id.clone(),
                kind: "file".to_string(),
                size_class,
                bytes,
                est_tokens,
            });

            edges.push(MapEdge {
                id: format!("{}->{}", parent_id, id),
                source: parent_id.clone(),
                target: id,
            });
        }
    }

    // Smart edges: resolve file-to-file imports (relative imports for TS/JS).
    let mut id_set: BTreeSet<String> = BTreeSet::new();
    for n in &nodes {
        id_set.insert(n.id.clone());
    }

    // Build a quick lookup of existing file ids.
    let mut file_ids: Vec<String> = Vec::new();
    for n in &nodes {
        if n.kind == "file" {
            file_ids.push(n.id.clone());
        }
    }

    // Attempt to resolve relative imports within the repo.
    let exts = ["ts", "tsx", "js", "jsx", "json", "md"];
    for src_id in &file_ids {
        let src_abs = repo_root.join(src_id);
        let analyzed = match analyze_file(&src_abs) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for imp in analyzed.imports {
            let imp = imp.trim();
            if !imp.starts_with('.') {
                continue;
            }

            let base_dir = src_abs.parent().unwrap_or(repo_root);
            let mut candidates: Vec<PathBuf> = Vec::new();

            let raw = base_dir.join(imp);
            candidates.push(raw.clone());
            for e in exts {
                candidates.push(base_dir.join(format!("{}.{}", imp, e)));
            }
            // Directory-style imports: ./foo -> ./foo/index.ts
            for e in ["ts", "tsx", "js", "jsx"] {
                candidates.push(base_dir.join(imp).join(format!("index.{}", e)));
            }

            let mut resolved: Option<String> = None;
            for cand in candidates {
                if !cand.exists() {
                    continue;
                }
                let cand_abs = cand.canonicalize().unwrap_or(cand);
                if let Ok(rel) = cand_abs.strip_prefix(repo_root) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    let id = normalize_module_id(&rel_str);
                    if id_set.contains(&id) {
                        resolved = Some(id);
                        break;
                    }
                }
            }

            let Some(dst_id) = resolved else { continue };
            if dst_id == *src_id {
                continue;
            }

            edges.push(MapEdge {
                id: format!("import:{}->{}", src_id, dst_id),
                source: src_id.clone(),
                target: dst_id,
            });
        }
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(RepoMap { nodes, edges })
}
