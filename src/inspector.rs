use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::universal::render_universal_skeleton;

#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,

    /// 0-indexed start line
    pub line: u32,

    /// 0-indexed end line (inclusive-ish; derived from tree-sitter end position)
    pub line_end: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileSymbols {
    pub file: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub symbols: Vec<Symbol>,
}

fn normalize_path_for_output(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

pub trait LanguageDriver: Send + Sync {
    fn name(&self) -> &'static str;
    /// Primary file extensions handled by this driver (lowercase, without dot).
    fn extensions(&self) -> &'static [&'static str];
    fn handles_path(&self, path: &Path) -> bool;
    fn language_for_path(&self, path: &Path) -> Language;

    fn find_imports(&self, _path: &Path, _source: &[u8], _root: Node, _language: Language) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn find_exports(&self, _path: &Path, _source: &[u8], _root: Node, _language: Language) -> Result<Vec<String>> {
        Ok(vec![])
    }

    /// Return byte ranges to replace with skeleton placeholders.
    ///
    /// Each tuple is (start_byte, end_byte, replacement_text).
    /// Implementations should only return ranges for *bodies* (function/method bodies, etc)
    /// and avoid matching arbitrary blocks (e.g. `if` blocks).
    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        _source: &[u8],
        _root: Node,
        _language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        Ok(vec![])
    }

    fn extract_skeleton(&self, path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>>;
}

fn apply_replacements(source_text: &str, mut reps: Vec<(usize, usize, String)>) -> String {
    // Apply from end -> start so byte offsets remain valid.
    reps.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = source_text.to_string();

    let mut last_start: Option<usize> = None;
    for (start, end, replacement) in reps.into_iter().rev() {
        if start >= end || start > out.len() || end > out.len() {
            continue;
        }

        // Skip overlapping edits (prefer inner-most / later ranges due to reverse order).
        if let Some(ls) = last_start {
            if end > ls {
                continue;
            }
        }

        out.replace_range(start..end, &replacement);
        last_start = Some(start);
    }

    out
}

fn contains_todo_fixme(s: &str) -> bool {
    let up = s.to_ascii_uppercase();
    up.contains("TODO") || up.contains("FIXME")
}

fn is_comment_only_line_trimmed(t: &str) -> bool {
    if t.is_empty() {
        return false;
    }

    // Preserve shebangs (#!/usr/bin/env ...)
    if t.starts_with("#!") && !t.starts_with("#![") {
        return false;
    }

    t.starts_with("//") || t.starts_with('#') || t.starts_with("--")
}

fn strip_trailing_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for part in text.split_inclusive('\n') {
        if let Some(line) = part.strip_suffix('\n') {
            out.push_str(line.trim_end_matches([' ', '\t', '\r']));
            out.push('\n');
        } else {
            out.push_str(part.trim_end_matches([' ', '\t', '\r']));
        }
    }
    out
}

fn strip_python_module_docstring_if_present(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut start_idx: usize = 0;

    // Keep optional shebang.
    if let Some(l0) = lines.first().copied() {
        let t0 = l0.trim_start();
        if t0.starts_with("#!") && !t0.starts_with("#![") {
            start_idx = 1;
        }
    }

    while start_idx < lines.len() && lines[start_idx].trim().is_empty() {
        start_idx += 1;
    }
    if start_idx >= lines.len() {
        return text.to_string();
    }

    let first = lines[start_idx].trim_start();
    let (quote, prefix_len) = if first.starts_with("\"\"\"") {
        ("\"\"\"", 3)
    } else if first.starts_with("'''") {
        ("'''", 3)
    } else {
        return text.to_string();
    };

    // Find closing triple quotes.
    let mut end_idx = start_idx;
    let mut found_close = false;
    let mut combined = String::new();

    // Handle single-line docstring: """foo"""
    if first[prefix_len..].contains(quote) {
        combined.push_str(first);
        found_close = true;
    } else {
        combined.push_str(first);
        combined.push('\n');
        end_idx += 1;
        while end_idx < lines.len() {
            let l = lines[end_idx];
            combined.push_str(l);
            combined.push('\n');
            if l.contains(quote) {
                found_close = true;
                break;
            }
            end_idx += 1;
        }
    }

    if !found_close {
        return text.to_string();
    }

    if contains_todo_fixme(&combined) {
        return text.to_string();
    }

    // Remove docstring lines [start_idx..=end_idx]
    lines.drain(start_idx..=end_idx.min(lines.len().saturating_sub(1)));
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn strip_comment_only_lines_and_blocks(text: &str) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    let mut i: usize = 0;
    let lines: Vec<&str> = text.lines().collect();

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if contains_todo_fixme(trimmed) {
            out_lines.push(line.to_string());
            i += 1;
            continue;
        }

        // Remove block comments that start at the beginning of a line (common for license headers).
        if trimmed.starts_with("/*") {
            // Preserve our own skeleton placeholders and truncation markers.
            let keep = trimmed.contains("/* ... */") || trimmed.contains("TRUNCATED") || contains_todo_fixme(trimmed);
            if keep {
                out_lines.push(line.to_string());
                i += 1;
                continue;
            }

            // Consume until closing */
            let mut block_text = String::new();
            block_text.push_str(trimmed);
            block_text.push('\n');

            let mut j = i;
            let mut closed = trimmed.contains("*/");
            while !closed {
                j += 1;
                if j >= lines.len() {
                    break;
                }
                block_text.push_str(lines[j]);
                block_text.push('\n');
                if lines[j].contains("*/") {
                    closed = true;
                }
            }

            if contains_todo_fixme(&block_text) {
                let end = j.min(lines.len().saturating_sub(1));
                for l in lines.iter().take(end + 1).skip(i) {
                    out_lines.push((*l).to_string());
                }
            }
            i = j.saturating_add(1);
            continue;
        }

        if is_comment_only_line_trimmed(trimmed) {
            // Drop comment-only lines unless TODO/FIXME (handled above).
            i += 1;
            continue;
        }

        out_lines.push(line.to_string());
        i += 1;
    }

    let mut out = out_lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn nuke_all_imports(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.to_string();
    }

    let mut preserved_lines: Vec<String> = Vec::new();
    let mut import_count: usize = 0;
    let mut in_go_import_block = false;
    let mut first_import_keyword: Option<&str> = None;
    let mut i: usize = 0;

    // Preserve optional shebang.
    if lines[0].trim_start().starts_with("#!") && !lines[0].trim_start().starts_with("#![") {
        preserved_lines.push(lines[0].to_string());
        i = 1;
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Detect Go import block: import ( ... )
        if trimmed.starts_with("import (") || trimmed == "import(" {
            in_go_import_block = true;
            import_count += 1;
            if first_import_keyword.is_none() {
                first_import_keyword = Some("import");
            }
            i += 1;
            continue;
        }

        if in_go_import_block {
            if trimmed.starts_with(')') {
                in_go_import_block = false;
            } else if !trimmed.is_empty() {
                import_count += 1;
            }
            i += 1;
            continue;
        }

        // Detect individual import/use/from/using lines.
        if trimmed.starts_with("use ") || trimmed.starts_with("import ") || trimmed.starts_with("from ") || trimmed.starts_with("using ") {
            if first_import_keyword.is_none() {
                if trimmed.starts_with("use ") {
                    first_import_keyword = Some("use");
                } else if trimmed.starts_with("using ") {
                    first_import_keyword = Some("using");
                } else {
                    first_import_keyword = Some("import");
                }
            }
            import_count += 1;
            i += 1;
            continue;
        }

        preserved_lines.push(line.to_string());
        i += 1;
    }

    // Inject import hint at the top (after shebang if present).
    if import_count > 0 {
        let keyword = first_import_keyword.unwrap_or("import");
        let hint = format!("// ... ({} {}s)", import_count, keyword);
        if preserved_lines.is_empty() || (preserved_lines.len() == 1 && preserved_lines[0].starts_with("#!")) {
            preserved_lines.push(hint);
        } else {
            preserved_lines.insert(0, hint);
        }
    }

    let mut out = preserved_lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn flatten_indentation_for_braces(path: &Path, text: &str) -> String {
    let ext = path_ext_lower(path);

    // Keep indentation for indent-sensitive languages.
    if matches!(ext.as_str(), "py" | "yaml" | "yml") {
        return text.to_string();
    }

    // For brace-based languages, strip leading whitespace from every line.
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        out.push_str(trimmed);
        out.push('\n');
    }

    // Preserve final newline status.
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

