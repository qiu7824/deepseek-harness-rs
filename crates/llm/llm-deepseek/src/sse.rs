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

    pub(crate) fn finish(&self) -> Result<(), LlmFailure> {
        if self.done {
            return Ok(());
        }
        Err(LlmFailure {
            message: "SSE stream ended without [DONE]".to_string(),
            code: "STREAM_CLOSED".to_string(),
            status: None,
            provider_retry_after_ms: None,
            request_id: None,
        })
    }
}
