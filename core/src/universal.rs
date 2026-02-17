use regex::Regex;
use std::sync::OnceLock;

fn contains_todo_fixme(s: &str) -> bool {
    let up = s.to_ascii_uppercase();
    up.contains("TODO") || up.contains("FIXME")
}

fn def_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            // Ruby/Swift/Kotlin-ish: class Foo, def bar, func baz, struct X, enum Y, interface Z
            Regex::new(r"^\s*(function|class|def|func|struct|interface|enum)\s+([a-zA-Z0-9_]+)").unwrap(),
            // Kotlin: public/private/protected static fn/var/val name
            Regex::new(r"^\s*(?:public|private|protected)?\s*(?:static\s*)?(?:fn|var|val)\s+([a-zA-Z0-9_]+)").unwrap(),
            // Swift with modifiers: public/private/protected static func name
            Regex::new(r"^\s*(?:public|private|protected)?\s*(?:static\s*)?func\s+([a-zA-Z0-9_]+)").unwrap(),
        ]
    })
}

fn is_definition_line(line: &str) -> bool {
    // Cheap prefilter to avoid regex cost on most lines.
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }

    // Keep TODO/FIXME comments; they can be valuable context even in fallback mode.
    if contains_todo_fixme(t) {
        return true;
    }

    // Quick keyword scan.
    if !(t.starts_with("function")
        || t.starts_with("class")
        || t.starts_with("def")
        || t.starts_with("func")
        || t.starts_with("struct")
        || t.starts_with("interface")
        || t.starts_with("enum")
        || t.starts_with("public")
        || t.starts_with("private")
        || t.starts_with("protected")
        || t.starts_with("static")
        || t.starts_with("fn")
        || t.starts_with("var")
        || t.starts_with("val"))
    {
        return false;
    }

    def_regexes().iter().any(|re| re.is_match(line))
}

/// Regex-based skeleton extraction for unsupported languages.
///
/// Output is line-based: definition-ish lines are kept, gaps are collapsed to a single `...` line.
pub fn render_universal_skeleton(source_text: &str) -> String {
    let max_kept_lines: usize = 600;

    let mut out = String::new();
    let mut last_kept_line: Option<usize> = None;
    let mut kept: usize = 0;

    for (idx, line) in source_text.lines().enumerate() {
        if kept >= max_kept_lines {
            out.push_str("...\n");
            break;
        }

        if !is_definition_line(line) {
            continue;
        }

        if let Some(prev) = last_kept_line {
            if idx > prev + 1 {
                out.push_str("...\n");
            }
        }

        // Strip both leading and trailing whitespace (flatten indentation).
        out.push_str(line.trim());
        out.push('\n');
        last_kept_line = Some(idx);
        kept += 1;
    }

    if out.trim().is_empty() {
        // If no structure was found, return a small head snippet (still better than full file).
        let head: String = source_text
            .lines()
            .take(50)
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        return format!("/* TRUNCATED */\n{}\n", head);
    }

    out
}
