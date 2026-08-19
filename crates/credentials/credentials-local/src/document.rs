//! Credentials-document mechanics: strict parse validation and the
//! comment-preserving line editor. Rust port of the TS
//! `parseCredentialsDocument` + `renderDocument` behavior
//! (`packages/credentials/credentials-local/src/index.ts`).
//!
//! The TS uses the `yaml` package's AST (`Document`), which preserves
//! comments and formatting through `setIn`/`deleteIn`. The Rust port has no
//! comment-preserving YAML AST, so the editor works on the LINE level guided
//! by a scan of top-level entries; the strict shape validation still runs
//! through `serde_yaml`. For the flat `REF: scalar` documents this package
//! owns, the line editor reproduces the TS bytes exactly (patches touch only
//! the edited entry and its own leading comments; sibling block scalars stay
//! verbatim).
//!
//! # Deviations
//!
//! - Explicit `? ` complex-key forms are not line-editable (they parse, but
//!   a patch would append a duplicate plain key); such documents are outside
//!   the strict flat shape this package validates.
//! - YAML parse error positions come from `serde_yaml` (0-based, converted
//!   to the TS 1-based `line, column` wording); the exact line/column may
//!   differ from the `yaml` package on the same malformed input.

use indexmap::IndexMap;

use dsh_credentials::credential_ref;

/// Describe one YAML parse failure without quoting the source: the parser's
/// own message embeds the offending line, which here holds a secret.
fn describe_yaml_error(error: &serde_yaml::Error) -> String {
    // serde_yaml rejects duplicate mapping keys itself; report the TS
    // `yaml` package's code for that shape so diagnostics stay stable.
    if error.to_string().contains("duplicate") {
        let location = error.location();
        let position = location
            .map(|location| {
                format!(
                    " at line {}, column {}",
                    location.line() + 1,
                    location.column() + 1
                )
            })
            .unwrap_or_default();
        return format!("DUPLICATE_KEY{position}");
    }
    let Some(location) = error.location() else {
        return "PARSE_ERROR".to_string();
    };
    format!(
        "PARSE_ERROR at line {}, column {}",
        location.line() + 1,
        location.column() + 1
    )
}

/// Parse one credentials document into its entries. The document is a strict
/// mapping of `CredentialRef` to non-empty string: a non-mapping root, a key
/// that is not a POSIX identifier, a non-string value, and an empty string
/// are all rejected rather than skipped, because this file holds nothing but
/// credentials and a silently ignored entry reads as "the key I stored has no
/// effect". Duplicate keys are rejected. An empty document is an empty store.
pub fn parse_credentials_document(
    text: &str,
    filename: &str,
) -> Result<IndexMap<String, String>, String> {
    // Whole-document shape validation through the YAML parser; its message is
    // never surfaced — only the code and position leave this function.
    let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|error| {
        format!(
            "credentials-local: invalid document at {filename}: {}",
            describe_yaml_error(&error)
        )
    })?;
    // An empty document (comments and blank lines only) parses to Null; the
    // TS `document.toJS() ?? {}` treats it as the empty store.
    let serde_yaml::Value::Mapping(mapping) = value else {
        if value.is_null() {
            return Ok(IndexMap::new());
        }
        return Err(format!(
            "credentials-local: {filename} must be a mapping of credential reference to value"
        ));
    };
    let mut entries = IndexMap::new();
    for (key, value) in mapping {
        let key = key.as_str().ok_or_else(|| {
            format!(
                "credentials-local: {filename} must be a mapping of credential reference to value"
            )
        })?;
        // credential_ref rejects anything that is not a POSIX identifier,
        // which is exactly the constraint a stored reference must satisfy to
        // be addressable through the seam. The panic is contained here.
        let checked =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| credential_ref(key)));
        if let Err(payload) = checked {
            let message = payload
                .downcast::<String>()
                .map(|message| *message)
                .unwrap_or_else(|_| "credential ref shape invalid".to_string());
            return Err(message);
        }
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
    // serde_yaml silently keeps the LAST duplicate; the line scan below is
    // the duplicate-key rejection the TS parser performs.
    if let Some(duplicate) = duplicate_top_level_key(text) {
        return Err(format!(
            "credentials-local: invalid document at {filename}: DUPLICATE_KEY at line {}, column 1",
            duplicate
        ));
    }
    Ok(entries)
}