fn collapse_empty_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_nl = false;
    for ch in text.chars() {
        if ch == '\n' {
            if prev_nl {
                continue;
            }
            prev_nl = true;
            out.push(ch);
        } else {
            prev_nl = false;
            out.push(ch);
        }
    }
    out
}

fn clean_skeleton_text(path: &Path, text: &str) -> String {
    // Order matters: strip whitespace first to make comment/import detection more consistent.
    let mut out = strip_trailing_whitespace(text);
    out = strip_comment_only_lines_and_blocks(&out);

    if path_ext_lower(path) == "py" {
        out = strip_python_module_docstring_if_present(&out);
    }

    // Nuclear optimization: delete ALL imports and replace with a single hint line.
    out = nuke_all_imports(&out);

    // Flatten indentation for brace-based languages (preserve Python/YAML).
    out = flatten_indentation_for_braces(path, &out);

    out = collapse_empty_newlines(&out);
    out
}

fn line_indent_at_byte(source_text: &str, byte_idx: usize) -> String {
    let bytes = source_text.as_bytes();
    let mut i = byte_idx.min(bytes.len());
    while i > 0 {
        if bytes[i - 1] == b'\n' {
            break;
        }
        i -= 1;
    }

    let mut j = i;
    while j < bytes.len() {
        let b = bytes[j];
        if b == b' ' || b == b'\t' {
            j += 1;
            continue;
        }
        break;
    }

    source_text[i..j].to_string()
}

/// Render a "skeleton" version of a file by pruning function/method bodies.
///
/// This is designed to be *high-signal, low-noise* context for LLMs.
pub fn render_skeleton(path: &Path) -> Result<String> {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get current dir")?.join(path)
    };

    let driver = language_config()
        .driver_for_path(&abs)
        .ok_or_else(|| anyhow!("Unsupported file extension: {}", abs.display()))?;
    let language = driver.language_for_path(&abs);

    // Binary-safe read: detect null bytes before attempting UTF-8 decode.
    let raw = std::fs::read(&abs)
        .with_context(|| format!("Failed to read {}", abs.display()))?;
    if raw.contains(&0u8) {
        return Ok("/* BINARY_FILE — skipped */\n".to_string());
    }
    let source_text = String::from_utf8_lossy(&raw).into_owned();

    // Safety net: bail out before Tree-sitter on minified/machine-generated content.
    if is_minified_or_generated(&source_text) {
        return Ok("/* MINIFIED_OR_GENERATED — skipped */\n".to_string());
    }

    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Failed to set tree-sitter language")?;
    let tree = parser
        .parse(source_text.as_str(), None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;
    let root = tree.root_node();

    let ranges = driver.body_prune_ranges(&abs, &source_text, source, root, language)?;
    let out = apply_replacements(&source_text, ranges);
    Ok(clean_skeleton_text(&abs, &out))
}

/// Like render_skeleton(), but uses the provided source text (avoids double file reads).
pub fn render_skeleton_from_source(path: &Path, source_text: &str) -> Result<String> {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get current dir")?.join(path)
    };

    // Safety net.
    if is_minified_or_generated(source_text) {
        return Ok("/* MINIFIED_OR_GENERATED — skipped */\n".to_string());
    }

    let driver = language_config()
        .driver_for_path(&abs)
        .ok_or_else(|| anyhow!("Unsupported file extension: {}", abs.display()))?;
    let language = driver.language_for_path(&abs);

    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Failed to set tree-sitter language")?;
    let tree = parser
        .parse(source_text, None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;
    let root = tree.root_node();

    let ranges = driver.body_prune_ranges(&abs, source_text, source, root, language)?;
    let out = apply_replacements(source_text, ranges);
    Ok(clean_skeleton_text(&abs, &out))
}

/// Return true when a source text looks minified or machine-generated.
///
/// Heuristic: inspect the first 5 non-empty lines.  If *any* single line exceeds 2 000 chars
/// the file is almost certainly minified JS/CSS/JSON — running Tree-sitter or Regex on it
/// wastes CPU and may hang a low-RAM machine.
pub fn is_minified_or_generated(source_text: &str) -> bool {
    const MAX_SAFE_LINE_CHARS: usize = 2_000;
    source_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(5)
        .any(|l| l.len() > MAX_SAFE_LINE_CHARS)
}

/// Attempt to skeletonize a file, returning None when the file type isn't supported.
///
/// This is intended for slicer fallbacks: unsupported file types should not default to full content.
pub fn try_render_skeleton_from_source(path: &Path, source_text: &str) -> Result<Option<String>> {
    // Safety net: skip minified / machine-generated files before any parsing.
    if is_minified_or_generated(source_text) {
        return Ok(Some("/* MINIFIED_OR_GENERATED — skipped */\n".to_string()));
    }
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get current dir")?.join(path)
    };

    let Some(driver) = language_config().driver_for_path(&abs) else {
        // Universal fallback for unsupported *code-like* file types.
        // For docs/config/text formats, keep the existing truncation logic at higher layers.
        let ext = path_ext_lower(&abs);
        if matches!(
            ext.as_str(),
            "" | "md" | "txt" | "toml" | "json" | "yaml" | "yml" | "scm" | "lock" | "csv" | "tsv" | "xml" | "html" | "css"
        ) {
            return Ok(None);
        }
        return Ok(Some(render_universal_skeleton(source_text)));
    };
    let language = driver.language_for_path(&abs);

    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Failed to set tree-sitter language")?;

    let Some(tree) = parser.parse(source_text, None) else {
        // Parse failures degrade to full content at higher layers (or truncation).
        return Ok(None);
    };
    let root = tree.root_node();

    let ranges = driver.body_prune_ranges(&abs, source_text, source, root, language)?;
    let out = apply_replacements(source_text, ranges);
    Ok(Some(clean_skeleton_text(&abs, &out)))
}

pub struct LanguageConfig {
    drivers: Vec<Box<dyn LanguageDriver>>,
    by_ext: HashMap<String, usize>,
}

impl LanguageConfig {
    fn driver_for_path(&self, path: &Path) -> Option<&dyn LanguageDriver> {
        let ext = path_ext_lower(path);
        if let Some(&idx) = self.by_ext.get(&ext) {
            let d = self.drivers.get(idx).map(|x| x.as_ref());
            if let Some(d) = d {
                if d.handles_path(path) {
                    return Some(d);
                }
            }
        }

        // Fallback for special filename-based handling (e.g. `.d.ts`).
        self.drivers.iter().find(|d| d.handles_path(path)).map(|d| d.as_ref())
    }
}

impl Default for LanguageConfig {
    fn default() -> Self {
        let mut drivers: Vec<Box<dyn LanguageDriver>> = vec![
            Box::new(RustDriver),
            Box::new(TypeScriptDriver),
            Box::new(PythonDriver),
        ];

        #[cfg(feature = "lang-go")]
        drivers.push(Box::new(GoDriver));

        #[cfg(feature = "lang-dart")]
        drivers.push(Box::new(DartDriver));

        #[cfg(feature = "lang-java")]
        drivers.push(Box::new(JavaDriver));

        #[cfg(feature = "lang-csharp")]
        drivers.push(Box::new(CSharpDriver));

        #[cfg(feature = "lang-php")]
        drivers.push(Box::new(PhpDriver));

        #[cfg(feature = "lang-proto")]
        drivers.push(Box::new(ProtoDriver));

        let mut cfg = Self {
            drivers,
            by_ext: HashMap::new(),
        };

        for (idx, d) in cfg.drivers.iter().enumerate() {
            for ext in d.extensions() {
                cfg.by_ext.insert(ext.to_string(), idx);
            }
        }

        cfg
    }
}

fn language_config() -> &'static LanguageConfig {
    static CFG: OnceLock<LanguageConfig> = OnceLock::new();
    CFG.get_or_init(LanguageConfig::default)
}

fn path_ext_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase()
}

