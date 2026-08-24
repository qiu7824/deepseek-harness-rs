//! Zstandard frame primitives for the JSONL persistence backend. Rust port
//! of `packages/session/session-persistence-jsonl/src/zstd.ts`.
//!
//! The backend owns a concatenated-frame container: each durable batch is one
//! independently decodable, checksummed frame, so appends never rewrite prior
//! bytes and a torn final frame can be located structurally.

/// The Zstandard frame magic number (LE bytes `28 B5 2F FD`).
const ZSTD_MAGIC: u32 = 0xFD2FB528;

/// Byte range occupied by one structurally complete Zstandard frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZstdFrameRange {
    /// Inclusive frame start.
    pub start: usize,
    /// Exclusive frame end.
    pub end: usize,
}

/// Structural scan result for a concatenated Zstandard stream.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZstdFrameScan {
    /// Complete frames in file order.
    pub frames: Vec<ZstdFrameRange>,
    /// Start of an incomplete final frame, when EOF interrupts one.
    pub torn_start: Option<usize>,
}

/// Locate complete frames without decompressing their blocks (TS
/// `scanZstdFrames`).
pub fn scan_zstd_frames(buffer: &[u8]) -> Result<ZstdFrameScan, String> {
    let mut frames = Vec::new();
    let mut offset = 0usize;
    while offset < buffer.len() {
        let start = offset;
        if buffer.len() - offset < 4 {
            return Ok(ZstdFrameScan {
                frames,
                torn_start: Some(start),
            });
        }
        if u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) != ZSTD_MAGIC {
            return Err(format!(
                "corrupt Zstandard session log: invalid frame magic at byte {offset}"
            ));
        }
        offset += 4;

        if offset == buffer.len() {
            return Ok(ZstdFrameScan {
                frames,
                torn_start: Some(start),
            });
        }
        let descriptor = buffer[offset];
        offset += 1;
        if (descriptor & 0x18) != 0 {
            return Err(format!(
                "corrupt Zstandard session log: reserved frame-header bit at byte {}",
                offset - 1
            ));
        }

        let content_size_flag = (descriptor >> 6) as usize;
        let single_segment = (descriptor & 0x20) != 0;
        let checksum = (descriptor & 0x04) != 0;
        let dictionary_flag = (descriptor & 0x03) as usize;
        let dictionary_bytes = if dictionary_flag == 3 {
            4
        } else {
            dictionary_flag
        };
        let content_size_bytes = if content_size_flag == 0 {
            if single_segment { 1 } else { 0 }
        } else {
            1 << content_size_flag
        };
        let remaining_header_bytes =
            (if single_segment { 0 } else { 1 }) + dictionary_bytes + content_size_bytes;
        if buffer.len() - offset < remaining_header_bytes {
            return Ok(ZstdFrameScan {
                frames,
                torn_start: Some(start),
            });
        }
        offset += remaining_header_bytes;

        loop {
            if buffer.len() - offset < 3 {
                return Ok(ZstdFrameScan {
                    frames,
                    torn_start: Some(start),
                });
            }
            let block_header =
                u32::from_le_bytes([buffer[offset], buffer[offset + 1], buffer[offset + 2], 0]);
            offset += 3;
            let last_block = (block_header & 1) != 0;
            let block_type = ((block_header >> 1) & 0x03) as u8;
            let block_size = (block_header >> 3) as usize;
            if block_type == 0x03 {
                return Err(format!(
                    "corrupt Zstandard session log: reserved block type at byte {}",
                    offset - 3
                ));
            }
            let payload_bytes = if block_type == 0x01 { 1 } else { block_size };
            if buffer.len() - offset < payload_bytes {
                return Ok(ZstdFrameScan {
                    frames,
                    torn_start: Some(start),
                });
            }
            offset += payload_bytes;
            if last_block {
                break;
            }
        }

        if checksum {
            if buffer.len() - offset < 4 {
                return Ok(ZstdFrameScan {
                    frames,
                    torn_start: Some(start),
                });
            }
            offset += 4;
        }
        frames.push(ZstdFrameRange { start, end: offset });
    }
    Ok(ZstdFrameScan {
        frames,
        torn_start: None,
    })
}

/// Compress one independently decodable, checksummed Zstandard frame
/// (TS `compressZstdFrame`).
pub fn compress_zstd_frame(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 0)
        .map_err(|error| format!("zstd encoder failed: {error}"))?;
    encoder
        .include_checksum(true)
        .map_err(|error| format!("zstd checksum flag failed: {error}"))?;
    // One frame per call: finish() emits exactly one frame.
    std::io::Write::write_all(&mut encoder, input)
        .map_err(|error| format!("zstd write failed: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("zstd finish failed: {error}"))
}

/// Decompress one complete frame and validate its checksum
/// (TS `decompressZstdFrame`).
pub fn decompress_zstd_frame(input: &[u8]) -> Result<Vec<u8>, String> {
    zstd::stream::decode_all(input).map_err(|error| format!("zstd decode failed: {error}"))
}

/// Recover available plaintext from a structurally incomplete final frame
/// (TS `decompressZstdPrefix`). The zstd stream decoder emits what it can
/// before the truncation error; partial plaintext is preserved.
pub fn decompress_zstd_prefix(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut decoder = match zstd::stream::read::Decoder::new(input) {
        Ok(decoder) => decoder,
        Err(_) => return Vec::new(),
    };
    let mut plaintext = Vec::new();
    // Read until the truncated stream errors; whatever was produced is the
    // recoverable prefix.
    let _ = std::io::copy(&mut decoder, &mut plaintext);
    plaintext
}

/// Whether one header-frame plaintext is exactly one header line.
fn assert_zstd_header_frame(plaintext: &[u8]) -> Result<(), String> {
    if plaintext.is_empty()
        || plaintext.last() != Some(&0x0A)
        || plaintext
            .iter()
            .take(plaintext.len() - 1)
            .any(|byte| *byte == 0x0A)
    {
        return Err(
            "corrupt Zstandard session log: first frame is not exactly one header line".to_string(),
        );
    }
    Ok(())
}

/// Decode complete frames in source order into owned plaintext buffers.
pub fn decode_zstd_frames(
    source: &[u8],
    frames: &[ZstdFrameRange],
) -> Result<Vec<Vec<u8>>, String> {
    let mut plaintexts = Vec::with_capacity(frames.len());
    for frame in frames {
        let plaintext = decompress_zstd_frame(&source[frame.start..frame.end])?;
        plaintexts.push(plaintext);
    }
    Ok(plaintexts)
}

/// Parse one ALREADY-DECOMPRESSED header-frame plaintext and return the
/// header line WITHOUT the trailing newline. Callers that only have raw
/// frame bytes must decompress first (see [`decode_zstd_header_line`]).
pub fn parse_zstd_header_plaintext(plaintext: &[u8]) -> Result<String, String> {
    assert_zstd_header_frame(plaintext)?;
    Ok(String::from_utf8_lossy(&plaintext[..plaintext.len() - 1]).to_string())
}

/// Decode the independently decodable first frame and assert it is the
/// header record; returns the header line WITHOUT the trailing newline.
pub fn decode_zstd_header_line(first_frame: &[u8]) -> Result<String, String> {
    let plaintext = decompress_zstd_frame(first_frame)?;
    parse_zstd_header_plaintext(&plaintext)
}
