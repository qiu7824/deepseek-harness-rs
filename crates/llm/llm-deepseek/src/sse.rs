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
    failure: Option<LlmFailure>,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            data: Vec::new(),
            first_line: true,
            done: false,
            failure: None,
        }
    }

    /// Keep a malformed line after all complete preceding events in the same
    /// batch. Callers can deliver that valid prefix before failing the stream.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<Result<String, LlmFailure>> {
        if let Some(error) = &self.failure {
            return vec![Err(error.clone())];
        }
        if self.done {
            return Vec::new();
        }
        self.bytes.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(newline) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let mut line = self.bytes.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let mut line = match std::str::from_utf8(&line) {
                Ok(line) => line,
                Err(_) => {
                    let error = malformed("SSE stream contained invalid UTF-8");
                    self.failure = Some(error.clone());
                    self.bytes.clear();
                    self.data.clear();
                    payloads.push(Err(error));
                    break;
                }
            };
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
                    payloads.push(Ok(payload));
                    if self.done {
                        self.bytes.clear();
                        break;
                    }
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
        payloads
    }

    /// Flush a final event at EOF for protocols whose translator owns the
    /// terminal marker instead of using chat-completions `[DONE]`.
    pub(crate) fn finish_at_eof(&mut self) -> Vec<Result<String, LlmFailure>> {
        if let Some(error) = &self.failure {
            return vec![Err(error.clone())];
        }
        if self.done || (self.bytes.is_empty() && self.data.is_empty()) {
            return Vec::new();
        }
        self.bytes.push(b'\n');
        self.bytes.push(b'\n');
        self.push(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_events_precede_a_same_batch_encoding_failure_and_cannot_resume() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: first\n\ndata: second\n\ndata: \xff\n\ndata: ignored\n\n");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].as_deref().unwrap(), "first");
        assert_eq!(events[1].as_deref().unwrap(), "second");
        assert_eq!(events[2].as_ref().unwrap_err().code, "MALFORMED_RESPONSE");
        assert!(parser.push(b"data: later\n\n")[0].is_err());
        assert!(parser.finish_at_eof()[0].is_err());
    }

    #[test]
    fn eof_reports_incomplete_utf8_after_delivering_the_complete_prefix() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: valid\n\ndata: \xe4\xb8");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_deref().unwrap(), "valid");
        assert_eq!(
            parser.finish_at_eof()[0].as_ref().unwrap_err().code,
            "MALFORMED_RESPONSE"
        );
    }

    #[test]
    fn split_utf8_and_a_final_event_without_blank_separator_stay_valid() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: \xe4\xb8").is_empty());
        assert!(parser.push(b"\xad").is_empty());
        let events = parser.finish_at_eof();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_deref().unwrap(), "中");
        assert!(parser.finish_at_eof().is_empty());
    }

    #[test]
    fn explicit_done_ends_the_protocol_before_unrelated_tail_bytes() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: valid\n\ndata: [DONE]\n\ndata: \xff\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].as_deref().unwrap(), DONE);
        assert!(parser.finish_at_eof().is_empty());
    }
}
