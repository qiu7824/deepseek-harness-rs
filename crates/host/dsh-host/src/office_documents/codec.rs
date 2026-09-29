//! Bounded, declarative OOXML data access. No formulas or document code run.
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

#[path = "reader.rs"]
mod reader;
#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
#[path = "writer.rs"]
mod writer;

pub(super) const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PART: u64 = 8 * 1024 * 1024;
const MAX_CELLS: usize = 20_000;
type Result<T> = std::result::Result<T, String>;
type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

fn cancelled(signal: &dyn Fn() -> bool) -> Result<()> {
    if signal() {
        Err("OFFICE_ABORTED: Office operation cancelled".into())
    } else {
        Ok(())
    }
}

fn integer(args: &Value, name: &str, default: u64, maximum: u64) -> Result<u64> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    value
        .as_u64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| {
                    value.is_finite()
                        && value.fract() == 0.0
                        && *value >= 0.0
                        && *value <= maximum as f64
                })
                .map(|value| value as u64)
        })
        .filter(|value| *value > 0 && *value <= maximum)
        .ok_or_else(|| format!("{name} must be an integer from 1 to {maximum}"))
}

fn part(archive: &mut Archive<'_>, name: &str) -> Result<String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| format!("Office part is missing: {name}"))?;
    if entry.size() > MAX_PART {
        return Err(format!("Office XML part exceeds 8 MiB: {name}"));
    }
    let mut bytes = Vec::new();
    (&mut entry)
        .take(MAX_PART + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PART {
        return Err("Office XML part exceeds 8 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "Office XML must use UTF-8".into())
}

fn xml(text: &str) -> Result<Document<'_>> {
    if text.contains("<!DOCTYPE") {
        return Err("Office XML document types are unsupported".into());
    }
    let document = Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 250_000,
            entity_resolver: None,
        },
    )
    .map_err(|error| format!("Invalid Office XML: {error}"))?;
    if document
        .descendants()
        .any(|node| node.ancestors().take(258).count() > 257)
    {
        return Err("Office XML nesting exceeds 256 levels".into());
    }
    Ok(document)
}

fn named(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}
fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn visible_text(node: Node<'_, '_>, word: bool) -> String {
    let mut output = String::new();
    for child in node.descendants().filter(|child| child.is_element()) {
        if named(child, "t") && !child.ancestors().any(|parent| named(parent, "rPh")) {
            output.push_str(child.text().unwrap_or(""));
        } else if word && named(child, "tab") {
            output.push('\t');
        } else if word && (named(child, "br") || named(child, "cr")) {
            output.push('\n');
        }
    }
    output
}

// ST_Xstring stores escaped UTF-16 code units. Literal escape-shaped text
// escapes its leading underscore; decoding is a single pass, never recursive.
fn escape_unit(value: &str, at: usize) -> Option<u16> {
    let bytes = value.as_bytes();
    let item = bytes.get(at..at + 7)?;
    if item[0] != b'_'
        || !matches!(item[1], b'x' | b'X')
        || item[6] != b'_'
        || !item[2..6].iter().all(u8::is_ascii_hexdigit)
    {
        return None;
    }
    u16::from_str_radix(&value[at + 2..at + 6], 16).ok()
}
fn literal_xstring(value: &str) -> String {
    let mut encoded = String::new();
    for (at, ch) in value.char_indices() {
        if ch == '_' && escape_unit(value, at).is_some() {
            encoded.push_str("_x005F_");
        } else {
            encoded.push(ch);
        }
    }
    encoded
}
fn decode_xstring(value: &str) -> Result<String> {
    let mut units = Vec::new();
    let mut at = 0;
    while at < value.len() {
        if let Some(unit) = escape_unit(value, at) {
            units.push(unit);
            at += 7;
        } else {
            let ch = value[at..].chars().next().unwrap();
            let mut encoded = [0; 2];
            units.extend_from_slice(ch.encode_utf16(&mut encoded));
            at += ch.len_utf8();
        }
    }
    String::from_utf16(&units).map_err(|_| "Invalid UTF-16 escape in spreadsheet text".into())
}
fn sheet_text(node: Node<'_, '_>) -> Result<String> {
    node.descendants()
        .filter(|node| named(*node, "t") && !node.ancestors().any(|parent| named(parent, "rPh")))
        .map(|node| decode_xstring(node.text().unwrap_or("")))
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join(""))
}

fn address(column: usize, row: usize) -> String {
    let mut column = column;
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    format!("{}{row}", letters.into_iter().rev().collect::<String>())
}
fn column_of(value: &str) -> Option<usize> {
    let mut column = 0usize;
    for byte in value.bytes().take_while(u8::is_ascii_alphabetic) {
        column = column
            .checked_mul(26)?
            .checked_add((byte.to_ascii_uppercase() - b'A' + 1) as usize)?;
    }
    (column > 0 && column <= 16_384).then_some(column)
}

pub(super) fn read(bytes: &[u8], options: &Value, signal: &dyn Fn() -> bool) -> Result<Value> {
    cancelled(signal)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Office input exceeds 32 MiB".into());
    }
    if !bytes.starts_with(b"PK") {
        return Err("OFFICE_FORMAT_UNSUPPORTED: office_read supports DOCX and XLSX, not legacy DOC/XLS, PDF or renamed HTML".into());
    }
    // Validate ZIP metadata/expansion limits before allocating its directory.
    if crate::office_input_validation::validate_office_for_read(bytes, "docx").is_err() {
        crate::office_input_validation::validate_office_for_read(bytes, "xlsx")?;
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Invalid Office ZIP: {error}"))?;
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    if !names.iter().any(|name| name == "[Content_Types].xml") {
        return Err("Office package is missing [Content_Types].xml".into());
    }
    if names
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != names.len()
    {
        return Err("Office package contains duplicate part names".into());
    }
    if names.iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name.contains("vbaproject")
            || name
                .split('/')
                .any(|part| matches!(part, "activex" | "embeddings"))
    }) {
        return Err(
            "OFFICE_ACTIVE_CONTENT: macros and embedded executable objects are unsupported".into(),
        );
    }
    let word = names.iter().any(|name| name == "word/document.xml");
    let excel = names.iter().any(|name| name == "xl/workbook.xml");
    let format = match (word, excel) {
        (true, false) => "docx",
        (false, true) => "xlsx",
        _ => return Err("OFFICE_FORMAT_UNSUPPORTED: expected one DOCX or XLSX package".into()),
    };
    cancelled(signal)?;
    let start = integer(options, "start_row", 1, 1_048_576)? as usize;
    let limit = integer(options, "row_limit", 200, 1000)? as usize;
    if word && options.get("sheet_name").is_some() || excel && options.get("table_index").is_some()
    {
        return Err("sheet_name selects XLSX worksheets; table_index selects DOCX tables".into());
    }
    let mut result = if word {
        reader::docx(&mut archive, options, start, limit, signal)?
    } else {
        reader::xlsx(&mut archive, options, start, limit, signal)?
    };
    result["format"] = json!(format);
    result["formulasEvaluated"] = json!(false);
    if serde_json::to_vec(&result)
        .map_err(|error| error.to_string())?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(
            "Office read result exceeds 4 MiB; select a smaller sheet, table or row range".into(),
        );
    }
    cancelled(signal)?;
    Ok(result)
}

pub(super) fn write(spec: &Value, signal: &dyn Fn() -> bool) -> Result<(Vec<u8>, Value)> {
    writer::write(spec, signal)
}
