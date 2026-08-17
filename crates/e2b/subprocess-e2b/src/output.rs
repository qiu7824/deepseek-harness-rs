//! Bounded host-side projection of a complete output file retained in E2B.
//! Rust port of `output.ts`.

use base64::Engine as _;
use dsh_subprocess::SubprocessOutputRead;

/// Reserved non-base64 frame proving that one remote encoder reached clean
/// EOF (TS `E2B_OUTPUT_COMPLETE_FRAME`).
pub const E2B_OUTPUT_COMPLETE_FRAME: &str = "!dsh-e2b-output-complete!";

/// Whether one frame is canonical base64 text (the TS `BASE64_TEXT` regex
/// contract).
fn is_base64_text(frame: &str) -> bool {
    if frame.is_empty() || !frame.is_ascii() {
        return false;
    }
    match base64::engine::general_purpose::STANDARD.decode(frame) {
        Ok(bytes) => base64::engine::general_purpose::STANDARD.encode(&bytes) == frame,
        Err(_) => false,
    }
}

/// Incrementally decode newline-delimited base64 frames emitted by one
/// remote encoder (TS `E2BBase64Decoder`).
pub struct E2bBase64Decoder {
    pending: String,
    complete: bool,
}

impl E2bBase64Decoder {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            complete: false,
        }
    }

    /// Decode every complete newline-delimited frame in one arbitrarily
    /// split SDK callback (TS `push`).
    pub fn push(&mut self, text: &str) -> Result<Vec<u8>, String> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        self.pending.push_str(text);
        let mut decoded: Vec<u8> = Vec::new();
        loop {
            let Some(boundary) = self.pending.find('\n') else {
                break;
            };
            let frame = self.pending[..boundary].to_string();
            self.pending.drain(..=boundary);
            if frame == E2B_OUTPUT_COMPLETE_FRAME {
                if self.complete {
                    return Err("subprocess-e2b: duplicate output transport completion".to_string());
                }
                self.complete = true;
                continue;
            }
            if self.complete {
                return Err("subprocess-e2b: output transport continued after completion".to_string());
            }
            if !is_base64_text(&frame) {
                return Err("subprocess-e2b: invalid base64 output transport".to_string());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&frame)
                .map_err(|error| format!("subprocess-e2b: invalid base64 output transport: {error}"))?;
            decoded.extend_from_slice(&bytes);
        }
        Ok(decoded)
    }

    /// Validate clean encoder completion, or discard an interrupted trailing
    /// frame after requested termination (TS `finish`).
    pub fn finish(&mut self, require_complete: bool) -> Result<(), String> {
        if !require_complete {
            self.pending.clear();
            return Ok(());
        }
        if !self.pending.is_empty() {
            return Err("subprocess-e2b: truncated base64 output transport".to_string());
        }
        if !self.complete {
            return Err("subprocess-e2b: incomplete output transport".to_string());
        }
        Ok(())
    }
}

/// Offset reader used for one collect-mode E2B stream (TS
/// `E2BOutputReader`).
pub struct E2bOutputReader {
    max_bytes: u64,
    max_spill_bytes: Option<u64>,
    spill_path: String,
    chunks: Vec<Vec<u8>>,
    retained_bytes: u64,
    total_bytes: u64,
    spill_valid: bool,
}

impl E2bOutputReader {
    pub fn new(max_bytes: u64, max_spill_bytes: Option<u64>, spill_path: String) -> Self {
        Self {
            max_bytes,
            max_spill_bytes,
            spill_path,
            chunks: Vec::new(),
            retained_bytes: 0,
            total_bytes: 0,
            spill_valid: true,
        }
    }

    /// Total bytes observed from the SDK stream (TS `size`).
    pub fn size(&self) -> u64 {
        self.total_bytes
    }

    /// Stop advertising a remote spill whose writer did not reach clean EOF
    /// (TS `invalidateSpill`).
    pub fn invalidate_spill(&mut self) {
        self.spill_valid = false;
    }

    /// Append one byte-faithful decoded transport event (TS `push`).
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let chunk = bytes.to_vec();
        self.total_bytes += chunk.len() as u64;
        self.chunks.push(chunk);
        self.retained_bytes += bytes.len() as u64;
        while self.retained_bytes > self.max_bytes {
            let head_len = self.chunks[0].len() as u64;
            let excess = self.retained_bytes - self.max_bytes;
            if head_len <= excess {
                self.chunks.remove(0);
                self.retained_bytes -= head_len;
            } else {
                let cut = excess as usize;
                let kept = self.chunks[0][cut..].to_vec();
                self.chunks[0] = kept;
                self.retained_bytes -= excess;
            }
        }
    }
}

impl dsh_subprocess::SubprocessOutputReader for E2bOutputReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let mut retained = Vec::with_capacity(self.retained_bytes as usize);
        for chunk in &self.chunks {
            retained.extend_from_slice(chunk);
        }
        let first_retained = self.total_bytes - self.retained_bytes;
        let lossy = from_byte < first_retained;
        let start = if lossy {
            0
        } else {
            (from_byte - first_retained)
                .min(retained.len() as u64)
                .max(0) as usize
        };
        let spill_path = if lossy
            && self.spill_valid
            && self.max_spill_bytes.is_some()
            && self.total_bytes <= self.max_spill_bytes.unwrap_or(0)
        {
            Some(self.spill_path.clone())
        } else {
            None
        };
        SubprocessOutputRead {
            text: String::from_utf8_lossy(&retained[start..]).into_owned(),
            next_offset: self.total_bytes,
            lossy,
            spill_path,
        }
    }
}
