use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

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

    let source_text = std::fs::read_to_string(&abs)
        .with_context(|| format!("Failed to read {}", abs.display()))?;
    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(language)
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

    let driver = language_config()
        .driver_for_path(&abs)
        .ok_or_else(|| anyhow!("Unsupported file extension: {}", abs.display()))?;
    let language = driver.language_for_path(&abs);

    let source = source_text.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(language)
        .context("Failed to set tree-sitter language")?;
    let tree = parser
        .parse(source_text, None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;
    let root = tree.root_node();

    let ranges = driver.body_prune_ranges(&abs, source_text, source, root, language)?;
    let out = apply_replacements(source_text, ranges);
    Ok(clean_skeleton_text(&abs, &out))
}

/// Attempt to skeletonize a file, returning None when the file type isn't supported.
///
/// This is intended for slicer fallbacks: unsupported file types should not default to full content.
pub fn try_render_skeleton_from_source(path: &Path, source_text: &str) -> Result<Option<String>> {
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
        .set_language(language)
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
        run_query_strings(source, root, language, r#"(use_declaration argument: (_) @path)"#, "path")
    }

    fn find_exports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut exports: Vec<String> = Vec::new();
        exports.extend(run_query_strings(
            source,
            root,
            language,
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
            language,
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
            language,
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
            language,
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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/rust_prune.scm"), "body")?;
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
            language,
            r#"(function_item name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(struct_item name: (type_identifier) @name) @def"#,
            "struct",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(enum_item name: (type_identifier) @name) @def"#,
            "enum",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
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
        let import_srcs = run_query_strings(source, root, language, r#"(import_statement source: (string) @src)"#, "src")?;
        Ok(import_srcs.into_iter().map(|s| strip_string_quotes(&s)).collect())
    }

    fn find_exports(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<String>> {
        let mut exports: Vec<String> = Vec::new();

        exports.extend(run_query_strings(
            source,
            root,
            language,
            r#"(export_statement declaration: (function_declaration name: (identifier) @name))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            language,
            r#"(export_statement declaration: (class_declaration name: (type_identifier) @name))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            language,
            r#"(export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name)))"#,
            "name",
        )?);

        exports.extend(run_query_strings(
            source,
            root,
            language,
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
            language,
            r#"(function_declaration name: (identifier) @name) @def"#,
            "function",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @def"#,
            "function",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(class_declaration name: (type_identifier) @name) @def"#,
            "class",
            false,
        )?);

        symbols.extend(run_query(
            source,
            root,
            language,
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

        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/ts_prune.scm"), "body")?;
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
            language,
            r#"(function_definition name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/py_prune.scm"), "body")?;
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
            language,
            r#"(import_spec (interpreted_string_literal) @src)"#,
            "src",
        )?);
        out.extend(run_query_strings(
            source,
            root,
            language,
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
            language,
            r#"(function_declaration name: (identifier) @name)"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            language,
            r#"(method_declaration name: (field_identifier) @name)"#,
            "name",
        )?);
        exports.extend(run_query_strings(
            source,
            root,
            language,
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
            language,
            r#"(function_declaration name: (identifier) @name) @def"#,
            "function",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_declaration name: (field_identifier) @name) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/go_prune.scm"), "body")?;
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
            language,
            r#"(class_definition name: (identifier) @name) @def"#,
            "class",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(enum_declaration name: (identifier) @name) @def"#,
            "enum",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(mixin_declaration (identifier) @name) @def"#,
            "mixin",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(extension_declaration name: (identifier) @name) @def"#,
            "extension",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(type_alias (type_identifier) @name) @def"#,
            "type",
            false,
        )?);

        // Top-level function signatures.
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(function_signature name: (identifier) @name) @def"#,
            "function",
            true,
        )?);

        // Method signatures inside classes/mixins/extensions.
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_signature (function_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_signature (getter_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_signature (setter_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_signature (constructor_signature name: (identifier) @name)) @def"#,
            "method",
            true,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/dart_prune.scm"), "body")?;
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
            language,
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
            language,
            r#"(class_declaration (identifier) @name) @def"#,
            "class",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(interface_declaration (identifier) @name) @def"#,
            "interface",
            false,
        )?);
        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(enum_declaration name: (identifier) @name) @def"#,
            "enum",
            false,
        )?);

        symbols.extend(run_query(
            source,
            root,
            language,
            r#"(method_declaration (identifier) @name) @def"#,
            "method",
            true,
        )?);

        symbols.extend(run_query(
            source,
            root,
            language,
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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/java_prune.scm"), "body")?;
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
        out.extend(run_query_strings(source, root, language, r#"(using_directive (identifier) @path)"#, "path")?);
        out.extend(run_query_strings(source, root, language, r#"(using_directive (qualified_name) @path)"#, "path")?);
        out.extend(run_query_strings(source, root, language, r#"(using_directive (alias_qualified_name) @path)"#, "path")?);
        Ok(out)
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(source, root, language, r#"(class_declaration name: (identifier) @name) @def"#, "class", false)?);
        symbols.extend(run_query(source, root, language, r#"(struct_declaration name: (identifier) @name) @def"#, "struct", false)?);
        symbols.extend(run_query(source, root, language, r#"(interface_declaration name: (identifier) @name) @def"#, "interface", false)?);
        symbols.extend(run_query(source, root, language, r#"(enum_declaration name: (identifier) @name) @def"#, "enum", false)?);

        symbols.extend(run_query(source, root, language, r#"(method_declaration name: (identifier) @name) @def"#, "method", true)?);
        symbols.extend(run_query(source, root, language, r#"(constructor_declaration name: (identifier) @name) @def"#, "constructor", true)?);

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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/csharp_prune.scm"), "body")?;
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
        tree_sitter_php::language_php()
    }

    fn extract_skeleton(&self, _path: &Path, source: &[u8], root: Node, language: Language) -> Result<Vec<Symbol>> {
        let mut symbols: Vec<Symbol> = Vec::new();

        symbols.extend(run_query(source, root, language, r#"(class_declaration name: (name) @name) @def"#, "class", false)?);
        symbols.extend(run_query(source, root, language, r#"(interface_declaration name: (name) @name) @def"#, "interface", false)?);
        symbols.extend(run_query(source, root, language, r#"(trait_declaration name: (name) @name) @def"#, "trait", false)?);

        symbols.extend(run_query(source, root, language, r#"(function_definition name: (name) @name) @def"#, "function", true)?);
        symbols.extend(run_query(source, root, language, r#"(method_declaration name: (name) @name) @def"#, "method", true)?);

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
        let bodies = run_query_byte_ranges(source, root, language, include_str!("../queries/php_prune.scm"), "body")?;
        Ok(bodies
            .into_iter()
            .map(|(s, e)| (s, e, "{ /* ... */ }".to_string()))
            .collect())
    }
}

fn run_query_byte_ranges(
    source: &[u8],
    root: Node,
    language: Language,
    query_src: &str,
    cap: &str,
) -> Result<Vec<(usize, usize)>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();
    let mut out: Vec<(usize, usize)> = Vec::new();

    for m in cursor.matches(&query, root, source) {
        for cap0 in m.captures {
            let cap_name = query.capture_names()[cap0.index as usize].as_str();
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

fn run_query_strings(source: &[u8], root: Node, language: Language, query_src: &str, cap: &str) -> Result<Vec<String>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();

    let mut out: Vec<String> = Vec::new();
    for m in cursor.matches(&query, root, source) {
        for cap0 in m.captures {
            let cap_name = query.capture_names()[cap0.index as usize].as_str();
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
    language: Language,
    query_src: &str,
    kind: &str,
    include_signature: bool,
) -> Result<Vec<Symbol>> {
    let query = Query::new(language, query_src).context("Failed to compile tree-sitter query")?;
    let mut cursor = QueryCursor::new();

    let mut out: Vec<Symbol> = Vec::new();

    for m in cursor.matches(&query, root, source) {
        let mut name_node: Option<Node> = None;
        let mut def_node: Option<Node> = None;

        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize].as_str();
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
        .set_language(language)
        .context("Failed to set tree-sitter language")?;

    let tree = parser
        .parse(source_text.as_str(), None)
        .ok_or_else(|| anyhow!("Failed to parse file"))?;

    let root = tree.root_node();

    let mut symbols = driver.extract_skeleton(&abs, source, root, language)?;
    let mut imports = driver.find_imports(&abs, source, root, language)?;
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
