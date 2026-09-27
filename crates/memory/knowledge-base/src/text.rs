//! Search segmentation and chunking.
//!
//! SQLite's `unicode61` tokenizer treats a run of CJK characters as one
//! token, so Chinese text would only match whole sentences. Before indexing
//! and querying, each CJK run is rewritten as overlapping character bigrams
//! (a single character stays as it is); other text is lower-cased words.
//! Queries use the same rewrite, so a two-character word matches wherever it
//! appears inside a longer run.

/// Whether `ch` belongs to a script written without spaces.
pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x30FF   // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0x20000..=0x2FA1F)
}

/// Search terms of `text`: lower-cased alphanumeric words and CJK bigrams.
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut run: Vec<char> = Vec::new();
    let flush_word = |word: &mut String, out: &mut Vec<String>| {
        if !word.is_empty() {
            out.push(std::mem::take(word));
        }
    };
    let flush_run = |run: &mut Vec<char>, out: &mut Vec<String>| {
        match run.len() {
            0 => {}
            1 => out.push(run[0].to_string()),
            _ => {
                for pair in run.windows(2) {
                    out.push(pair.iter().collect());
                }
            }
        }
        run.clear();
    };
    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut out);
            run.push(ch);
        } else if ch.is_alphanumeric() || ch == '_' {
            flush_run(&mut run, &mut out);
            word.extend(ch.to_lowercase());
        } else {
            flush_word(&mut word, &mut out);
            flush_run(&mut run, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_run(&mut run, &mut out);
    out
}

/// The text stored in the full-text index for one chunk.
pub fn index_text(text: &str) -> String {
    terms(text).join(" ")
}

/// An FTS5 MATCH expression requiring every query term, or None when the
/// query has no searchable term. Terms are quoted so user input cannot
/// inject FTS syntax.
pub fn match_expression(query: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let quoted: Vec<String> = terms(query)
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .take(32)
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    (!quoted.is_empty()).then(|| quoted.join(" AND "))
}

/// Split text into chunks of about `target` characters, preferring
/// paragraph, then sentence boundaries, with `overlap` characters repeated
/// between neighbours so a passage cut at a boundary stays findable.
pub fn chunks(text: &str, target: usize, overlap: usize) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        // Skip leading whitespace so chunks start on content.
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }
        if start >= chars.len() {
            break;
        }
        let hard_end = (start + target).min(chars.len());
        let mut end = hard_end;
        if hard_end < chars.len() {
            let window = &chars[start..hard_end];
            let min = window.len() / 2;
            let cut = |predicate: &dyn Fn(usize) -> bool| {
                (min..window.len()).rev().find(|&index| predicate(index))
            };
            let paragraph =
                cut(&|index| window[index] == '\n' && index > 0 && window[index - 1] == '\n');
            let sentence = cut(&|index| {
                matches!(window[index], '。' | '！' | '？' | '；' | '\n')
                    || (matches!(window[index], '.' | '!' | '?' | ';')
                        && window
                            .get(index + 1)
                            .is_none_or(|next| next.is_whitespace()))
            });
            if let Some(index) = paragraph.or(sentence) {
                end = start + index + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        if end >= chars.len() {
            break;
        }
        let next = end.saturating_sub(overlap);
        start = if next > start { next } else { end };
    }
    out
}

/// A short excerpt of `text` around the first query term, at most `width`
/// characters, with ellipses where it was cut.
pub fn snippet(text: &str, query: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let lower = text.to_lowercase();
    let position = terms(query)
        .iter()
        .filter_map(|term| lower.find(term.as_str()))
        .min()
        .map(|byte| lower[..byte].chars().count())
        .unwrap_or(0);
    let start = position.saturating_sub(width / 3).min(chars.len() - width);
    let end = start + width;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_runs_become_bigrams_and_words_are_lowercased() {
        assert_eq!(terms("定时任务"), ["定时", "时任", "任务"]);
        assert_eq!(terms("用 Rust 写"), ["用", "rust", "写"]);
        assert_eq!(terms("API_Key 配置!"), ["api_key", "配置"]);
        assert!(terms("  ，。").is_empty());
    }

    #[test]
    fn match_expressions_quote_terms_and_drop_duplicates() {
        assert_eq!(match_expression("任务 任务").as_deref(), Some("\"任务\""));
        assert_eq!(
            match_expression("deploy \"x\" OR y").as_deref(),
            Some("\"deploy\" AND \"x\" AND \"or\" AND \"y\"")
        );
        assert_eq!(match_expression("…"), None);
    }

    #[test]
    fn chunks_prefer_paragraph_and_sentence_boundaries_with_overlap() {
        let text = format!("{}\n\n{}", "甲".repeat(60), "乙".repeat(60));
        let pieces = chunks(&text, 80, 10);
        assert_eq!(pieces[0], "甲".repeat(60));
        assert!(pieces[1].ends_with(&"乙".repeat(60)));
        let prose = "第一句。第二句很长很长。第三句。".repeat(10);
        for piece in chunks(&prose, 40, 5) {
            assert!(piece.chars().count() <= 40);
        }
        assert_eq!(chunks("   ", 10, 2), Vec::<String>::new());
        assert_eq!(chunks("short", 100, 10), ["short"]);
    }

    #[test]
    fn snippets_center_on_the_first_match() {
        let text = format!("{}关键内容在这里{}", "前".repeat(100), "后".repeat(100));
        let excerpt = snippet(&text, "关键内容", 30);
        assert!(excerpt.contains("关键内容"));
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        assert_eq!(snippet("短文本", "x", 30), "短文本");
    }
}
