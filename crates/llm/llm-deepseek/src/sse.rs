use dsh_llm::LlmFailure;

pub(crate) const DONE: &str = "[DONE]";

fn malformed(message: impl Into<String>) -> LlmFailure {
    LlmFailure {
        offload_images: None,
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
    retained_data_bytes: usize,
    max_pending_bytes: usize,
    first_line: bool,
    done: bool,
    failure: Option<LlmFailure>,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::with_limit(32 * 1024 * 1024)
    }

    fn with_limit(max_pending_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            data: Vec::new(),
            retained_data_bytes: 0,
            max_pending_bytes,
            first_line: true,
            done: false,
            failure: None,
        }
    }

    fn reject(&mut self, error: LlmFailure) -> Result<String, LlmFailure> {
        self.failure = Some(error.clone());
        self.bytes = Vec::new();
        self.data = Vec::new();
        self.retained_data_bytes = 0;
        Err(error)
    }

    fn oversized(&mut self) -> Result<String, LlmFailure> {
        let mut error = malformed(format!(
            "SSE pending event exceeded {} bytes",
            self.max_pending_bytes
        ));
        error.code = "RESPONSE_TOO_LARGE".into();
        self.reject(error)
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
        let mut payloads = Vec::new();
        // Bound the incomplete event, not the lifetime of a healthy stream.
        // Process line segments directly so a large batch of small events does
        // not keep shifting the entire remaining network buffer.
        for segment in chunk.split_inclusive(|byte| *byte == b'\n') {
            if self
                .retained_data_bytes
                .saturating_add(self.bytes.len())
                .saturating_add(segment.len())
                > self.max_pending_bytes
            {
                payloads.push(self.oversized());
                break;
            }
            self.bytes.extend_from_slice(segment);
            if segment.last() != Some(&b'\n') {
                continue;
            }
            let mut line = std::mem::take(&mut self.bytes);
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let mut line = match std::str::from_utf8(&line) {
                Ok(line) => line,
                Err(_) => {
                    let error = malformed("SSE stream contained invalid UTF-8");
                    payloads.push(self.reject(error));
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
                    self.retained_data_bytes = 0;
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
                let value = value.strip_prefix(' ').unwrap_or(value);
                self.retained_data_bytes = self
                    .retained_data_bytes
                    .saturating_add(value.len())
                    .saturating_add(std::mem::size_of::<String>() + 1);
                if self.retained_data_bytes > self.max_pending_bytes {
                    payloads.push(self.oversized());
                    break;
                }
                self.data.push(value.to_string());
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
        self.push(b"\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_stream_can_exceed_eight_mib_without_retaining_completed_events() {
        let event = format!("data: {}\n\n", "x".repeat(4096));
        let mut parser = SseParser::new();
        for _ in 0..2300 {
            let payloads = parser.push(event.as_bytes());
            assert_eq!(payloads.len(), 1);
            assert_eq!(payloads[0].as_ref().unwrap().len(), 4096);
            assert!(parser.bytes.is_empty() && parser.data.is_empty());
            assert_eq!(parser.retained_data_bytes, 0);
        }
        assert_eq!(
            parser.push(b"data: [DONE]\n\n")[0].as_deref().unwrap(),
            DONE
        );
    }

    #[test]
    fn oversized_pending_line_or_multiline_event_fails_after_valid_prefix() {
        for suffix in [format!("data: {}", "x".repeat(150)), "data:\n".repeat(10)] {
            let mut parser = SseParser::with_limit(128);
            let mut events = parser.push(b"data: valid\n\n");
            for fragment in suffix.as_bytes().chunks(16) {
                events.extend(parser.push(fragment));
                if parser.failure.is_some() {
                    break;
                }
            }
            assert_eq!(events[0].as_deref().unwrap(), "valid");
            assert_eq!(events[1].as_ref().unwrap_err().code, "RESPONSE_TOO_LARGE");
            assert!(parser.bytes.is_empty() && parser.data.is_empty());
            assert!(parser.push(b"data: later\n\n")[0].is_err());
        }
    }

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
