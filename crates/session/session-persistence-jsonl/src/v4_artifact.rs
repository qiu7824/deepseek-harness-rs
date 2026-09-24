//! Streaming validation for complete V4 generation artifacts. Callbacks may
//! observe a frame before its checksum is verified, so their output must remain
//! private until the entire operation succeeds.

use crate::JsonlCompression;
use dsh_session::format_v4::{
    V4LogScan, V4LogScanner, V4Recovery, V4ValidationSummary, V4Validator, V4Vocabulary,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const BUFFER: usize = 32 * 1024;
const HEADER_LIMIT: usize = 256 * 1024;

pub(crate) enum ArtifactPart<'a> {
    Header(&'a [u8]),
    Body(&'a [u8]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactFraming {
    pub physical_bytes: u64,
    pub plaintext_bytes: u64,
    pub frames: u64,
    pub recovered_tail: bool,
}

pub(crate) fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Session generation operation cancelled; no generation was published".into())
    } else {
        Ok(())
    }
}

/// Read complete frames using a fixed-size input/output buffer. Neither the
/// compressed artifact nor the decoded conversation is collected or mapped.
pub(crate) fn visit_artifact(
    file: &mut (impl Read + Seek),
    compression: JsonlCompression,
    cancelled: &impl Fn() -> bool,
    visit: impl FnMut(ArtifactPart<'_>) -> Result<(), String>,
) -> Result<ArtifactFraming, String> {
    visit_artifact_with_recovery(file, compression, cancelled, false, visit)
}

/// Live journals can end during a write. Only physical EOF in the final body
/// frame is recoverable; checksum failures and complete corrupt frames are not.
pub(crate) fn visit_artifact_with_recovery(
    file: &mut (impl Read + Seek),
    compression: JsonlCompression,
    cancelled: &impl Fn() -> bool,
    recover_tail: bool,
    mut visit: impl FnMut(ArtifactPart<'_>) -> Result<(), String>,
) -> Result<ArtifactFraming, String> {
    check_cancel(cancelled)?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(BUFFER, file);
    let mut header = Vec::new();
    let mut buffer = [0; BUFFER];
    let mut plaintext_bytes = 0_u64;
    let mut frames = 0_u64;
    let mut recovered_tail = false;
    match compression {
        JsonlCompression::None => {
            reader
                .by_ref()
                .take(HEADER_LIMIT as u64 + 1)
                .read_until(b'\n', &mut header)
                .map_err(|e| e.to_string())?;
            check_header(&header)?;
            plaintext_bytes += header.len() as u64;
            visit(ArtifactPart::Header(&header))?;
            loop {
                check_cancel(cancelled)?;
                let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                plaintext_bytes += count as u64;
                visit(ArtifactPart::Body(&buffer[..count]))?;
            }
        }
        JsonlCompression::Zstd => {
            'frames: while !reader.fill_buf().map_err(|e| e.to_string())?.is_empty() {
                check_cancel(cancelled)?;
                let start = reader.stream_position().map_err(|e| e.to_string())?;
                let mut magic = [0; 4];
                if let Err(error) = reader.read_exact(&mut magic) {
                    if recover_tail
                        && frames > 0
                        && error.kind() == std::io::ErrorKind::UnexpectedEof
                    {
                        recovered_tail = true;
                        break;
                    }
                    return Err(error.to_string());
                }
                if magic != [0x28, 0xb5, 0x2f, 0xfd] {
                    return Err(format!("invalid Zstandard frame magic at byte {start}"));
                }
                reader
                    .seek(SeekFrom::Start(start))
                    .map_err(|e| e.to_string())?;
                let mut decoder = zstd::stream::read::Decoder::with_buffer(&mut reader)
                    .map_err(|e| e.to_string())?
                    .single_frame();
                if frames == 0 {
                    // Match the existing bounded authority-header reader.
                    decoder.window_log_max(21).map_err(|e| e.to_string())?;
                }
                let mut last = None;
                loop {
                    check_cancel(cancelled)?;
                    let count = match decoder.read(&mut buffer) {
                        Ok(count) => count,
                        Err(error)
                            if recover_tail
                                && frames > 0
                                && error.kind() == std::io::ErrorKind::UnexpectedEof =>
                        {
                            recovered_tail = true;
                            decoder.finish();
                            break 'frames;
                        }
                        Err(error) => {
                            return Err(format!(
                                "incomplete or corrupt Zstandard frame at byte {start}: {error}"
                            ));
                        }
                    };
                    if count == 0 {
                        break;
                    }
                    plaintext_bytes += count as u64;
                    last = Some(buffer[count - 1]);
                    if frames == 0 {
                        if header.len() + count > HEADER_LIMIT {
                            return Err("Session authority header exceeds 256 KiB".into());
                        }
                        header.extend_from_slice(&buffer[..count]);
                    } else {
                        visit(ArtifactPart::Body(&buffer[..count]))?;
                    }
                }
                decoder.finish();
                if frames == 0 {
                    check_header(&header)?;
                    visit(ArtifactPart::Header(&header))?;
                } else if last.is_some_and(|byte| byte != b'\n') {
                    if !recover_tail || !reader.fill_buf().map_err(|e| e.to_string())?.is_empty() {
                        return Err("Zstandard body frame ends inside a JSONL record".into());
                    }
                    recovered_tail = true;
                }
                frames += 1;
            }
            if frames == 0 {
                return Err("empty Zstandard Session artifact".into());
            }
        }
    }
    check_cancel(cancelled)?;
    Ok(ArtifactFraming {
        physical_bytes: reader.stream_position().map_err(|e| e.to_string())?,
        plaintext_bytes,
        frames,
        recovered_tail,
    })
}

fn check_header(header: &[u8]) -> Result<(), String> {
    if header.len() > HEADER_LIMIT {
        return Err("Session authority header exceeds 256 KiB".into());
    }
    if header.last() != Some(&b'\n') || header[..header.len() - 1].contains(&b'\n') {
        return Err("Session header must be exactly one complete JSONL record".into());
    }
    std::str::from_utf8(header).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct V4ArtifactValidation {
    pub framing: ArtifactFraming,
    pub scan: V4LogScan,
    pub lifecycle: V4ValidationSummary,
}

/// Validate a complete staged generation. The first streaming pass discovers
/// the inherited cut; the second validates relationships with that exact cut.
/// A successful return does not establish publication or writer ownership.
pub fn validate_v4_artifact(
    path: &Path,
    compression: JsonlCompression,
    expected_id: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<V4ArtifactValidation, String> {
    let mut source = crate::generations::StableGenerationSource::open(path, cancelled)?;
    let result = validate_open_v4(source.file(), compression, expected_id, cancelled)?;
    source.assert_unchanged(cancelled)?;
    Ok(result)
}

pub(crate) fn validate_open_v4(
    file: &mut File,
    compression: JsonlCompression,
    expected_id: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<V4ArtifactValidation, String> {
    fn scan(
        file: &mut File,
        compression: JsonlCompression,
        cancelled: &impl Fn() -> bool,
        mut event: impl FnMut(serde_json::Value) -> Result<(), String>,
    ) -> Result<(ArtifactFraming, V4LogScan), String> {
        let mut scanner = None;
        let framing = visit_artifact(file, compression, cancelled, |part| match part {
            ArtifactPart::Header(line) => {
                scanner = Some(V4LogScanner::new(
                    line,
                    V4Recovery::Strict,
                    V4Vocabulary::default(),
                )?);
                Ok(())
            }
            ArtifactPart::Body(chunk) => scanner
                .as_mut()
                .ok_or("missing Session header")?
                .write(chunk, &mut event),
        })?;
        Ok((framing, scanner.ok_or("missing Session header")?.finish()?))
    }
    let (framing, first) = scan(file, compression, cancelled, |_| Ok(()))?;
    if first.decoded.header["id"].as_str() != Some(expected_id) {
        return Err("Session artifact identity mismatch".into());
    }
    let mut validator = V4Validator::new(
        first.decoded.header.clone(),
        first.decoded.inherited_event_count,
    )?;
    let (second_framing, second) = scan(file, compression, cancelled, |row| validator.push(&row))?;
    if first != second || framing != second_framing {
        return Err("Session artifact changed between validation passes".into());
    }
    let lifecycle = validator.finish()?;
    if lifecycle.event_count != first.decoded.event_count {
        return Err("Session artifact lifecycle length mismatch".into());
    }
    Ok(V4ArtifactValidation {
        framing,
        scan: first,
        lifecycle,
    })
}

#[cfg(test)]
mod tests;
