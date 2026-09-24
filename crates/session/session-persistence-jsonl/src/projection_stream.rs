use super::*;
use dsh_session_persistence::StoredEventNormalizer;
use std::io::{Read, Seek, SeekFrom};

/// Structural frame validation without mapping the compressed file into the
/// Host's working set. The decoder separately verifies each frame checksum.
fn complete_zstd_layout(file: &mut std::fs::File, cancelled: &AtomicBool) -> Result<bool, String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let mut offset = 0;
    fn advance(offset: &mut u64, count: u64, length: u64) -> bool {
        let Some(next) = offset.checked_add(count).filter(|next| *next <= length) else {
            return false;
        };
        *offset = next;
        true
    }
    while offset < length {
        if cancelled.load(Ordering::Acquire) {
            return Err("projection replay cancelled".into());
        }
        if length - offset < 5 {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut header = [0; 5];
        file.read_exact(&mut header).map_err(|e| e.to_string())?;
        if header[..4] != [0x28, 0xb5, 0x2f, 0xfd] || header[4] & 0x18 != 0 {
            return Ok(false);
        }
        let descriptor = header[4];
        let single = descriptor & 0x20 != 0;
        let flag = descriptor >> 6;
        let size = if flag == 0 {
            u64::from(single)
        } else {
            1 << flag
        };
        let dictionary = match descriptor & 3 {
            3 => 4,
            n => n as u64,
        };
        if !advance(
            &mut offset,
            5 + u64::from(!single) + dictionary + size,
            length,
        ) {
            return Ok(false);
        }
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err("projection replay cancelled".into());
            }
            if length - offset < 3 {
                return Ok(false);
            }
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut bytes = [0; 3];
            file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            let block = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
            let kind = (block >> 1) & 3;
            if kind == 3 {
                return Ok(false);
            }
            let payload = if kind == 1 { 1 } else { (block >> 3) as u64 };
            if !advance(&mut offset, 3 + payload, length) {
                return Ok(false);
            }
            if block & 1 != 0 {
                break;
            }
        }
        if descriptor & 4 != 0 && !advance(&mut offset, 4, length) {
            return Ok(false);
        }
    }
    Ok(length > 0)
}

/// Preserve JSONL record boundaries even though the streaming serde parser
/// also understands whitespace-separated JSON values. A malformed/torn record
/// selects the existing recovery path before any projection is emitted.
struct CheckedLines<R> {
    reader: R,
    cancelled: Arc<AtomicBool>,
    depth: usize,
    quoted: bool,
    escaped: bool,
    started: bool,
    closed: bool,
}
impl<R: Read> Read for CheckedLines<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("projection replay cancelled"));
        }
        let count = self.reader.read(buffer)?;
        if count == 0 && self.started {
            return Err(std::io::Error::other("incomplete JSONL record"));
        }
        for &byte in &buffer[..count] {
            if byte == b'\n' {
                if !self.closed || self.quoted || self.depth != 0 {
                    return Err(std::io::Error::other("invalid JSONL record boundary"));
                }
                self.started = false;
                self.closed = false;
                continue;
            }
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.quoted = false;
                }
                continue;
            }
            if byte.is_ascii_whitespace() {
                continue;
            }
            if self.closed {
                return Err(std::io::Error::other("multiple values in one JSONL record"));
            }
            if !self.started {
                if byte != b'{' {
                    return Err(std::io::Error::other("event record must be an object"));
                }
                self.started = true;
            }
            match byte {
                b'"' => self.quoted = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => {
                    self.depth = self
                        .depth
                        .checked_sub(1)
                        .ok_or_else(|| std::io::Error::other("invalid JSON nesting"))?;
                    if self.depth == 0 {
                        self.closed = true;
                    }
                }
                _ => {}
            }
        }
        Ok(count)
    }
}

