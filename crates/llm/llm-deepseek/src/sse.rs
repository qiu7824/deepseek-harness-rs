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

    #[cfg(test)]
    pub(crate) fn finish(&mut self) -> Result<Vec<String>, LlmFailure> {
        if self.done {
            return Ok(Vec::new());
        }
        if !self.bytes.is_empty() {
            self.bytes.push(b'\n');
            self.bytes.push(b'\n');
            let payloads = self.push(&[])?;
            if self.done {
                return Ok(payloads);
            }
        }
        Err(LlmFailure {
            message: "SSE stream ended without [DONE]".to_string(),
            code: "STREAM_CLOSED".to_string(),
            status: None,
            provider_retry_after_ms: None,
            request_id: None,
        })
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

#[cfg(test)]
mod tests {
    use super::{DONE, SseParser};

    #[test]
    fn finish_flushes_a_done_event_without_a_trailing_blank_line() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: [DONE]").expect("push").is_empty());
        assert_eq!(parser.finish().expect("finish"), vec![DONE.to_string()]);
    }

    #[test]
    fn finish_still_rejects_a_truncated_non_done_event() {
        let mut parser = SseParser::new();
        parser
            .push(b"data: {\"choices\":[]}")
            .expect("push truncated event");
        let error = parser.finish().expect_err("missing done must fail");
        assert_eq!(error.code, "STREAM_CLOSED");
    }
}