/// Detect the line (1-based) of the second occurrence of a duplicated
/// top-level mapping key, when one exists.
fn duplicate_top_level_key(text: &str) -> Option<usize> {
    let mut seen = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let Some(key) = plain_key_of(line) else {
            continue;
        };
        if seen.contains(&key) {
            return Some(index + 1);
        }
        seen.push(key);
    }
    None
}

/// The bare mapping key of a top-level `KEY: …` line (unquoted or quoted
/// POSIX identifier), or `None` when the line is not an entry start.
fn plain_key_of(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") {
        return None;
    }
    let rest = trimmed.strip_prefix('"').unwrap_or(trimmed);
    if rest == trimmed {
        // Unquoted key: an identifier directly followed by `:`.
        let colon = trimmed.find(':')?;
        let candidate = &trimmed[..colon];
        if candidate.is_empty() {
            return None;
        }
        if !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
        {
            return None;
        }
        Some(candidate.to_string())
    } else {
        // Quoted key: read to the closing quote.
        let closing = rest.find('"')?;
        let candidate = &rest[..closing];
        if candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
        {
            Some(candidate.to_string())
        } else {
            None
        }
    }
}

/// Render the next document text with one reference set or deleted. Editing
/// the scanned line ranges rather than rebuilding the document keeps
/// comments and the formatting of every untouched entry; an absent document
/// starts a fresh one.
pub fn render_document(
    text: Option<&str>,
    reference: &dsh_credentials::CredentialRef,
    value: Option<&str>,
) -> String {
    let reference = reference.to_string();
    let Some(text) = text else {
        let value = value.expect("an absent document only renders a set");
        return format!("{reference}: {}\n", serialize_scalar(value));
    };
    let entry = locate_entry(text, &reference);
    match (entry, value) {
        (Some(entry), Some(value)) => {
            // Patch only the entry's own lines: keep everything through the
            // `KEY:` prefix, then the freshly serialized value.
            let mut out = String::with_capacity(text.len() + 16);
            out.push_str(&text[..entry.key_end]);
            out.push(' ');
            out.push_str(&serialize_scalar(value));
            out.push('\n');
            out.push_str(&text[entry.end..]);
            out
        }
        (Some(entry), None) => {
            // Delete the entry and the comment block directly above it (its
            // own annotation); a document left with only blank lines
            // renders as the empty mapping.
            let annotation_start = annotation_start_of(text, entry.start);
            let mut remaining = String::new();
            remaining.push_str(&text[..annotation_start]);
            remaining.push_str(&text[entry.end..]);
            if remaining.trim().is_empty() {
                "{}\n".to_string()
            } else {
                remaining
            }
        }
        (None, Some(value)) => {
            // Append a fresh entry to the existing document.
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("{reference}: {}\n", serialize_scalar(value)));
            out
        }
        (None, None) => text.to_string(),
    }
}

/// Byte ranges of one top-level entry: `start` at the entry's key line,
/// `key_end` just after the `KEY:` prefix, `end` after the entry's last
/// line (including its newline). Indented continuation lines (block
/// scalars, folded quoted scalars) extend the range to the next top-level
/// entry or the end of the document.
struct EntrySpan {
    key: String,
    start: usize,
    key_end: usize,
    end: usize,
}

