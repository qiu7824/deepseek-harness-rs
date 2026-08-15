//! String case, path, and property formatting helpers (port of
//! `src/string.ts`).

/// Uppercase the first character of a string.
pub fn capitalize(source: &str) -> String {
    let mut chars = source.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Lowercase the first character of a string.
pub fn uncapitalize(source: &str) -> String {
    let mut chars = source.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Convert dash or underscore delimited text to camelCase.
pub fn camel_case(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut upper_next = false;
    for ch in source.chars() {
        if ch == '-' || ch == '_' {
            upper_next = true;
        } else if upper_next && ch.is_ascii_lowercase() {
            out.push(ch.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Runtime alias for [`camel_case`] (TS `camelize`).
pub use camel_case as camelize;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenState {
    Delim,
    Upper,
    Lower,
}

/// Tokenize a string into a delimiter-separated lowercase form
/// (port of the TS `tokenize` state machine; ASCII semantics).
fn tokenize(source: &str, delimiters: &[u32], delimiter: u32) -> String {
    let mut output = String::with_capacity(source.len());
    let mut state = TokenState::Delim;
    let codes: Vec<u32> = source.chars().map(|c| c as u32).collect();
    for (i, &code) in codes.iter().enumerate() {
        if (65..=90).contains(&code) {
            let next = codes.get(i + 1).copied();
            if state == TokenState::Upper {
                if next.is_some_and(|next| (97..=122).contains(&next)) {
                    output.push(char::from_u32(delimiter).unwrap());
                }
                output.push(char::from_u32(code + 32).unwrap());
            } else {
                if state != TokenState::Delim {
                    output.push(char::from_u32(delimiter).unwrap());
                }
                output.push(char::from_u32(code + 32).unwrap());
            }
            state = TokenState::Upper;
        } else if (97..=122).contains(&code) {
            output.push(char::from_u32(code).unwrap());
            state = TokenState::Lower;
        } else if delimiters.contains(&code) {
            if state != TokenState::Delim {
                output.push(char::from_u32(delimiter).unwrap());
            }
            state = TokenState::Delim;
        } else {
            output.push(char::from_u32(code).unwrap());
        }
    }
    output
}

/// Convert text to dash-delimited parameter case.
pub fn param_case(source: &str) -> String {
    tokenize(source, &[45, 95], 45)
}

/// Convert text to underscore-delimited snake case.
pub fn snake_case(source: &str) -> String {
    tokenize(source, &[45, 95], 95)
}

/// Runtime alias for [`param_case`] (TS `hyphenate`).
pub use param_case as hyphenate;

/// Format a property key as a JavaScript member access suffix.
pub fn format_property(key: &str) -> String {
    let is_ident = !key.is_empty()
        && key
            .chars()
            .enumerate()
            .all(|(i, ch)| {
                if i == 0 {
                    ch.is_ascii_alphabetic() || ch == '_' || ch == '$'
                } else {
                    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
                }
            });
    if is_ident {
        format!(".{key}")
    } else {
        format!("[{}]", serde_json::to_string(key).unwrap_or_default())
    }
}

/// Remove one trailing slash from a path string.
pub fn trim_slash(source: &str) -> String {
    source.strip_suffix('/').map(|s| s.to_string()).unwrap_or_else(|| source.to_string())
}

/// Ensure a path starts with `/` and has no trailing slash.
pub fn sanitize(source: &str) -> String {
    let with_leading = if source.starts_with('/') {
        source.to_string()
    } else {
        format!("/{source}")
    };
    trim_slash(&with_leading)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalization() {
        assert_eq!(capitalize("foo"), "Foo");
        assert_eq!(capitalize("Foo"), "Foo");
        assert_eq!(capitalize(""), "");
        assert_eq!(uncapitalize("Foo"), "foo");
        assert_eq!(uncapitalize("foo"), "foo");
    }

    #[test]
    fn case_conversions() {
        assert_eq!(camel_case("foo-bar"), "fooBar");
        assert_eq!(camel_case("foo_bar"), "fooBar");
        assert_eq!(camel_case("foo-bar_baz"), "fooBarBaz");
        assert_eq!(camelize("a-b"), "aB");
        assert_eq!(param_case("fooBar"), "foo-bar");
        assert_eq!(param_case("fooBarBaz"), "foo-bar-baz");
        assert_eq!(param_case("foo_bar"), "foo-bar");
        assert_eq!(snake_case("fooBar"), "foo_bar");
        assert_eq!(snake_case("foo-bar"), "foo_bar");
        assert_eq!(hyphenate("FooBar"), "foo-bar");
        // TS tokenizer: "A" alone → "a"; "AB" → "ab" (no delimiter inside)
        assert_eq!(param_case("AB"), "ab");
    }

    #[test]
    fn property_and_path_formatting() {
        assert_eq!(format_property("foo"), ".foo");
        assert_eq!(format_property("foo.bar"), "[\"foo.bar\"]");
        assert_eq!(trim_slash("/a/b/"), "/a/b");
        assert_eq!(trim_slash("/a/b"), "/a/b");
        assert_eq!(sanitize("a/b/"), "/a/b");
        assert_eq!(sanitize("/a/b/"), "/a/b");
        assert_eq!(sanitize("a/b"), "/a/b");
    }
}
