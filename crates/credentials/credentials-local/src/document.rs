//! Credentials-document mechanics for the version-1 credentials layout.
//!
//! Current Harness documents store environment-style references and opaque
//! credential records in separate namespaces:
//!
//! ```yaml
//! version: 1
//! refs:
//!   DEEPSEEK_API_KEY: value
//! records: {}
//! ```
//!
//! Pre-release Rust builds wrote a flat `CredentialRef: string` mapping.  The
//! recognizer below upgrades only that exact old shape; ambiguous documents
//! remain hard failures.  Reference edits are line based so an untouched
//! `records` section (including its comments and scalar spelling) is preserved
//! byte-for-byte.

use std::sync::OnceLock;

use indexmap::IndexMap;

/// The only structured credentials-document version this build reads/writes.
pub const DOCUMENT_VERSION: u64 = 1;

/// Describe one YAML parse failure without quoting source text. Parser
/// messages can contain the offending line, which may hold a secret.
fn describe_yaml_error(error: &serde_yaml::Error) -> String {
    let code = if error.to_string().contains("duplicate") {
        "DUPLICATE_KEY"
    } else {
        "PARSE_ERROR"
    };
    let Some(location) = error.location() else {
        return code.to_string();
    };
    format!(
        "{code} at line {}, column {}",
        location.line() + 1,
        location.column() + 1
    )
}

fn ref_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("credential ref pattern")
    })
}

fn key_segment_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"^[a-z][a-z0-9-]*$").expect("credential key segment pattern")
    })
}

fn assert_ref_name(key: &str) -> Result<(), String> {
    if ref_pattern().is_match(key) {
        Ok(())
    } else {
        Err(format!(
            "credential ref \"{key}\" must match {}",
            ref_pattern().as_str()
        ))
    }
}

fn assert_record_key(key: &str) -> Result<(), String> {
    let segments: Vec<_> = key.split('/').collect();
    if segments.len() != 2 {
        return Err(format!("credential key \"{key}\" must be \"<scope>/<id>\""));
    }
    for segment in segments {
        if !key_segment_pattern().is_match(segment) {
            return Err(format!(
                "credential key segment \"{segment}\" must match {}",
                key_segment_pattern().as_str()
            ));
        }
    }
    Ok(())
}

fn mapping_field<'a>(
    mapping: &'a serde_yaml::Mapping,
    name: &str,
) -> Option<&'a serde_yaml::Value> {
    mapping.get(serde_yaml::Value::String(name.to_string()))
}

fn parse_flat_mapping(
    mapping: &serde_yaml::Mapping,
    filename: &str,
) -> Result<IndexMap<String, String>, String> {
    let mut entries = IndexMap::new();
    for (key, value) in mapping {
        let key = key.as_str().ok_or_else(|| {
            format!(
                "credentials-local: {filename} must be a mapping of credential reference to value"
            )
        })?;
        assert_ref_name(key)?;
        let Some(stored) = value.as_str() else {
            return Err(format!(
                "credentials-local: the value for \"{key}\" in {filename} must be a string"
            ));
        };
        if stored.is_empty() {
            return Err(format!(
                "credentials-local: the value for \"{key}\" in {filename} is empty; remove the key instead"
            ));
        }
        entries.insert(key.to_string(), stored.to_string());
    }
    Ok(entries)
}

fn section_mapping<'a>(
    value: Option<&'a serde_yaml::Value>,
    name: &str,
    filename: &str,
) -> Result<Option<&'a serde_yaml::Mapping>, String> {
    match value {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::Mapping(mapping)) => Ok(Some(mapping)),
        Some(_) => Err(format!(
            "credentials-local: \"{name}\" in {filename} must be a mapping"
        )),
    }
}

fn parse_refs(
    section: Option<&serde_yaml::Value>,
    filename: &str,
) -> Result<IndexMap<String, String>, String> {
    let mut entries = IndexMap::new();
    let Some(mapping) = section_mapping(section, "refs", filename)? else {
        return Ok(entries);
    };
    for (key, value) in mapping {
        let key = key.as_str().ok_or_else(|| {
            format!("credentials-local: \"refs\" in {filename} must use string keys")
        })?;
        assert_ref_name(key)?;
        let Some(stored) = value.as_str() else {
            return Err(format!(
                "credentials-local: the value for \"{key}\" in {filename} must be a string"
            ));
        };
        if stored.is_empty() {
            return Err(format!(
                "credentials-local: the value for \"{key}\" in {filename} is empty; remove the key instead"
            ));
        }
        entries.insert(key.to_string(), stored.to_string());
    }
    Ok(entries)
}