struct RustDriver;
impl LanguageDriver for RustDriver {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "rs"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_rust::language()
    }

    fn find_imports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        run_query_strings(source, root, &language, r#"(use_declaration argument: (_) @path)"#, "path")
    }

    fn find_exports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut exports: Vec<String> = Vec::new();
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(
                function_item
                                    (visibility_modifier) @vis
                  name: (identifier) @name
              )
              (#match? @vis \"^pub\")"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(
                struct_item
                                    (visibility_modifier) @vis
                  name: (type_identifier) @name
              )
              (#match? @vis \"^pub\")"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(
                enum_item
                                    (visibility_modifier) @vis
                  name: (type_identifier) @name
              )
              (#match? @vis \"^pub\")"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(
                trait_item
                                    (visibility_modifier) @vis
                  name: (type_identifier) @name
              )
              (#match? @vis \"^pub\")"#,
            "name",
        )?);
        Ok(exports)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        // Only function bodies. We do NOT prune impl/trait blocks; their methods will be pruned.
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/rust_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(function_item name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(struct_item name: (type_identifier) @name) @def"#,
            "struct",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(enum_item name: (type_identifier) @name) @def"#,
            "enum",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(trait_item name: (type_identifier) @name) @def"#,
            "trait",
            false,
        )?);
        Ok(symbols)
    }
}

struct TypeScriptDriver;
impl LanguageDriver for TypeScriptDriver {
    fn name(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        let ext = path_ext_lower(path);
        if matches!(ext.as_str(), "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs") {
            return true;
        }
        file_name_lower(path).ends_with(".d.ts")
    }

    fn language_for_path(&self, path: &Path) -> Language {
        let ext = path_ext_lower(path);
        if ext == "tsx" || ext == "jsx" {
            tree_sitter_typescript::language_tsx()
        } else {
            // JS/TS share the TypeScript grammar for our purposes.
            tree_sitter_typescript::language_typescript()
        }
    }

    fn find_imports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let import_srcs = run_query_strings(source, root, &language, r#"(import_statement source: (string) @src)"#, "src")?;
        Ok(import_srcs.into_iter().map(|s| strip_string_quotes(&s)).collect())
    }

    fn find_exports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut exports: Vec<String> = Vec::new();

        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(export_statement declaration: (function_declaration name: (identifier) @name))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(export_statement declaration: (class_declaration name: (type_identifier) @name))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name)))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(export_statement (export_clause (export_specifier name: (identifier) @name)))"#,
            "name",
        )?);

        Ok(exports)
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(function_declaration name: (identifier) @name) @def"#,
            "function",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @def"#,
            "function",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(class_declaration name: (type_identifier) @name) @def"#,
            "class",
            false,
        )?);

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_definition name: (property_identifier) @name) @def"#,
            "method",
            true,
        )?);

        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        // Focus on statement blocks for functions/methods. Skip arbitrary blocks.
        let mut out: Vec<(usize, usize, String)> = Vec::new();

        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/ts_prune.scm"), "body")?;
        for (s, e) in bodies {
            out.push((s, e, "{ /* ... */ }".to_string()));
        }
        Ok(out)
    }
}

struct PythonDriver;
impl LanguageDriver for PythonDriver {
    fn name(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "py"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_python::language()
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(function_definition name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(class_definition name: (identifier) @name) @def"#,
            "class",
            false,
        )?);
        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        // Replace function/class suite blocks with an indented "..." line.
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/py_prune.scm"), "body")?;
        let mut out: Vec<(usize, usize, String)> = Vec::new();
        for (s, e) in bodies {
            let indent = line_indent_at_byte(source_text, s);
            out.push((s, e, format!("{}...\n", indent)));
        }
        Ok(out)
    }
}

fn is_go_exported_ident(name: &str) -> bool {
    name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
}

#[cfg(feature = "lang-go")]
struct GoDriver;

#[cfg(feature = "lang-go")]
impl LanguageDriver for GoDriver {
    fn name(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "go"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_go::language()
    }

    fn find_imports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        out.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(import_spec (interpreted_string_literal) @src)"#,
            "src",
        )?);
        out.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(import_spec (raw_string_literal) @src)"#,
            "src",
        )?);
        Ok(out.into_iter().map(|s| strip_string_quotes(&s)).collect())
    }

    fn find_exports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut exports: Vec<String> = Vec::new();

        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(function_declaration name: (identifier) @name)"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(method_declaration name: (field_identifier) @name)"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(type_spec name: (type_identifier) @name)"#,
            "name",
        )?);

        exports.retain(|n| is_go_exported_ident(n));
        Ok(exports)
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(function_declaration name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_declaration name: (field_identifier) @name) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(type_spec name: (type_identifier) @name) @def"#,
            "type",
            false,
        )?);
        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/go_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

#[cfg(feature = "lang-dart")]
struct DartDriver;

#[cfg(feature = "lang-dart")]
impl LanguageDriver for DartDriver {
    fn name(&self) -> &'static str {
        "dart"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["dart"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "dart"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_dart::language()
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(class_definition name: (identifier) @name) @def"#,
            "class",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(enum_declaration name: (identifier) @name) @def"#,
            "enum",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(mixin_declaration (identifier) @name) @def"#,
            "mixin",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(extension_declaration name: (identifier) @name) @def"#,
            "extension",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(type_alias (type_identifier) @name) @def"#,
            "type",
            false,
        )?);

        // Top-level function signatures.
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(function_signature name: (identifier) @name) @def"#,
            "function",
            true,
        )?);

        // Method signatures inside classes/mixins/extensions.
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_signature (function_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_signature (getter_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_signature (setter_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_signature (constructor_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_signature (factory_constructor_signature (identifier) @name)) @def"#,
            "method",
            true,
        )?);

        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        // Dart function/method bodies are represented as `function_body` nodes.
        // We only prune block-bodied functions (skip `=> expr;` forms for now).
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/dart_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

#[cfg(feature = "lang-java")]
struct JavaDriver;

#[cfg(feature = "lang-java")]
impl LanguageDriver for JavaDriver {
    fn name(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "java"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_java::language()
    }

    fn find_imports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        // import java.util.Vector;
        // import static foo.Bar.*;
        let mut out: Vec<String> = Vec::new();
        out.extend(run_query_strings(
            source,
            root,
            &language,
            r#"(import_declaration (scoped_identifier) @path)"#,
            "path",
        )?);
        Ok(out)
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(class_declaration (identifier) @name) @def"#,
            "class",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(interface_declaration (identifier) @name) @def"#,
            "interface",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(enum_declaration name: (identifier) @name) @def"#,
            "enum",
            false,
        )?);

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(method_declaration (identifier) @name) @def"#,
            "method",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            &language,
            r#"(constructor_declaration (identifier) @name) @def"#,
            "constructor",
            true,
        )?);

        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/java_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

#[cfg(feature = "lang-csharp")]
struct CSharpDriver;