fn visit_native(
    path: &Path,
    compression: JsonlCompression,
    cancelled: Arc<AtomicBool>,
    visitor: &mut impl FnMut(SessionEvent) -> Result<bool, String>,
) -> Result<(), String> {
    let before = file_revision(&std::fs::metadata(path).map_err(|e| e.to_string())?);
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut header = Vec::new();
    let body: Box<dyn Read> = match compression {
        JsonlCompression::None => {
            let mut reader = BufReader::new(file);
            reader
                .by_ref()
                .take(MAX_AUTHORITY_HEADER_BYTES + 1)
                .read_until(b'\n', &mut header)
                .map_err(|e| e.to_string())?;
            Box::new(reader)
        }
        JsonlCompression::Zstd => {
            let mut decoder = zstd::stream::read::Decoder::new(file)
                .map_err(|e| e.to_string())?
                .single_frame();
            decoder
                .window_log_max(MAX_AUTHORITY_WINDOW_LOG)
                .map_err(|e| e.to_string())?;
            decoder
                .by_ref()
                .take(MAX_AUTHORITY_HEADER_BYTES + 1)
                .read_to_end(&mut header)
                .map_err(|e| e.to_string())?;
            let reader = decoder.finish();
            Box::new(zstd::stream::read::Decoder::with_buffer(reader).map_err(|e| e.to_string())?)
        }
    };
    if header.len() as u64 > MAX_AUTHORITY_HEADER_BYTES
        || header.last() != Some(&b'\n')
        || header[..header.len() - 1].contains(&b'\n')
    {
        return Err("invalid projection source header".into());
    }
    let raw: serde_json::Value = serde_json::from_slice(&header).map_err(|e| e.to_string())?;
    if !matches!(raw["version"].as_u64(), Some(3 | 4)) {
        return Err("projection source requires historical framing".into());
    }
    let reader = CheckedLines {
        reader: body,
        cancelled,
        depth: 0,
        quoted: false,
        escaped: false,
        started: false,
        closed: false,
    };
    let reader = BufReader::with_capacity(32 * 1024, reader);
    crate::packed_stream::visit_frame_from(reader, 0, visitor)
        .map_err(|e| format!("projection stream requires recovery: {e:?}"))?;
    if before != file_revision(&std::fs::metadata(path).map_err(|e| e.to_string())?) {
        return Err("projection source changed while reading".into());
    }
    Ok(())
}

impl JsonlSessionPersistence {
    pub(super) async fn projection_stream(
        &self,
        id: &SessionId,
        cancelled: Arc<AtomicBool>,
        visitor: Arc<dyn for<'a> Fn(&'a SessionEvent) -> Result<(), String> + Send + Sync>,
    ) -> Result<bool, String> {
        self.ensure_root_encoding().await?;
        let path = self
            .find_log(id)
            .await?
            .ok_or("Session projection source not found")?;
        let path = self.upgrade_v0(&path, id).await?;
        if crate::native_reader::is_native(&path)? {
            let reading = path.clone(); let expected = id.clone();
            return tokio::task::spawn_blocking(move || {
                let check = || cancelled.load(Ordering::Acquire);
                let preflight = match crate::native_reader::visit(&reading, expected.as_str(), &check, |_| Ok(())) {
                    Ok(summary) => summary,
                    Err(error) if check() => return Err(error),
                    Err(_) => return Ok(false),
                };
                if preflight.recovered_tail { return Ok(false); }
                crate::native_reader::visit(&reading, expected.as_str(), &check, |event| visitor(&event))?;
                Ok(true)
            }).await.map_err(|error| error.to_string())?;
        }
        let id = id.clone();
        let compression = crate::format::compression_of(&path);
        tokio::task::spawn_blocking(move || {
            let check_cancel = || {
                if cancelled.load(Ordering::Acquire) {
                    Err("projection replay cancelled".to_string())
                } else {
                    Ok(())
                }
            };
            check_cancel()?;
            if compression == JsonlCompression::Zstd {
                let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                if !complete_zstd_layout(&mut file, &cancelled)? {
                    return Ok(false);
                }
            }
            // Preflight preserves the historical recoverable-prefix behavior
            // for malformed complete JSONL tails: no streamed values have been
            // emitted when the existing recovery reader is selected instead.
            let mut normalizer = StoredEventNormalizer::new(id.clone());
            let mut next = 0;
            let checked = visit_native(&path, compression, cancelled.clone(), &mut |event| {
                check_cancel()?;
                if event.seq.get() != next {
                    return Err("non-contiguous projection source".into());
                }
                next += 1;
                normalizer.normalize(&event)?;
                Ok(true)
            });
            check_cancel()?;
            if checked.is_err() {
                return Ok(false);
            }
            drop(normalizer);
            let mut normalizer = StoredEventNormalizer::new(id);
            let mut next = 0;
            visit_native(&path, compression, cancelled.clone(), &mut |event| {
                check_cancel()?;
                if event.seq.get() != next {
                    return Err("projection source changed during replay".into());
                }
                next += 1;
                let event = normalizer.normalize(&event)?;
                visitor(&event)?;
                Ok(true)
            })?;
            check_cancel()?;
            Ok(true)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