fn record_fields<'a>(
    key: &str,
    value: &'a serde_yaml::Value,
    filename: &str,
) -> Result<&'a serde_yaml::Mapping, String> {
    value.as_mapping().ok_or_else(|| {
        format!("credentials-local: record \"{key}\" in {filename} must be a mapping")
    })
}

fn string_field_name<'a>(
    key: &str,
    field: &'a serde_yaml::Value,
    filename: &str,
) -> Result<&'a str, String> {
    field.as_str().ok_or_else(|| {
        format!("credentials-local: record \"{key}\" in {filename} has a non-string field name")
    })
}

fn validate_json_value(value: &serde_yaml::Value, key: &str, filename: &str) -> Result<(), String> {
    match value {
        serde_yaml::Value::Null | serde_yaml::Value::Bool(_) | serde_yaml::Value::String(_) => {
            Ok(())
        }
        serde_yaml::Value::Number(number) => {
            if number.as_f64().is_some_and(f64::is_finite) {
                Ok(())
            } else {
                Err(format!(
                    "credentials-local: record \"{key}\" payload in {filename} holds a non-finite number"
                ))
            }
        }
        serde_yaml::Value::Sequence(sequence) => {
            for nested in sequence {
                validate_json_value(nested, key, filename)?;
            }
            Ok(())
        }
        serde_yaml::Value::Mapping(mapping) => {
            for (field, nested) in mapping {
                if field.as_str().is_none() {
                    return Err(format!(
                        "credentials-local: record \"{key}\" payload in {filename} has a non-string object key"
                    ));
                }
                validate_json_value(nested, key, filename)?;
            }
            Ok(())
        }
        serde_yaml::Value::Tagged(_) => Err(format!(
            "credentials-local: record \"{key}\" payload in {filename} holds a value JSON cannot represent"
        )),
    }
}

fn validate_api_key_record(
    key: &str,
    fields: &serde_yaml::Mapping,
    filename: &str,
) -> Result<(), String> {
    for field in fields.keys() {
        let field = string_field_name(key, field, filename)?;
        if !matches!(field, "kind" | "key" | "env") {
            return Err(format!(
                "credentials-local: record \"{key}\" in {filename} has unknown field \"{field}\""
            ));
        }
    }
    if let Some(value) = mapping_field(fields, "key")
        && value.as_str().is_none_or(str::is_empty)
    {
        return Err(format!(
            "credentials-local: record \"{key}\" in {filename} has a non-string or empty key"
        ));
    }
    if let Some(env) = mapping_field(fields, "env") {
        let env = env.as_mapping().ok_or_else(|| {
            format!("credentials-local: record \"{key}\" in {filename} has a non-mapping env")
        })?;
        for (name, value) in env {
            let name = name.as_str().ok_or_else(|| {
                format!(
                    "credentials-local: record \"{key}\" env in {filename} has a non-string name"
                )
            })?;
            assert_ref_name(name)?;
            if value.as_str().is_none_or(str::is_empty) {
                return Err(format!(
                    "credentials-local: record \"{key}\" env \"{name}\" in {filename} must be a non-empty string"
                ));
            }
        }
    }
    Ok(())
}

fn validate_grant_record(
    key: &str,
    fields: &serde_yaml::Mapping,
    filename: &str,
) -> Result<(), String> {
    for field in fields.keys() {
        let field = string_field_name(key, field, filename)?;
        if !matches!(field, "kind" | "payload") {
            return Err(format!(
                "credentials-local: record \"{key}\" in {filename} has unknown field \"{field}\""
            ));
        }
    }
    let payload = mapping_field(fields, "payload").ok_or_else(|| {
        format!("credentials-local: record \"{key}\" in {filename} has no payload")
    })?;
    validate_json_value(payload, key, filename)
}