#[cfg(feature = "lang-csharp")]
impl LanguageDriver for CSharpDriver {
    fn name(&self) -> &'static str {
        "csharp"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "cs"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_c_sharp::language()
    }

    fn find_imports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        out.extend(run_query_strings(source, root, &language, r#"(using_directive (identifier) @path)"#, "path")?);
        out.extend(run_query_strings(source, root, &language, r#"(using_directive (qualified_name) @path)"#, "path")?);
        out.extend(run_query_strings(source, root, &language, r#"(using_directive (alias_qualified_name) @path)"#, "path")?);
        Ok(out)
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(source, root, &language, r#"(class_declaration name: (identifier) @name) @def"#, "class", false)?);
        symbols.extend(run_query(source, root, &language, r#"(struct_declaration name: (identifier) @name) @def"#, "struct", false)?);
        symbols.extend(run_query(source, root, &language, r#"(interface_declaration name: (identifier) @name) @def"#, "interface", false)?);
        symbols.extend(run_query(source, root, &language, r#"(enum_declaration name: (identifier) @name) @def"#, "enum", false)?);

        symbols.extend(run_query(source, root, &language, r#"(method_declaration name: (identifier) @name) @def"#, "method", true)?);
        symbols.extend(run_query(source, root, &language, r#"(constructor_declaration name: (identifier) @name) @def"#, "constructor", true)?);

        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/csharp_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

#[cfg(feature = "lang-php")]
struct PhpDriver;

#[cfg(feature = "lang-php")]
impl LanguageDriver for PhpDriver {
    fn name(&self) -> &'static str {
        "php"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "php"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_php::LANGUAGE_PHP.into()
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(source, root, &language, r#"(class_declaration name: (name) @name) @def"#, "class", false)?);
        symbols.extend(run_query(source, root, &language, r#"(interface_declaration name: (name) @name) @def"#, "interface", false)?);
        symbols.extend(run_query(source, root, &language, r#"(trait_declaration name: (name) @name) @def"#, "trait", false)?);

        symbols.extend(run_query(source, root, &language, r#"(function_definition name: (name) @name) @def"#, "function", true)?);
        symbols.extend(run_query(source, root, &language, r#"(method_declaration name: (name) @name) @def"#, "method", true)?);

        Ok(symbols)
    }

    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        source: &[u8],
        root: Node,
        language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        let bodies = run_query_byte_ranges(source, root, &language, include_str!("../queries/php_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Proto3 / Proto2 driver (tree-sitter-proto)
// ---------------------------------------------------------------------------
// Exposes services, messages, enums, and rpc methods for map_repo, read_symbol,
// find_usages, and call_hierarchy. No skeleton pruning needed — .proto files
// are already human-readable contracts without implementation bodies.

#[cfg(feature = "lang-proto")]
struct ProtoDriver;

#[cfg(feature = "lang-proto")]
impl LanguageDriver for ProtoDriver {
    fn name(&self) -> &'static str {
        "proto"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["proto"]
    }

    fn handles_path(&self, path: &Path) -> bool {
        path_ext_lower(path) == "proto"
    }

    fn language_for_path(&self, _path: &Path) -> Language {
        tree_sitter_proto::LANGUAGE.into()
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        // Top-level services
        symbols.extend(run_query(
            source, root, &language,
            r#"(service (service_name (identifier) @name)) @def"#,
            "service", false,
        )?);

        // Top-level messages
        symbols.extend(run_query(
            source, root, &language,
            r#"(message (message_name (identifier) @name)) @def"#,
            "message", false,
        )?);

        // Top-level enums
        symbols.extend(run_query(
            source, root, &language,
            r#"(enum (enum_name (identifier) @name)) @def"#,
            "enum", false,
        )?);

        // RPC methods inside services (pruned = true so they collapse in skeleton view)
        symbols.extend(run_query(
            source, root, &language,
            r#"(rpc (rpc_name (identifier) @name)) @def"#,
            "rpc", true,
        )?);

        Ok(symbols)
    }

    // Proto files have no function bodies to prune — return empty.
    fn body_prune_ranges(
        &self,
        _path: &Path,
        _source_text: &str,
        _source: &[u8],
        _root: Node,
        _language: Language,
    ) -> Result<Vec<(usize, usize, String)>> {
        Ok(vec![])
    }
}

fn run_query_byte_ranges(
    source: &[u8],
    root: Node,
    language: &Language,
    query_src: &str,
    cap: &str,
) -> Result<Vec<(usize, usize)>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();
    let mut out: Vec<(usize, usize)> = Vec::new();

    let mut matches = cursor.matches(&query, root, source);
    while let Some(m) = matches.next() {
        for cap0 in m.captures {
            let cap_name = query.capture_names()[cap0.index as usize];
            if cap_name != cap {
                continue;
            }
            out.push((cap0.node.start_byte(), cap0.node.end_byte()));
        }
    }

    Ok(out)
}

fn first_line_signature(def_text: &str) -> String {
    let mut s = def_text;
    if let Some(i) = s.find('{') {
        s = &s[..i];
    }
    if let Some(i) = s.find("\n") {
        s = &s[..i];
    }

    // Collapse whitespace for readability.
    let mut out = String::with_capacity(s.len().min(200));
    let mut prev_ws = false;
    for ch in s.chars() {
        let is_ws = ch.is_whitespace();
        if is_ws {
            if !prev_ws {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
        prev_ws = is_ws;
        if out.len() >= 240 {
            break;
        }
    }

    out.trim().trim_end_matches('{').trim().to_string()
}

fn node_text<'a>(source: &'a [u8], node: Node) -> &'a str {
    let start = node.start_byte();
    let end = node.end_byte();
    std::str::from_utf8(&source[start..end]).unwrap_or("")
}

fn strip_string_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 {
        let bytes = t.as_bytes();
        let first = bytes[0];
        let last = bytes[t.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') || (first == b'`' && last == b'`') {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

fn run_query_strings(source: &[u8], root: Node, language: &Language, query_src: &str, cap: &str) -> Result<Vec<String>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();

    let mut out: Vec<String> = Vec::new();
    let mut matches = cursor.matches(&query, root, source);
    while let Some(m) = matches.next() {
        for cap0 in m.captures {
            let cap_name = query.capture_names()[cap0.index as usize];
            if cap_name != cap {
                continue;
            }
            let text = node_text(source, cap0.node).trim().to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
    }
    Ok(out)
}

fn dedup_sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn run_query(
    source: &[u8],
    root: Node,
    language: &Language,
    query_src: &str,
    kind: &str,
    include_signature: bool,
) -> Result<Vec<Symbol>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();

    let mut out: Vec<Symbol> = Vec::new();

    let mut matches = cursor.matches(&query, root, source);
    while let Some(m) = matches.next() {
        let mut name_node: Option<Node> = None;
        let mut def_node: Option<Node> = None;

        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize];
            match cap_name {
                "name" => name_node = Some(cap.node),
                "def" => def_node = Some(cap.node),
                _ => {}
            }
        }

        let Some(name_node) = name_node else { continue };
        let def_node = def_node.unwrap_or(name_node);

        let name = node_text(source, name_node).trim().to_string();
        if name.is_empty() {
            continue;
        }

        let start = def_node.start_position();
        let end = def_node.end_position();

        let signature = if include_signature {
            let def_text = node_text(source, def_node);
            Some(first_line_signature(def_text))
        } else {
            None
        };

        out.push(Symbol {
            name,
            kind: kind.to_string(),
            line: start.row as u32,
            line_end: end.row as u32,
            signature,
        });
    }

    Ok(out)
}

/// Parse a single file and extract symbols (functions/structs/classes) using tree-sitter.
///
/// - Lines are 0-indexed.
/// - `file` is emitted as the provided path string (normalized to '/').
pub fn analyze_file(path: &Path) -> Result<FileSymbols> {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get current dir")?.join(path)
    };

    let driver = language_config()
        .driver_for_path(&abs)
        .ok_or_else(|| anyhow!("Unsupported file extension: {}", abs.display()))?;
    let language = driver.language_for_path(&abs);

    let source_text = std::fs::read_to_string(&abs)
        .with_context(|| format!("Failed to read {}", abs.display()))?;
    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Failed to set tree-sitter language")?;

    let tree = parser
        .parse(source_text.as_str(), None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;

    let root = tree.root_node();

    let mut symbols = driver.extract_skeleton(&abs, source, root, language.clone())?;
    let mut imports = driver.find_imports(&abs, source, root, language.clone())?;
    let mut exports = driver.find_exports(&abs, source, root, language)?;

    // Stable ordering: by line then name.
    symbols.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));

    imports = dedup_sorted(imports);
    exports = dedup_sorted(exports);

    Ok(FileSymbols {
        file: normalize_path_for_output(path),
        imports,
        exports,
        symbols,
    })
}

/// Extract all top-level symbols from source text without a disk read.
///
/// Used by the vector store for:
///  1. AST-aware chunk boundary detection (group `chunk_lines` of symbols per chunk).
///  2. Symbol anchoring: store symbol names in the index so search can boost exact matches.
///
/// Returns an empty vec for unsupported file types (graceful fallback to line-chunking).
pub fn extract_symbols_from_source(path: &Path, source_text: &str) -> Vec<Symbol> {
    if is_minified_or_generated(source_text) {
        return vec![];
    }

    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => return vec![],
        }
    };

    let Some(driver) = language_config().driver_for_path(&abs) else {
        return vec![];
    };

    let language = driver.language_for_path(&abs);
    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return vec![];
    }

    let Some(tree) = parser.parse(source_text, None) else {
        return vec![];
    };

    let root = tree.root_node();

    match driver.extract_skeleton(&abs, source, root, language) {
        Ok(mut syms) => {
            syms.sort_by(|a, b| a.line.cmp(&b.line));
            syms
        }
        Err(_) => vec![],
    }
}

// ---------------------------------------------------------------------------
// Tool: read_symbol — The X-Ray
// ---------------------------------------------------------------------------

/// Extract the full, unpruned source of a specific named symbol from `path`.
///
/// Uses tree-sitter to locate the exact declaration node — bodies are never pruned.
/// For Rust files `impl Foo` blocks are also searchable even though they are omitted
/// from the standard skeleton.
///
/// Returns a header line followed by the raw source text:
/// ```text
/// // fn `process` — src/handler.rs:L42-L78
/// pub fn process(...) {
///     ...
/// }
/// ```
pub fn read_symbol(path: &Path, symbol_name: &str) -> Result<String> {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get cwd")?.join(path)
    };

    let raw = std::fs::read(&abs)
        .with_context(|| format!("Failed to read {}", abs.display()))?;
    if raw.contains(&0u8) {
        return Err(anyhow!("Binary file — cannot extract symbol"));
    }
    let source_text = String::from_utf8_lossy(&raw).into_owned();

    let Some(driver) = language_config().driver_for_path(&abs) else {
        return Err(anyhow!(
            "Unsupported file type: {}",
            abs.extension().and_then(|e| e.to_str()).unwrap_or("?")
        ));
    };
    let language = driver.language_for_path(&abs);
    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser.set_language(&language).context("Failed to set tree-sitter language")?;
    let tree = parser
        .parse(&source_text, None)
        .ok_or_else(|| anyhow!("Tree-sitter parse failed for {}", abs.display()))?;
    let root = tree.root_node();

    // ── Step 1: gather all named declarations with byte offsets ──────────
    let offsets = line_byte_offsets(&source_text);
    let mut candidates: Vec<(String, String, usize, usize)> = Vec::new(); // (name, kind, start, end)

    // Standard symbols from the driver (fn, struct, enum, trait, class, method…)
    if let Ok(syms) = driver.extract_skeleton(&abs, source, root, language.clone()) {
        for sym in &syms {
            let start = offsets.get(sym.line as usize).copied().unwrap_or(0);
            let end = if (sym.line_end as usize + 1) < offsets.len() {
                offsets[sym.line_end as usize + 1]
            } else {
                source_text.len()
            };
            candidates.push((sym.name.clone(), sym.kind.clone(), start, end));
        }
    }

    // For Rust: also include `impl` blocks (not returned by extract_skeleton).
    if driver.name() == "rust" {
        let impl_blocks = rust_impl_byte_ranges(source, root, &language);
        candidates.extend(impl_blocks);
    }

    // ── Step 2: find best match (exact → case-insensitive) ───────────────
    let found = candidates
        .iter()
        .find(|(name, _, _, _)| name == symbol_name)
        .or_else(|| {
            candidates
                .iter()
                .find(|(name, _, _, _)| name.eq_ignore_ascii_case(symbol_name))
        });

    let Some((name, kind, start_byte, end_byte)) = found else {
        let mut available: Vec<String> = candidates
            .iter()
            .map(|(n, k, _, _)| format!("  {k} {n}"))
            .collect();
        available.sort();
        return Err(anyhow!(
            "Symbol `{}` not found in {}.\nAvailable symbols:\n{}",
            symbol_name,
            abs.display(),
            available.join("\n")
        ));
    };

    // ── Step 3: format and return ─────────────────────────────────────────
    let body = &source_text[*start_byte..*end_byte];
    let start_line = source_text[..*start_byte].bytes().filter(|&b| b == b'\n').count() + 1;
    let end_line = source_text[..*end_byte].bytes().filter(|&b| b == b'\n').count() + 1;

    Ok(format!(
        "// {kind} `{name}` — {}:L{start_line}-L{end_line}\n{body}",
        abs.display()
    ))
}

/// Compute byte offset of the start of each line (0-indexed).
fn line_byte_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            offsets.push(i + 1);
        }
    }
    offsets
}

/// Run a tree-sitter query with `@name` / `@def` captures and return
/// `(name_text, start_byte, end_byte)` tuples.
fn find_named_decls_raw(
    source: &[u8],
    root: Node,
    language: &Language,
    query_src: &str,
) -> Vec<(String, usize, usize)> {
    let Ok(query) = Query::new(language, query_src) else {
        return vec![];
    };
    let mut cursor = QueryCursor::new();
    let mut out: Vec<(String, usize, usize)> = Vec::new();

    let mut matches = cursor.matches(&query, root, source);
    while let Some(m) = matches.next() {
        let mut name_text = String::new();
        let mut def_start = 0usize;
        let mut def_end = 0usize;
        let mut has_def = false;

        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize];
            match cap_name {
                "name" => {
                    name_text = std::str::from_utf8(
                        &source[cap.node.start_byte()..cap.node.end_byte()],
                    )
                    .unwrap_or("")
                    .trim()
                    .to_string();
                }
                "def" => {
                    def_start = cap.node.start_byte();
                    def_end = cap.node.end_byte();
                    has_def = true;
                }
                _ => {}
            }
        }

        if !name_text.is_empty() && has_def {
            out.push((name_text, def_start, def_end));
        }
    }
    out
}

/// Find Rust `impl` blocks by byte range.
/// Returns `(name, "impl", start_byte, end_byte)` tuples.
fn rust_impl_byte_ranges(
    source: &[u8],
    root: Node,
    language: &Language,
) -> Vec<(String, String, usize, usize)> {
    let mut out: Vec<(String, String, usize, usize)> = Vec::new();

    // impl Foo { ... }
    for (name, start, end) in find_named_decls_raw(
        source,
        root,
        language,
        r#"(impl_item type: (type_identifier) @name) @def"#,
    ) {
        out.push((name, "impl".to_string(), start, end));
    }

    // impl<T> Foo<T> { ... }
    for (name, start, end) in find_named_decls_raw(
        source,
        root,
        language,
        r#"(impl_item type: (generic_type type: (type_identifier) @name)) @def"#,
    ) {
        out.push((name, "impl".to_string(), start, end));
    }

    out
}

// ---------------------------------------------------------------------------
// Tool: find_usages — The AST-Tracer
// ---------------------------------------------------------------------------

/// Find all semantic usages of `symbol_name` across code files under `target_dir`.
///
/// Algorithm:
///  1. Walk `target_dir` with `ignore::WalkBuilder` (honours `.gitignore`).
///  2. For each supported-language file containing `symbol_name` as a substring
///     (fast pre-filter), parse with tree-sitter.
///  3. Recursively visit AST leaf nodes: collect `identifier`, `type_identifier`,
///     `field_identifier`, `property_identifier` nodes whose text == `symbol_name`.
///  4. Prune entire comment / string subtrees — zero false positives from docs or
///     string constants.
///  5. Return a dense listing with 2-line context windows.
///
/// Works even when the project currently **fails to compile** because it uses the
/// raw AST, not an LSP or compiler.
pub fn find_usages(target_dir: &Path, symbol_name: &str) -> Result<String> {
    use ignore::WalkBuilder;

    let abs_dir: PathBuf = if target_dir.is_absolute() {
        target_dir.to_path_buf()
    } else {
        std::env::current_dir().context("Failed to get cwd")?.join(target_dir)
    };

    let walker = WalkBuilder::new(&abs_dir)
        .standard_filters(true) // respects .gitignore, .git/info/exclude, default ignores
        .hidden(true)            // skip dot-dirs like .git, node_modules handled by standard_filters
        .build();

    let cfg = language_config();
    let mut all_results: Vec<UsageMatch> = Vec::new();

    for entry_result in walker {
        let Ok(entry) = entry_result else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Only process files with a supported language driver.
        if cfg.driver_for_path(path).is_none() {
            continue;
        }

        let Ok(raw) = std::fs::read(path) else { continue };
        if raw.contains(&0u8) {
            continue; // binary
        }
        let Ok(source_text) = std::str::from_utf8(&raw) else { continue };

        // Hot path: fast substring pre-filter before paying the tree-sitter parse cost.
        if !source_text.contains(symbol_name) {
            continue;
        }

        let Some(driver) = cfg.driver_for_path(path) else { continue };
        let language = driver.language_for_path(path);
        let source = source_text.as_bytes();

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(source_text, None) else { continue };
        let root = tree.root_node();

        // AST-level reference collection — excludes comments and string literals.
        let mut hit_rows: Vec<u32> = Vec::new();
        collect_identifier_refs(root, source, symbol_name, &mut hit_rows);

        if hit_rows.is_empty() {
            continue;
        }

        hit_rows.sort();
        hit_rows.dedup();

        let text_lines: Vec<&str> = source_text.lines().collect();
        let display_path = path.to_string_lossy();

        for row_0 in hit_rows {
            all_results.push(UsageMatch {
                file: display_path.to_string(),
                line_1: row_0 + 1,
                context: extract_context_lines(&text_lines, row_0 as usize, 2),
            });
        }
    }

    if all_results.is_empty() {
        return Ok(format!(
            "No usages of `{}` found in {}.",
            symbol_name,
            abs_dir.display()
        ));
    }

    let mut out = format!("{} usage(s) of `{symbol_name}` found:\n\n", all_results.len());
    for m in &all_results {
        out.push_str(&format!("[{}:{}]\n", m.file, m.line_1));
        out.push_str(&format!("Context:\n{}\n\n", m.context));
    }
    Ok(out)
}

struct UsageMatch {
    file: String,
    line_1: u32,
    context: String,
}

/// Recursively collect AST leaf identifier nodes that match `symbol_name`,
/// skipping comment and string-literal subtrees entirely.
fn collect_identifier_refs(node: Node, source: &[u8], symbol_name: &str, out: &mut Vec<u32>) {
    let kind = node.kind();

    // Prune entire comment / string subtrees — no matches inside these nodes.
    if kind.contains("comment")
        || matches!(
            kind,
            "string"
                | "string_literal"
                | "raw_string"
                | "raw_string_literal"
                | "interpreted_string_literal"
                | "char_literal"
                | "template_string"
                | "string_fragment"
                | "heredoc_body"
                | "regex_pattern"
        )
    {
        return;
    }

    // For leaf nodes: check if this is a semantic identifier matching the target.
    if node.child_count() == 0 {
        if matches!(
            kind,
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "shorthand_property_identifier"
                | "shorthand_property_identifier_pattern"
        ) {
            let slice = &source[node.start_byte()..node.end_byte()];
            if let Ok(text) = std::str::from_utf8(slice) {
                if text == symbol_name {
                    out.push(node.start_position().row as u32);
                }
            }
        }
        return;
    }

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_refs(child, source, symbol_name, out);
    }
}

/// Build a 2×`ctx`-line context block around `target_0` (0-indexed), marking the
/// hit line with `>>>`.
fn extract_context_lines(lines: &[&str], target_0: usize, ctx: usize) -> String {
    let start = target_0.saturating_sub(ctx);
    let end = (target_0 + ctx + 1).min(lines.len());
    lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let ln = start + i + 1;
            let marker = if start + i == target_0 { ">>>" } else { "   " };
            format!("  {marker} {:>4} | {}", ln, l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tool: map_repo — The God's Eye View
// ---------------------------------------------------------------------------

/// Build a human-readable hierarchical text map of the codebase showing file
/// paths and their **exported / public symbols** only.
///
/// Designed for LLM consumption: compact, unambiguous, and token-budgeted.
/// Output is grouped by directory. The total output is capped at ~8 000 chars.
///
/// # Arguments
/// * `target_dir` — root directory to walk (respects `.gitignore`)
///
/// # Output example
/// ```text
/// project/   (12 files)
///
/// src/
///   handler.rs
///     [fn      ] handle_request
///     [fn      ] handle_response
///   models/
///     user.rs
///       [struct  ] User
/// ```
pub fn repo_map(target_dir: &Path) -> Result<String> {
    use ignore::WalkBuilder;
    use std::collections::BTreeMap;

    const MAX_SYMS_PER_FILE: usize = 20;
    const MAX_CHARS_TOTAL: usize = 8_000;

    let abs_dir: PathBuf = if target_dir.is_absolute() {
        target_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to get cwd")?
            .join(target_dir)
    };

    let walker = WalkBuilder::new(&abs_dir)
        .standard_filters(true)
        .hidden(true)
        .build();

    let cfg = language_config();

    // dir_rel → Vec<(filename, Vec<(kind, name)>)>
    let mut by_dir: BTreeMap<String, Vec<(String, Vec<(String, String)>)>> = BTreeMap::new();

    for entry_result in walker {
        let Ok(entry) = entry_result else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if cfg.driver_for_path(path).is_none() {
            continue;
        }

        let rel_from_target = match path.strip_prefix(&abs_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let dir_rel = rel_from_target
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Read source once; use extract_symbols_from_source (never fails on bad
        // queries) then filter to "public" symbols with a language-aware heuristic.
        let sym_pairs: Vec<(String, String)> = if let Ok(source_text) = std::fs::read_to_string(path) {
            let syms = extract_symbols_from_source(path, &source_text);
            let source_lines: Vec<&str> = source_text.lines().collect();

            syms.into_iter()
                .filter(|s| is_public_symbol(s, &source_lines, path))
                .take(MAX_SYMS_PER_FILE)
                .map(|s| (s.kind.clone(), s.name.clone()))
                .collect()
        } else {
            vec![]
        };

        by_dir.entry(dir_rel).or_default().push((filename, sym_pairs));
    }

    let mut out = String::new();
    let root_name = abs_dir
        .file_name()
        .unwrap_or_else(|| abs_dir.as_os_str())
        .to_string_lossy();
    let total_files: usize = by_dir.values().map(|v| v.len()).sum();
    out.push_str(&format!("{root_name}/   ({total_files} files)\n"));

    for (dir_rel, mut files) in by_dir {
        files.sort_by(|a, b| a.0.cmp(&b.0));

        if !dir_rel.is_empty() {
            out.push_str(&format!("\n{dir_rel}/\n"));
        }

        for (filename, syms) in &files {
            out.push_str(&format!("  {filename}\n"));
            for (kind, name) in syms {
                out.push_str(&format!("    [{:<8}] {name}\n", kind));
            }
        }

        if out.len() > MAX_CHARS_TOTAL {
            out.push_str("\n... (output truncated — token limit reached)\n");
            break;
        }
    }

    Ok(out)
}

/// Determine whether a symbol should be considered "public" for repo_map display.
///
/// Uses a fast source-line heuristic rather than AST predicates so it never fails.
/// - **Rust**: declaration line contains `pub ` or `pub(`
/// - **Python**: name does not start with `_`
/// - **Go**: name starts with an ASCII upper-case letter
/// - **TypeScript/JS**: show all top-level symbols (exports are shown by TS driver,
///   but here we always include since we're doing a map, not a strict export list)
/// - **Everything else**: include all symbols
fn is_public_symbol(sym: &Symbol, source_lines: &[&str], path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "rs" => {
            // For agentic repo mapping, private Rust symbols are often just as
            // useful as `pub` ones. Also, attribute/doc/macro lines can precede
            // the actual declaration, making naive `pub` string checks brittle.
            //
            // Intentionally bypass public-only filtering for Rust.
            let _ = (sym, source_lines);
            true
        }
        "py" => !sym.name.starts_with('_'),
        "go" => sym
            .name
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false),
        // TypeScript/JS/Java/C#/Dart/PHP — include everything
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Tool: call_hierarchy — The Call Graph
// ---------------------------------------------------------------------------

/// Language-agnostic deny-list of common stdlib / runtime method names that
/// produce noise in the outgoing call list without conveying domain intent.
///
/// Covers the most frequent offenders across Rust, Python, TypeScript, and Go.
/// Names are exact (case-sensitive); entries are checked with `contains()`.
static CALL_NOISE: &[&str] = &[
    // Rust — core/std
    "clone", "to_string", "to_owned", "into", "from", "default",
    "trim", "trim_start", "trim_end", "to_lowercase", "to_uppercase",
    "is_empty", "is_some", "is_none", "len", "push", "pop", "clear",
    "iter", "iter_mut", "into_iter", "collect", "map", "filter",
    "flat_map", "filter_map", "fold", "reduce", "any", "all", "find",
    "next", "take", "skip", "enumerate", "zip", "chain", "rev",
    "unwrap", "unwrap_or", "unwrap_or_else", "expect",
    "ok", "err", "ok_or", "ok_or_else", "and_then", "or_else",
    "as_ref", "as_mut", "as_str", "as_bytes", "as_slice", "as_deref",
    "to_str", "to_path_buf", "to_string_lossy",
    "contains", "starts_with", "ends_with", "split", "splitn",
    "find", "rfind", "replace", "replacen",
    "push_str",
    "get", "set", "insert", "remove", "retain",
    "join", "extend", "append", "truncate", "resize",
    "new", "with_capacity", "capacity",
    "path", "file_name", "parent", "extension", "exists", "is_file", "is_dir",
    "read_to_string", "read_dir", "create_dir_all",
    "send", "recv", "await", "spawn", "block_on",
    "context", "with_context", "map_err",
    "lock", "try_lock", "read", "write",
    "format", "parse", "lines", "chars", "bytes",
    "sort", "sort_by", "sort_by_key", "dedup",
    "first", "last", "nth",
    "min", "max", "min_by", "max_by", "min_by_key", "max_by_key",
    "sum", "product", "count", "position",
    "flush", "close",
    // Python builtins / common methods
    "append", "extend", "update", "keys", "values", "items",
    "strip", "lstrip", "rstrip", "lower", "upper",
    "encode", "decode", "format",
    "isinstance", "hasattr", "getattr", "setattr",
    "open", "print", "len", "range", "enumerate", "zip",
    "list", "dict", "set", "tuple", "str", "int", "float", "bool",
    "super", "type",
    // TypeScript/JavaScript
    "toString", "valueOf", "hasOwnProperty", "bind", "call", "apply",
    "then", "catch", "finally",
    "reduce", "forEach", "some", "every", "includes", "indexOf",
    "slice", "splice", "concat", "flat", "flatMap",
    "trim", "split", "replace", "match", "test",
    "JSON",
    // Go
    "Error", "String", "Len",
];

/// Analyse the complete call hierarchy for a named symbol.
///
/// Returns three sections:
/// - **Definition** — file and line where the symbol is declared.
/// - **Outgoing calls** — identifiers called *from within* the symbol's body,
///   extracted via `call_expression` / `method_call_expression` AST nodes.
/// - **Incoming calls** — files and enclosing functions that call this symbol,
///   located by scanning every supported source file under `target_dir`.
///
/// Works without compilation — uses the raw tree-sitter AST, so it operates
/// even on partially broken code.
///
/// # Arguments
/// * `target_dir`   — directory to search (respects `.gitignore`)
/// * `symbol_name`  — exact symbol name (case-sensitive)
pub fn call_hierarchy(target_dir: &Path, symbol_name: &str) -> Result<String> {
    use ignore::WalkBuilder;

    let abs_dir: PathBuf = if target_dir.is_absolute() {
        target_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to get cwd")?
            .join(target_dir)
    };

    let cfg = language_config();

    struct DefSite {
        file: String,
        line_1: u32,
        kind: String,
    }

    let mut definitions: Vec<DefSite> = Vec::new();
    let mut outgoing_calls: Vec<(String, u32, String)> = Vec::new(); // (callee, abs_line_1, file)
    let mut callers: Vec<(String, u32, Option<String>, String)> = Vec::new(); // (file, line_1, enclosing, ctx)

    let walker = WalkBuilder::new(&abs_dir)
        .standard_filters(true)
        .hidden(true)
        .build();

    for entry_result in walker {
        let Ok(entry) = entry_result else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if cfg.driver_for_path(path).is_none() {
            continue;
        }

        let Ok(raw) = std::fs::read(path) else { continue };
        if raw.contains(&0u8) {
            continue;
        }
        let Ok(source_text) = std::str::from_utf8(&raw) else { continue };
        if !source_text.contains(symbol_name) {
            continue;
        }

        let driver = cfg.driver_for_path(path).unwrap();
        let language = driver.language_for_path(path);
        let source = source_text.as_bytes();

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(source_text, None) else { continue };
        let root = tree.root_node();

        let text_lines: Vec<&str> = source_text.lines().collect();
        let display_path = path.to_string_lossy().to_string();

        // Extract skeleton (symbol list) for this file — used for definition
        // detection AND for resolving enclosing function context.
        let syms = match driver.extract_skeleton(path, source, root, language.clone()) {
            Ok(s) => s,
            Err(_) => vec![],
        };

        // 1) Definitions + outgoing calls from definition body
        for sym in &syms {
            if sym.name != symbol_name {
                continue;
            }
            definitions.push(DefSite {
                file: display_path.clone(),
                line_1: sym.line + 1,
                kind: sym.kind.clone(),
            });

            // Re-parse the definition body text to extract outgoing call targets.
            let body_start = sym.line as usize;
            let body_end = (sym.line_end as usize + 1).min(text_lines.len());
            let body_text: String = text_lines[body_start..body_end].join("\n");
            let body_bytes = body_text.as_bytes();

            let mut body_parser = Parser::new();
            if body_parser.set_language(&language).is_ok() {
                if let Some(body_tree) = body_parser.parse(&body_text, None) {
                    let body_root = body_tree.root_node();
                    let mut raw_calls: Vec<(String, u32)> = Vec::new();
                    extract_call_targets_from_body(body_root, body_bytes, &mut raw_calls);
                    for (callee, li_in_body) in raw_calls {
                        let abs_line_1 = sym.line + 1 + li_in_body;
                        outgoing_calls.push((callee, abs_line_1, display_path.clone()));
                    }
                }
            }
        }

        // 2) Incoming call sites — find call_expression nodes targeting symbol_name
        let mut call_rows: Vec<u32> = Vec::new();
        collect_call_refs(root, source, symbol_name, &mut call_rows);
        call_rows.sort();
        call_rows.dedup();

        for row_0 in call_rows {
            // Find the tightest enclosing function/method
            let enclosing = syms
                .iter()
                .filter(|s| {
                    s.line <= row_0
                        && row_0 <= s.line_end
                        && matches!(
                            s.kind.as_str(),
                            "fn" | "function" | "method" | "arrow_function"
                        )
                })
                .min_by_key(|s| row_0 - s.line)
                .map(|s| format!("{} {}()", s.kind, s.name));

            let ctx = extract_context_lines(&text_lines, row_0 as usize, 2);
            callers.push((display_path.clone(), row_0 + 1, enclosing, ctx));
        }
    }

    // ── Format Markdown output ────────────────────────────────────────────
    let mut out = format!("## Call Hierarchy: `{symbol_name}`\n\n");

    if definitions.is_empty() {
        out.push_str(
            "> No definition found in target_dir — showing inbound call sites only.\n\n",
        );
    } else {
        out.push_str("### Definition\n");
        for d in &definitions {
            out.push_str(&format!("- `{}` at {}:L{}\n", d.kind, d.file, d.line_1));
        }
        out.push('\n');
    }

    out.push_str("### Outgoing Calls (made by this symbol)\n");
    if outgoing_calls.is_empty() {
        out.push_str("- *(none detected)*\n");
    } else {
        outgoing_calls.sort_by_key(|(_, line, _)| *line);
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (callee, line, file) in &outgoing_calls {
            // Skip common stdlib / language-runtime noise that produces no signal.
            if CALL_NOISE.contains(&callee.as_str()) {
                continue;
            }
            if seen.insert(callee.clone()) {
                out.push_str(&format!("- `{callee}` — {file}:L{line}\n"));
            }
        }
        if seen.is_empty() {
            out.push_str("- *(stdlib/built-in methods only — no domain calls detected)*\n");
        }
    }
    out.push('\n');

    const MAX_CALLERS: usize = 30;
    out.push_str("### Incoming Calls (callers of this symbol)\n");
    if callers.is_empty() {
        out.push_str("- *(none detected)*\n");
    } else {
        for (file, line_1, enclosing, ctx) in callers.iter().take(MAX_CALLERS) {
            let enc_str = enclosing.as_deref().unwrap_or("(top-level)");
            out.push_str(&format!("\n**{file}:{line_1}** in `{enc_str}`\n"));
            out.push_str(&format!("```\n{ctx}\n```\n"));
        }
        if callers.len() > MAX_CALLERS {
            out.push_str(&format!(
                "\n*... {} more callers not shown*\n",
                callers.len() - MAX_CALLERS
            ));
        }
    }

    Ok(out)
}

/// Collect all call sites of `symbol_name` by walking the AST for call nodes
/// whose callable resolves to `symbol_name` as the trailing identifier.
///
/// Handles:
/// - `call_expression` — Rust / TypeScript / JavaScript
/// - `method_call_expression` — Rust
/// - `call` — Python (direct call and attribute call)
fn collect_call_refs(node: Node, source: &[u8], symbol_name: &str, out: &mut Vec<u32>) {
    let kind = node.kind();
    if kind.contains("comment") || kind.contains("string") || kind.contains("template") {
        return;
    }

    if matches!(kind, "call_expression" | "method_call_expression" | "call") {
        // Field "function" covers Rust/TS/JS call_expression and Python call.
        // Field "method" covers Rust method_call_expression.
        let target_node = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("method"))
            .or_else(|| node.child_by_field_name("name"));

        if let Some(target) = target_node {
            if let Some(last) = extract_trailing_call_identifier(target, source) {
                if last == symbol_name {
                    out.push(node.start_position().row as u32);
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_call_refs(child, source, symbol_name, out);
    }
}

/// Extract all outgoing call targets from an AST subtree (typically a function
/// body). Returns `(callee_name, 0-indexed_line_in_body)` pairs.
///
/// Handles Rust `call_expression` / `method_call_expression`, TypeScript
/// `call_expression`, and Python `call`.
fn extract_call_targets_from_body(node: Node, source: &[u8], out: &mut Vec<(String, u32)>) {
    let kind = node.kind();
    if kind.contains("comment") || kind.contains("string") || kind.contains("template") {
        return;
    }

    if matches!(kind, "call_expression" | "method_call_expression" | "call") {
        let target_node = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("method"))
            .or_else(|| node.child_by_field_name("name"));

        if let Some(target) = target_node {
            if let Some(last) = extract_trailing_call_identifier(target, source) {
                out.push((last.to_string(), node.start_position().row as u32));
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_call_targets_from_body(child, source, out);
    }
}

fn extract_trailing_call_identifier<'a>(target: Node, source: &'a [u8]) -> Option<&'a str> {
    // Python: `call` nodes use `function:`. For method calls `obj.method()`,
    // that function field is an `attribute` node and the trailing identifier is
    // stored in the `attribute:` field (not `name:`).
    if target.kind() == "attribute" {
        if let Some(attr) = target.child_by_field_name("attribute") {
            let text = std::str::from_utf8(&source[attr.start_byte()..attr.end_byte()]).ok()?;
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }

    // Fallback: use the full slice and strip module/attribute/namespace prefixes.
    let text = std::str::from_utf8(&source[target.start_byte()..target.end_byte()]).ok()?;
    let last = text
        .rsplit(|c: char| c == '.' || c == ':')
        .next()
        .unwrap_or("")
        .trim();

    if last.is_empty() {
        return None;
    }
    if !last.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(last)
}

// ---------------------------------------------------------------------------
// Tool: run_diagnostics — The Compiler Oracle
// ---------------------------------------------------------------------------

/// Run the project's native diagnostics tool and return a structured report
/// of errors and warnings, each pinned to its source location with inline
/// code context.
///
/// **Project detection:**
/// - `Cargo.toml` present → `cargo check --message-format=json --quiet`
/// - `package.json` present → `npx tsc --noEmit --pretty false`
///
/// Errors are capped at 20; warnings at 10. Each entry includes a 1-line
/// code context window extracted from the source file.
///
/// # Arguments
/// * `repo_root` — root directory of the project
pub fn run_diagnostics(repo_root: &Path) -> Result<String> {
    use std::process::{Command, Stdio};

    let abs_root: PathBuf = if repo_root.is_absolute() {
        repo_root.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to get cwd")?
            .join(repo_root)
    };

    let has_cargo = abs_root.join("Cargo.toml").exists();
    let has_package_json = abs_root.join("package.json").exists();

    if !has_cargo && !has_package_json {
        return Ok(format!(
            "No Cargo.toml or package.json found in {}.\n\
             `run_diagnostics` supports Rust (`cargo check`) and \
             TypeScript (`tsc --noEmit`) projects.",
            abs_root.display()
        ));
    }

    if has_cargo {
        let output = Command::new("cargo")
            .args(["check", "--message-format=json", "--quiet"])
            .current_dir(&abs_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to run `cargo check` — is Rust installed?")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        diagnostics_parse_cargo(&stdout, &abs_root)
    } else {
        let output = Command::new("npx")
            .args(["tsc", "--noEmit", "--pretty", "false"])
            .current_dir(&abs_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to run `npx tsc` — is TypeScript installed?")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        diagnostics_parse_tsc(&stdout, &stderr)
    }
}

fn diagnostics_parse_cargo(cargo_output: &str, repo_root: &Path) -> Result<String> {
    use serde_json::Value;

    const MAX_ERRORS: usize = 20;
    const MAX_WARNINGS: usize = 10;

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for line in cargo_output.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if json.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let Some(msg) = json.get("message") else {
            continue;
        };
        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("unknown");
        if level != "error" && level != "warning" {
            continue;
        }

        let message_text = msg
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)");
        let code_str = msg
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(|c| format!("[{c}] "))
            .unwrap_or_default();

        let spans = msg.get("spans").and_then(|s| s.as_array());
        let mut location = String::new();
        let mut context_block = String::new();

        if let Some(spans_arr) = spans {
            if let Some(span) = spans_arr.first() {
                let file = span.get("file_name").and_then(|f| f.as_str()).unwrap_or("?");
                let line_start = span
                    .get("line_start")
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0);
                let col = span
                    .get("column_start")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                location = format!("{file}:{line_start}:{col}");

                if let Ok(contents) = std::fs::read_to_string(repo_root.join(file)) {
                    let text_lines: Vec<&str> = contents.lines().collect();
                    let target_0 = (line_start as usize).saturating_sub(1);
                    context_block = extract_context_lines(&text_lines, target_0, 1);
                }
            }
        }

        let mut entry = format!("**{level}**: {code_str}{message_text}\n  → {location}");
        if !context_block.is_empty() {
            entry.push_str(&format!("\n```\n{context_block}\n```"));
        }

        if level == "error" {
            errors.push(entry);
        } else {
            warnings.push(entry);
        }
    }

    if errors.is_empty() && warnings.is_empty() {
        return Ok("Project compiles cleanly — no errors or warnings.\n".to_string());
    }

    let mut out = String::new();

    if !errors.is_empty() {
        out.push_str(&format!(
            "## Errors ({} total, showing up to {MAX_ERRORS})\n\n",
            errors.len()
        ));
        for (i, e) in errors.iter().enumerate().take(MAX_ERRORS) {
            out.push_str(&format!("### Error {}\n{e}\n\n", i + 1));
        }
        if errors.len() > MAX_ERRORS {
            out.push_str(&format!(
                "*... {} more errors not shown*\n\n",
                errors.len() - MAX_ERRORS
            ));
        }
    }

    if !warnings.is_empty() {
        out.push_str(&format!(
            "## Warnings ({} total, showing up to {MAX_WARNINGS})\n\n",
            warnings.len()
        ));
        for w in warnings.iter().take(MAX_WARNINGS) {
            out.push_str(&format!("{w}\n\n"));
        }
        if warnings.len() > MAX_WARNINGS {
            out.push_str(&format!(
                "*... {} more warnings not shown*\n",
                warnings.len() - MAX_WARNINGS
            ));
        }
    }

    Ok(out)
}

fn diagnostics_parse_tsc(stdout: &str, stderr: &str) -> Result<String> {
    let combined = if stdout.trim().is_empty() { stderr } else { stdout };
    if combined.trim().is_empty() {
        return Ok("No TypeScript errors found — project compiles cleanly.\n".to_string());
    }

    let mut out = String::from("## TypeScript Diagnostics\n\n");
    let mut count = 0usize;
    const MAX_TSC: usize = 20;

    for line in combined.lines() {
        if count >= MAX_TSC {
            break;
        }
        let t = line.trim();
        if t.contains(": error TS") || t.contains(": warning TS") {
            out.push_str(&format!("- {t}\n"));
            count += 1;
        }
    }

    if count == 0 {
        // Fallback: include raw output (truncated)
        let snippet = &combined[..combined.len().min(3_000)];
        out.push_str(snippet);
    }

    Ok(out)
}