fn locate_entry(text: &str, reference: &str) -> Option<EntrySpan> {
    let mut offset = 0;
    let mut pending: Option<EntrySpan> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let blank_or_meta =
            trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---");
        if !blank_or_meta {
            if let Some(key) = plain_key_of(line) {
                if let Some(span) = pending.take() {
                    if span.key == reference {
                        return Some(span);
                    }
                }
                let colon = line.find(':').expect("key line has colon");
                pending = Some(EntrySpan {
                    key,
                    start: offset,
                    key_end: offset + colon + 1,
                    end: offset + line.len(),
                });
                offset += line.len();
                continue;
            }
        }
        // Indented non-comment lines extend the pending entry's value span
        // (block scalars and folded quoted scalars); blank and comment lines
        // stay out of the span so annotations survive a patch.
        if let Some(span) = pending.as_mut() {
            let indented = line.starts_with(' ') || line.starts_with('\t');
            if indented && !trimmed.starts_with('#') {
                span.end = offset + line.len();
            }
        }
        offset += line.len();
    }
    match pending {
        Some(span) if span.key == reference => Some(span),
        _ => None,
    }
}

/// The start of the comment block directly above `entry_start`: consecutive
/// comment lines immediately preceding the entry (blank lines stop the
/// block, per the TS annotation semantics).
fn annotation_start_of(text: &str, entry_start: usize) -> usize {
    let prefix = &text[..entry_start];
    let mut end = prefix.len();
    let mut saw_comment = false;
    for line in prefix.split_inclusive('\n').rev() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            saw_comment = true;
            end -= line.len();
            continue;
        }
        if trimmed.trim().is_empty() {
            if saw_comment {
                break;
            }
            end -= line.len();
            continue;
        }
        break;
    }
    end
}

/// Serialize one scalar for a single-line entry. Plain style when the value
/// is unambiguous; double-quoted otherwise.
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