fn validate_records(section: Option<&serde_yaml::Value>, filename: &str) -> Result<(), String> {
    let Some(records) = section_mapping(section, "records", filename)? else {
        return Ok(());
    };
    for (key, value) in records {
        let key = key.as_str().ok_or_else(|| {
            format!("credentials-local: \"records\" in {filename} must use string keys")
        })?;
        assert_record_key(key)?;
        let fields = record_fields(key, value, filename)?;
        let kind = mapping_field(fields, "kind")
            .and_then(serde_yaml::Value::as_str)
            .ok_or_else(|| {
                format!("credentials-local: record \"{key}\" in {filename} has no kind")
            })?;
        match kind {
            "api-key" => validate_api_key_record(key, fields, filename)?,
            "grant" => validate_grant_record(key, fields, filename)?,
            _ => {
                return Err(format!(
                    "credentials-local: record \"{key}\" in {filename} has unknown kind"
                ));
            }
        }
    }
    Ok(())
}

/// Parse a credentials document and return its reference namespace. Version-1
/// record entries are strictly validated even though the pre-alpha Rust seam
/// does not yet expose them; accepting a document must never mean silently
/// discarding an invalid or misspelled credential record. Recognized legacy
/// flat documents remain readable so callers can migrate them atomically.
pub fn parse_credentials_document(
    text: &str,
    filename: &str,
) -> Result<IndexMap<String, String>, String> {
    let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|error| {
        format!(
            "credentials-local: invalid document at {filename}: {}",
            describe_yaml_error(&error)
        )
    })?;
    let serde_yaml::Value::Mapping(mapping) = value else {
        if value.is_null() {
            return Ok(IndexMap::new());
        }
        return Err(format!("credentials-local: {filename} must be a mapping"));
    };
    if mapping.is_empty() {
        return Ok(IndexMap::new());
    }

    let version = mapping_field(&mapping, "version");
    if version.is_none() {
        return parse_flat_mapping(&mapping, filename);
    }
    if version.and_then(serde_yaml::Value::as_u64) != Some(DOCUMENT_VERSION) {
        return Err(format!(
            "credentials-local: {filename} declares an unsupported version; this build reads version {DOCUMENT_VERSION}"
        ));
    }
    for key in mapping.keys() {
        let key = key.as_str().ok_or_else(|| {
            format!("credentials-local: {filename} must use string top-level keys")
        })?;
        if !matches!(key, "version" | "refs" | "records") {
            return Err(format!(
                "credentials-local: unknown top-level key \"{key}\" in {filename}"
            ));
        }
    }
    let refs = parse_refs(mapping_field(&mapping, "refs"), filename)?;
    validate_records(mapping_field(&mapping, "records"), filename)?;
    Ok(refs)
}

/// Render the exact one-shot migration for a recognized pre-release flat
/// document. Values and comments are only indented, never parsed and
/// re-serialized. Ambiguous or malformed input is declined.
pub fn render_flat_layout_migration(text: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let mapping = value.as_mapping()?;
    if mapping.is_empty() || mapping_field(mapping, "version").is_some() {
        return None;
    }
    for line in text.lines() {
        if line.starts_with('%') || line == "---" || line == "..." {
            return None;
        }
    }
    for (key, value) in mapping {
        let key = key.as_str()?;
        if !ref_pattern().is_match(key) {
            return None;
        }
        if value.as_str().is_none_or(str::is_empty) {
            return None;
        }
    }
    let body = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "version: {DOCUMENT_VERSION}\nrefs:\n{body}{}",
        if text.ends_with('\n') { "" } else { "\n" }
    )
    .into()
}

#[derive(Debug, Clone)]
struct LineSpan {
    start: usize,
    end: usize,
    key_end: Option<usize>,
    indent: usize,
    key: Option<String>,
    blank: bool,
    comment: bool,
}

