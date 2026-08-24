use dsh_llm::LlmFailure;

pub(crate) const DONE: &str = "[DONE]";

fn malformed(message: impl Into<String>) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: "MALFORMED_RESPONSE".to_string(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

pub(crate) struct SseParser {
    bytes: Vec<u8>,
    data: Vec<String>,
    first_line: bool,
    done: bool,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            data: Vec::new(),
            first_line: true,
            done: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, LlmFailure> {
        if self.done {
            return Ok(Vec::new());
        }
        self.bytes.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(newline) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let mut line = std::str::from_utf8(&line)
                .map_err(|_| malformed("SSE stream contained invalid UTF-8"))?;
            if self.first_line {
                self.first_line = false;
                line = line.strip_prefix('\u{feff}').unwrap_or(line);
            }
            if line.is_empty() {
                if !self.data.is_empty() {
                    let payload = self.data.join("\n");
                    self.data.clear();
                    if payload == DONE {
                        self.done = true;
                    }
                    payloads.push(payload);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            if field == "data" {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            }
        }
        Ok(payloads)
    }

    /// Flush a final event at EOF for protocols whose translator owns the
    /// terminal marker instead of using chat-completions `[DONE]`.
    pub(crate) fn finish_at_eof(&mut self) -> Result<Vec<String>, LlmFailure> {
        if self.done || (self.bytes.is_empty() && self.data.is_empty()) {
            return Ok(Vec::new());
        }
        self.bytes.push(b'\n');
        self.bytes.push(b'\n');
        self.push(&[])
    }
}
