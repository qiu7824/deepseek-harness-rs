//! Title text normalization and UTF-8-safe truncation. Rust port of
//! `packages/session/session-title/src/normalize.ts`.

use std::sync::OnceLock;

use regex::Regex;

fn osc_sequence() -> &'static Regex {
    static OSC: OnceLock<Regex> = OnceLock::new();
    OSC.get_or_init(|| {
        // The JS look-around `(?:(?!\x07|\x1b\\)[\s\S])*` becomes the
        // equivalent negated-class alternation (the `regex` crate has no
        // look-around): any char except BEL/ESC, or ESC not followed by `\`.
        Regex::new(r"(?:\x1b\]|\x9d)(?:[^\x07\x1b]|\x1b[^\\])*(?:\x07|\x1b\\|$)").unwrap()
    })
}

fn csi_sequence() -> &'static Regex {
    static CSI: OnceLock<Regex> = OnceLock::new();
    CSI.get_or_init(|| Regex::new(r"(?:\x1b\[|\x9b)[0-?]*[ -/]*[@-~]").unwrap())
}

fn esc_sequence() -> &'static Regex {
    static ESC: OnceLock<Regex> = OnceLock::new();
    ESC.get_or_init(|| Regex::new(r"\x1b[@-_]").unwrap())
}

fn control_character() -> &'static Regex {
    static CONTROL: OnceLock<Regex> = OnceLock::new();
    CONTROL.get_or_init(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]").unwrap())
}

fn directional_control() -> &'static Regex {
    static DIRECTIONAL: OnceLock<Regex> = OnceLock::new();
    DIRECTIONAL.get_or_init(|| {
        Regex::new(r"[\u{200b}\u{200e}\u{200f}\u{202a}-\u{202e}\u{2060}-\u{2064}\u{2066}-\u{206f}\u{feff}]")
            .unwrap()
    })
}

fn whitespace_run() -> &'static Regex {
    static WHITESPACE: OnceLock<Regex> = OnceLock::new();
    WHITESPACE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

/// Reject an invalid public text limit (the TS runtime guard for
/// non-positive integers; non-integers are inexpressible in the `u64`
/// signature).
fn assert_positive_integer(name: &str, value: u64) {
    if value == 0 {
        panic!("{name} must be a positive integer");
    }
}

/// Remove controls and produce one trimmed, whitespace-normalized line.
fn clean_title_text(input: &str) -> String {
    let mut cleaned = osc_sequence().replace_all(input, "").into_owned();
    cleaned = csi_sequence().replace_all(&cleaned, "").into_owned();
    cleaned = esc_sequence().replace_all(&cleaned, "").into_owned();
    cleaned = control_character().replace_all(&cleaned, "").into_owned();
    cleaned = directional_control().replace_all(&cleaned, "").into_owned();
    let collapsed = whitespace_run().replace_all(&cleaned, " ");
    collapsed.trim().to_string()
}

/// Truncate a string to a UTF-8 byte budget without splitting a Unicode
/// code point (TS `truncateTitleUtf8`).
pub fn truncate_title_utf8(input: &str, max_bytes: u64) -> String {
    assert_positive_integer("maxBytes", max_bytes);
    if input.len() as u64 <= max_bytes && input.as_bytes().len() as u64 <= max_bytes {
        return input.to_string();
    }
    let mut used: u64 = 0;
    let mut output = String::new();
    for character in input.chars() {
        let bytes = character.len_utf8() as u64;
        if used + bytes > max_bytes {
            break;
        }
        output.push(character);
        used += bytes;
    }
    output
}

/// Normalize one accepted session title and enforce its UTF-8 byte budget
/// (TS `normalizeSessionTitle`).
pub fn normalize_session_title(input: &str, max_bytes: u64) -> String {
    truncate_title_utf8(&clean_title_text(input), max_bytes)
        .trim_end()
        .to_string()
}

/// Derive the deterministic first-prompt fallback (TS
/// `fallbackSessionTitle`).
pub fn fallback_session_title(input: &str, max_words: u64, max_bytes: u64) -> String {
    assert_positive_integer("maxWords", max_words);
    let cleaned = clean_title_text(input);
    let words: Vec<&str> = cleaned
        .split(' ')
        .filter(|word| !word.is_empty())
        .take(max_words as usize)
        .collect();
    truncate_title_utf8(&words.join(" "), max_bytes)
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_terminal_controls_and_collapses_whitespace() {
        let input = "\u{1b}]0;stolen\u{7}  Hello\t brave\nnew world  ";
        assert_eq!(normalize_session_title(input, 80), "Hello brave new world");
        assert_eq!(fallback_session_title("one two three four", 3, 80), "one two three");
        assert_eq!(fallback_session_title("你好世界", 5, 7), "你好");
        assert_eq!(fallback_session_title("😀😀", 5, 5).len(), 4);
    }

    #[test]
    fn strips_ansi_color_and_directional_controls() {
        assert_eq!(normalize_session_title("\u{1b}[31m red \u{1b}[0m", 80), "red");
        assert_eq!(normalize_session_title("\u{202e}tfel\u{202c}", 80), "tfel");
        assert_eq!(normalize_session_title("\u{200b}zero-width\u{feff}", 80), "zero-width");
    }

    #[test]
    fn rejects_non_positive_public_limits() {
        let outcome = std::panic::catch_unwind(|| truncate_title_utf8("title", 0));
        assert!(outcome.is_err());
        let message = outcome
            .err()
            .and_then(|payload| payload.downcast::<String>().ok())
            .expect("panic payload");
        assert!(message.contains("maxBytes must be a positive integer"), "{message}");
        let outcome = std::panic::catch_unwind(|| fallback_session_title("title", 0, 10));
        let message = outcome
            .err()
            .and_then(|payload| payload.downcast::<String>().ok())
            .expect("panic payload");
        assert!(message.contains("maxWords must be a positive integer"), "{message}");
    }

    #[test]
    fn truncation_never_splits_code_points() {
        assert_eq!(truncate_title_utf8("hello", 80), "hello");
        assert_eq!(truncate_title_utf8("你好世界", 7), "你好");
        assert_eq!(truncate_title_utf8("😀😀", 5), "😀");
        assert_eq!(truncate_title_utf8("😀", 3), "");
    }
}