/// Whether a scalar can round-trip as a plain (unquoted) YAML scalar.
fn is_plain_safe(value: &str) -> bool {
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return false;
    }
    // A plain scalar may never contain a double quote; a leading quote or
    // indicator character would change the token's kind.
    if value.contains('"') {
        return false;
    }
    let first = value.chars().next().expect("non-empty");
    if "-?:,[]{}#&*!|>'\"%@`".contains(first)
        || value.starts_with("---")
        || value.starts_with("...")
    {
        return false;
    }
    // A plain scalar must not contain ": " or end with ":".
    if value.contains(": ") || value.ends_with(':') || value.contains(" #") {
        return false;
    }
    // Control characters and leading/trailing whitespace disqualify.
    if value.chars().any(|ch| ch.is_control()) || value.trim() != value {
        return false;
    }
    // Plain style must re-parse as the same STRING: a bare `1`, `true`,
    // `null` would materialize as an implicit scalar type (the TS `yaml`
    // package quotes those too).
    matches!(
        serde_yaml::from_str::<serde_yaml::Value>(value),
        Ok(serde_yaml::Value::String(parsed)) if parsed == value
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_credentials::credential_ref;

    #[test]
    fn parse_accepts_comments_quoted_and_empty_documents() {
        let entries = parse_credentials_document(
            "# notes\nDSH_CRED_TEST: plain\nDSH_CRED_OTHER: \"with space\"\n",
            "f.yaml",
        )
        .expect("parse");
        assert_eq!(entries.get("DSH_CRED_TEST"), Some(&"plain".to_string()));
        assert_eq!(
            entries.get("DSH_CRED_OTHER"),
            Some(&"with space".to_string())
        );
        let empty = parse_credentials_document("# nothing stored yet\n", "f.yaml").expect("empty");
        assert!(empty.is_empty());
    }

    #[test]
    fn parse_rejects_bad_shapes_and_duplicates() {
        for (text, needle) in [
            ("just a string\n", "must be a mapping"),
            ("- DSH_CRED_TEST\n", "must be a mapping"),
            ("not-a-ref: value\n", "must match"),
            ("DSH_CRED_TEST: 123\n", "must be a string"),
            ("DSH_CRED_TEST: \"\"\n", "is empty"),
            ("DSH_CRED_TEST: one\nDSH_CRED_TEST: two\n", "DUPLICATE_KEY"),
            ("DSH_CRED_TEST: \"unterminated\n", "invalid document"),
        ] {
            let error = parse_credentials_document(text, "f.yaml")
                .err()
                .expect("rejects");
            assert!(error.contains(needle), "{text:?} -> {error}");
        }
    }

    #[test]
    fn render_patches_one_entry_preserving_comments() {
        let text = "# deployment notes\nDSH_CRED_OTHER: keep\n\n# the one under edit\nDSH_CRED_TEST: old\n";
        let rendered = render_document(
            Some(text),
            &credential_ref("DSH_CRED_TEST"),
            Some("new value!"),
        );
        assert_eq!(
            rendered,
            "# deployment notes\nDSH_CRED_OTHER: keep\n\n# the one under edit\nDSH_CRED_TEST: new value!\n"
        );
    }

    #[test]
    fn render_deletes_an_entry_with_its_annotation_and_empties_to_braces() {
        let text = "# about the doomed one\nDSH_CRED_TEST: gone\n# about the survivor\nDSH_CRED_OTHER: stays\n";
        let rendered = render_document(Some(text), &credential_ref("DSH_CRED_TEST"), None);
        assert_eq!(rendered, "# about the survivor\nDSH_CRED_OTHER: stays\n");
        let emptied = render_document(
            Some("DSH_CRED_TEST: only\n"),
            &credential_ref("DSH_CRED_TEST"),
            None,
        );
        assert_eq!(emptied, "{}\n");
    }

    #[test]
    fn render_leaves_sibling_block_scalars_untouched_and_quotes_structural_values() {
        let wrapped = "DSH_REVIEW_WRAPPED: |-\n  line1\n  line2\nDSH_REVIEW_ALPHA: a\n";
        let rendered = render_document(
            Some(wrapped),
            &credential_ref("DSH_REVIEW_ALPHA"),
            Some("b"),
        );
        assert_eq!(
            rendered,
            "DSH_REVIEW_WRAPPED: |-\n  line1\n  line2\nDSH_REVIEW_ALPHA: b\n"
        );

        let structural = render_document(
            None,
            &credential_ref("DSH_REVIEW_ALPHA"),
            Some("DSH_REVIEW_INNER: injected"),
        );
        assert_eq!(
            structural,
            "DSH_REVIEW_ALPHA: \"DSH_REVIEW_INNER: injected\"\n"
        );
        let multi = render_document(
            None,
            &credential_ref("DSH_REVIEW_ALPHA"),
            Some("line one\nline two"),
        );
        assert_eq!(multi, "DSH_REVIEW_ALPHA: \"line one\\nline two\"\n");
        let quotes = render_document(
            None,
            &credential_ref("DSH_REVIEW_ALPHA"),
            Some("both ' and \""),
        );
        assert_eq!(quotes, "DSH_REVIEW_ALPHA: \"both ' and \\\"\"\n");
    }

    #[test]
    fn render_appends_to_an_existing_document() {
        let text = "DSH_CRED_OTHER: keep\n";
        let rendered = render_document(Some(text), &credential_ref("DSH_CRED_TEST"), Some("fresh"));
        assert_eq!(rendered, "DSH_CRED_OTHER: keep\nDSH_CRED_TEST: fresh\n");
    }

    #[test]
    fn scalar_round_trips_through_the_parser() {
        for value in [
            "sk-fresh",
            "new value!",
            "line one\nline two",
            "both ' and \"",
            "a:b: injected",
            "#hash",
            "- dash",
        ] {
            let rendered = format!("DSH_CRED_TEST: {}\n", serialize_scalar(value));
            let entries = parse_credentials_document(&rendered, "f.yaml").expect("round-trips");
            assert_eq!(
                entries.get("DSH_CRED_TEST"),
                Some(&value.to_string()),
                "{rendered}"
            );
        }
    }
}
