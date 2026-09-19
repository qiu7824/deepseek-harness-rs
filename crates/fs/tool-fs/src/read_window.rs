//! Retain only requested lines, including for huge single-line log files.
use super::{READ_MAX_BYTES, READ_MAX_LINE_LENGTH};
use serde_json::{Value, json};
pub(crate) struct ReadWindow {
    offset: u64,
    limit: u64,
    total: u64,
    line: String,
    chars: usize,
    bytes: usize,
    full: bool,
    lines: Vec<Value>,
}
impl ReadWindow {
    pub fn new(offset: u64, limit: u64) -> Self {
        Self {
            offset,
            limit,
            total: 0,
            line: String::new(),
            chars: 0,
            bytes: 0,
            full: false,
            lines: Vec::new(),
        }
    }
    pub fn push(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                self.end_line();
            } else {
                self.chars = self.chars.saturating_add(1);
                if self.chars <= READ_MAX_LINE_LENGTH + 1 {
                    self.line.push(ch);
                }
            }
        }
    }
    fn end_line(&mut self) {
        self.total += 1;
        if !self.full && self.total >= self.offset && self.total - self.offset < self.limit {
            let raw = self.line.strip_suffix('\r').unwrap_or(&self.line);
            let text = if self.chars > READ_MAX_LINE_LENGTH {
                format!(
                    "{}... (line truncated to {READ_MAX_LINE_LENGTH} chars)",
                    raw.chars().take(READ_MAX_LINE_LENGTH).collect::<String>()
                )
            } else {
                raw.into()
            };
            let extra = text.len() + usize::from(!self.lines.is_empty());
            if self.bytes + extra > READ_MAX_BYTES {
                self.full = true;
            } else {
                self.bytes += extra;
                self.lines.push(json!({"number":self.total,"text":text}));
            }
        }
        self.line.clear();
        self.chars = 0;
    }
    pub fn finish(mut self) -> (u64, Vec<Value>) {
        if self.chars > 0 {
            self.end_line();
        }
        (self.total, self.lines)
    }
}
pub(crate) fn decode(bytes: &[u8], encoding: &str) -> Result<String, String> {
    if bytes.starts_with(b"%PDF-") || bytes.starts_with(b"PK\x03\x04") {
        return Err("PDF/Office files require document preview or rendering; text encoding does not turn a document package into text".into());
    }
    let codec = match encoding {
        "gb18030" | "gbk" => encoding_rs::GB18030,
        "utf-16le" => encoding_rs::UTF_16LE,
        "utf-16be" => encoding_rs::UTF_16BE,
        "utf-8" => encoding_rs::UTF_8,
        _ => return Err("Unsupported text encoding".into()),
    };
    let (text, _, errors) = codec.decode(bytes);
    if errors || text.contains('\0') {
        return Err(format!(
            "File is not valid {encoding} text; confirm the file type and encoding"
        ));
    }
    Ok(text.into_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_lines_keep_bounded_prefix_and_real_line_numbers() {
        let mut window = ReadWindow::new(2, 2);
        window.push("skip\r");
        window.push("\n");
        for _ in 0..10000 {
            window.push("中文字0123456789");
        }
        assert!(window.line.len() < READ_MAX_LINE_LENGTH * 4 + 8);
        window.push("\nlast\n");
        let (total, lines) = window.finish();
        assert_eq!(total, 3);
        assert_eq!(lines[0]["number"], 2);
        assert!(lines[0]["text"].as_str().unwrap().ends_with("chars)"));
        assert_eq!(lines[1]["text"], "last");
    }
    #[test]
    fn explicit_encoding_preserves_chinese_and_rejects_documents() {
        let (bytes, _, _) = encoding_rs::GBK.encode("中文日志");
        assert_eq!(decode(&bytes, "gb18030").unwrap(), "中文日志");
        assert!(decode(b"%PDF-1.7", "gb18030").is_err());
        assert!(decode(&[255], "utf-8").is_err());
    }
}
