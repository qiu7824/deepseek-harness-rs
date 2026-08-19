use std::fmt;

const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramingError(String);

impl fmt::Display for FramingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for FramingError {}

pub fn encode_message(message: &serde_json::Value) -> Result<Vec<u8>, FramingError> {
    let body = serde_json::to_vec(message).map_err(|error| FramingError(error.to_string()))?;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    Ok(frame)
}

pub struct MessageDecoder {
    max_message_bytes: usize,
    buffer: Vec<u8>,
}

impl MessageDecoder {
    pub fn new(max_message_bytes: usize) -> Self {
        Self {
            max_message_bytes,
            buffer: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<serde_json::Value>, FramingError> {
        self.buffer.extend_from_slice(chunk);
        let mut messages = Vec::new();
        while let Some(separator) = self
            .buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            if separator > MAX_HEADER_BYTES {
                return Err(FramingError("LSP header exceeded 64 KiB".to_string()));
            }
            let header = std::str::from_utf8(&self.buffer[..separator])
                .map_err(|error| FramingError(format!("LSP header was not ASCII: {error}")))?;
            let length = header
                .split("\r\n")
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                })
                .ok_or_else(|| FramingError("LSP header block missing Content-Length".to_string()))?
                .parse::<usize>()
                .map_err(|_| FramingError("invalid Content-Length header".to_string()))?;
            if length > self.max_message_bytes {
                return Err(FramingError(format!(
                    "LSP message length {length} exceeds the {}-byte limit",
                    self.max_message_bytes
                )));
            }
            let body_start = separator
                .checked_add(4)
                .ok_or_else(|| FramingError("LSP frame boundary overflowed".to_string()))?;
            let body_end = body_start
                .checked_add(length)
                .ok_or_else(|| FramingError("LSP message length overflowed".to_string()))?;
            if self.buffer.len() < body_end {
                break;
            }
            let message =
                serde_json::from_slice(&self.buffer[body_start..body_end]).map_err(|error| {
                    FramingError(format!("LSP message body was not valid JSON: {error}"))
                })?;
            self.buffer.drain(..body_end);
            messages.push(message);
        }
        if self
            .buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .is_none()
            && self.buffer.len() > MAX_HEADER_BYTES
        {
            return Err(FramingError("LSP header exceeded 64 KiB".to_string()));
        }
        Ok(messages)
    }
}