fn lines_of(text: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for piece in text.split_inclusive('\n') {
        let content = piece
            .strip_suffix('\n')
            .unwrap_or(piece)
            .strip_suffix('\r')
            .unwrap_or_else(|| piece.strip_suffix('\n').unwrap_or(piece));
        let indent = content
            .as_bytes()
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let trimmed = content[indent..].trim_end();
        let blank = trimmed.is_empty();
        let comment = trimmed.starts_with('#');
        let key = if blank || comment || trimmed.starts_with("---") || trimmed.starts_with("...") {
            None
        } else {
            plain_key_of(trimmed)
        };
        let key_end = key
            .as_ref()
            .and_then(|_| content.find(':').map(|colon| offset + colon + 1));
        spans.push(LineSpan {
            start: offset,
            end: offset + piece.len(),
            key_end,
            indent,
            key,
            blank,
            comment,
        });
        offset += piece.len();
    }
    if text.is_empty() {
        return spans;
    }
    if !text.ends_with('\n') && spans.is_empty() {
        spans.push(LineSpan {
            start: 0,
            end: text.len(),
            key_end: None,
            indent: 0,
            key: None,
            blank: false,
            comment: false,
        });
    }
    spans
}

fn plain_key_of(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix('"') {
        let closing = rest.find('"')?;
        if !rest[closing + 1..].trim_start().starts_with(':') {
            return None;
        }
        return Some(rest[..closing].to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        let closing = rest.find('\'')?;
        if !rest[closing + 1..].trim_start().starts_with(':') {
            return None;
        }
        return Some(rest[..closing].to_string());
    }
    let colon = trimmed.find(':')?;
    let candidate = trimmed[..colon].trim_end();
    if candidate.is_empty() || candidate.starts_with(['?', '-']) {
        return None;
    }
    Some(candidate.to_string())
}

#[derive(Debug, Clone, Copy)]
struct SectionSpan {
    start: usize,
    header_end: usize,
    end: usize,
    key_end: usize,
}

fn top_level_section(text: &str, name: &str) -> Option<SectionSpan> {
    let lines = lines_of(text);
    let (index, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key.as_deref() == Some(name))?;
    let end = lines[index + 1..]
        .iter()
        .find(|line| line.indent == 0 && line.key.is_some())
        .map_or(text.len(), |line| line.start);
    Some(SectionSpan {
        start: line.start,
        header_end: line.end,
        end,
        key_end: line.key_end.expect("mapping key line has a colon"),
    })
}

fn nested_indent(text: &str, section: SectionSpan) -> Option<usize> {
    lines_of(&text[section.header_end..section.end])
        .into_iter()
        .filter(|line| line.key.is_some() && line.indent > 0)
        .map(|line| line.indent)
        .min()
}

#[derive(Debug, Clone, Copy)]
struct EntrySpan {
    start: usize,
    key_end: usize,
    end: usize,
    indent: usize,
}

fn nested_entry(text: &str, section: SectionSpan, name: &str) -> Option<EntrySpan> {
    let body = &text[section.header_end..section.end];
    let indent = nested_indent(text, section)?;
    let lines = lines_of(body);
    let (index, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == indent && line.key.as_deref() == Some(name))?;
    let mut end = line.end;
    for next in &lines[index + 1..] {
        if next.blank || next.comment || next.indent <= indent {
            break;
        }
        end = next.end;
    }
    Some(EntrySpan {
        start: section.header_end + line.start,
        key_end: section.header_end + line.key_end.expect("entry key line has a colon"),
        end: section.header_end + end,
        indent,
    })
}

fn annotation_start(text: &str, floor: usize, entry: EntrySpan) -> usize {
    let prefix = &text[floor..entry.start];
    let lines = lines_of(prefix);
    let mut start = entry.start;
    for line in lines.iter().rev() {
        if line.blank {
            break;
        }
        if line.comment && line.indent >= entry.indent {
            start = floor + line.start;
            continue;
        }
        break;
    }
    start
}

fn new_document(reference: &str, value: &str) -> String {
    format!(
        "version: {DOCUMENT_VERSION}\nrefs:\n  {reference}: {}\n",
        serialize_scalar(value)
    )
}

/// Render one reference set/delete in the version-1 document without touching
/// the records namespace. The caller has already parsed the cached text, so an
/// invalid document is never repaired or overwritten here.
pub fn render_document(
    text: Option<&str>,
    reference: &dsh_credentials::CredentialRef,
    value: Option<&str>,
) -> String {
    let reference = reference.to_string();
    let Some(original) = text else {
        return new_document(
            &reference,
            value.expect("an absent document only renders a set"),
        );
    };
    let migrated = render_flat_layout_migration(original);
    let text = migrated.as_deref().unwrap_or(original);

    let refs = parse_credentials_document(text, "<cached credentials document>")
        .expect("render_document receives only previously admitted text");
    let section = top_level_section(text, "refs");
    let entry = section.and_then(|section| nested_entry(text, section, &reference));

    match (section, entry, value) {
        (_, Some(entry), Some(value)) => {
            let mut out = String::with_capacity(text.len() + value.len() + 8);
            out.push_str(&text[..entry.key_end]);
            out.push(' ');
            out.push_str(&serialize_scalar(value));
            out.push('\n');
            out.push_str(&text[entry.end..]);
            out
        }
        (Some(section), Some(entry), None) if refs.len() == 1 => {
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..section.start]);
            out.push_str("refs: {}\n");
            out.push_str(&text[entry.end..]);
            out
        }
        (Some(section), Some(entry), None) => {
            let start = annotation_start(text, section.header_end, entry);
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..start]);
            out.push_str(&text[entry.end..]);
            out
        }
        (_, None, None) => text.to_string(),
        (Some(section), None, Some(value)) => {
            let header = &text[section.key_end..section.header_end];
            let block_style = header.trim().is_empty();
            if !block_style {
                let mut out = String::with_capacity(text.len() + value.len() + 16);
                out.push_str(&text[..section.key_end]);
                out.push('\n');
                out.push_str(&format!("  {reference}: {}\n", serialize_scalar(value)));
                out.push_str(&text[section.header_end..]);
                return out;
            }
            let indent = nested_indent(text, section).unwrap_or(2);
            let mut insertion = section.end;
            let body_lines = lines_of(&text[section.header_end..section.end]);
            for line in body_lines.iter().rev() {
                if line.blank || (line.comment && line.indent == 0) {
                    insertion = section.header_end + line.start;
                    continue;
                }
                break;
            }
            let mut out = String::with_capacity(text.len() + value.len() + indent + 8);
            out.push_str(&text[..insertion]);
            if insertion > 0 && !text[..insertion].ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push_str(&reference);
            out.push_str(": ");
            out.push_str(&serialize_scalar(value));
            out.push('\n');
            out.push_str(&text[insertion..]);
            out
        }
        (None, None, Some(value)) => {
            if refs.is_empty() && top_level_section(text, "version").is_none() {
                return new_document(&reference, value);
            }
            let version = top_level_section(text, "version")
                .expect("a non-empty admitted structured document has a version");
            let insertion = version.header_end;
            let mut out = String::with_capacity(text.len() + value.len() + 20);
            out.push_str(&text[..insertion]);
            out.push_str("refs:\n  ");
            out.push_str(&reference);
            out.push_str(": ");
            out.push_str(&serialize_scalar(value));
            out.push('\n');
            out.push_str(&text[insertion..]);
            out
        }
        (None, Some(_), _) => unreachable!("an entry cannot exist without its section"),
    }
}

/// Serialize one scalar for a single-line entry. Plain style is used only when
/// YAML re-parses it as the same string; everything ambiguous is quoted.
pub fn serialize_scalar(value: &str) -> String {
    if is_plain_safe(value) {
        value.to_string()
    } else {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('"');
        for ch in value.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                ch if (ch as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", ch as u32)),
                ch => out.push(ch),
            }
        }
        out.push('"');
        out
    }
}

fn is_plain_safe(value: &str) -> bool {
    if value.is_empty() || value.contains('\n') || value.contains('\r') || value.contains('"') {
        return false;
    }
    let first = value.chars().next().expect("non-empty");
    if "-?:,[]{}#&*!|>'\"%@`".contains(first)
        || value.starts_with("---")
        || value.starts_with("...")
    {
        return false;
    }
    if value.contains(": ") || value.ends_with(':') || value.contains(" #") {
        return false;
    }
    if value.chars().any(char::is_control) || value.trim() != value {
        return false;
    }
    matches!(
        serde_yaml::from_str::<serde_yaml::Value>(value),
        Ok(serde_yaml::Value::String(parsed)) if parsed == value
    )
}
