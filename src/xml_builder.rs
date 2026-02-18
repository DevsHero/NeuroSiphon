use anyhow::Result;
use quick_xml::events::{BytesCData, BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::Writer;
use std::io::Cursor;

fn crunch_text_for_cdata(input: &str) -> String {
    // 1) Trim trailing whitespace on each line.
    // 2) Collapse repeated newlines (\n\n\n -> \n).

    // First pass: trim line-end whitespace.
    let mut trimmed = String::with_capacity(input.len());
    for part in input.split_inclusive('\n') {
        if let Some(line) = part.strip_suffix('\n') {
            trimmed.push_str(line.trim_end_matches([' ', '\t', '\r']));
            trimmed.push('\n');
        } else {
            trimmed.push_str(part.trim_end_matches([' ', '\t', '\r']));
        }
    }

    // Second pass: collapse consecutive newlines to a single newline.
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_nl = false;
    for ch in trimmed.chars() {
        if ch == '\n' {
            if prev_nl {
                continue;
            }
            prev_nl = true;
            out.push('\n');
        } else {
            prev_nl = false;
            out.push(ch);
        }
    }

    out.trim_end().to_string()
}

pub fn build_context_xml(repository_map: Option<&str>, files: &[(String, String)]) -> Result<String> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));

    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))?;

    let root = BytesStart::new("context_slicer");
    writer.write_event(Event::Start(root))?;

    if let Some(map_text) = repository_map {
        let map_el = BytesStart::new("repository_map");
        writer.write_event(Event::Start(map_el))?;
        let map_text = crunch_text_for_cdata(map_text);
        writer.write_event(Event::CData(BytesCData::new(map_text.as_str())))?;
        writer.write_event(Event::End(BytesEnd::new("repository_map")))?;
    }

    for (path, content) in files {
        let mut file_el = BytesStart::new("file");
        file_el.push_attribute(("path", path.as_str()));
        writer.write_event(Event::Start(file_el))?;

        // Write CDATA content.
        let content = crunch_text_for_cdata(content.as_str());
        writer.write_event(Event::CData(BytesCData::new(content.as_str())))?;
        writer.write_event(Event::End(BytesEnd::new("file")))?;
    }

    writer.write_event(Event::End(BytesEnd::new("context_slicer")))?;

    let bytes = writer.into_inner().into_inner();
    Ok(String::from_utf8(bytes)?)
}
