use super::{SessionGenerationLease, StableGenerationSource, generation_path, select_generation};
use crate::{
    JsonlCompression,
    v4_artifact::{
        ArtifactPart, V4ArtifactValidation, check_cancel, validate_open_v4, visit_artifact,
    },
};
use dsh_session::{
    StorageRecord,
    format_v4::{
        V3Dialect, V3ToV4Transform, V4Vocabulary, decode_v3_header, encode_v4_event,
        encode_v4_header,
    },
};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

const BATCH_BYTES: usize = 256 * 1024;

struct CancelWriter<'a, W, F> {
    inner: &'a mut W,
    cancelled: &'a F,
}
impl<W: Write, F: Fn() -> bool> Write for CancelWriter<'_, W, F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        check_cancel(self.cancelled).map_err(std::io::Error::other)?;
        self.inner.write(&bytes[..bytes.len().min(32 * 1024)])
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn write_json_line(
    inner: &mut impl Write,
    value: &Value,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut writer = CancelWriter { inner, cancelled };
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())
}

struct CountBytes(u64);
impl Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("JSON size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct ArtifactWriter {
    file: BufWriter<File>,
    compression: JsonlCompression,
    batch: Vec<u8>,
}
impl ArtifactWriter {
    fn record(
        &mut self,
        value: &Value,
        header: bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), String> {
        if self.compression == JsonlCompression::None {
            return write_json_line(&mut self.file, value, cancelled);
        }
        let mut size = CountBytes(0);
        write_json_line(&mut size, value, cancelled)?;
        if size.0 > BATCH_BYTES as u64 || header {
            self.flush_batch()?;
            // Large records are serialized directly into one frame rather than
            // constructing a second full JSON byte buffer.
            let mut encoder =
                zstd::stream::Encoder::new(&mut self.file, 0).map_err(|e| e.to_string())?;
            encoder.include_checksum(true).map_err(|e| e.to_string())?;
            encoder
                .set_pledged_src_size(Some(size.0))
                .map_err(|e| e.to_string())?;
            write_json_line(&mut encoder, value, cancelled)?;
            encoder.finish().map_err(|e| e.to_string())?;
        } else {
            if self.batch.len() + size.0 as usize > BATCH_BYTES {
                self.flush_batch()?;
            }
            write_json_line(&mut self.batch, value, cancelled)?;
        }
        Ok(())
    }
    fn flush_batch(&mut self) -> Result<(), String> {
        if !self.batch.is_empty() {
            let frame = crate::compress_zstd_frame(&self.batch)?;
            self.file.write_all(&frame).map_err(|e| e.to_string())?;
            self.batch.clear();
        }
        Ok(())
    }
    fn finish(mut self) -> Result<(), String> {
        self.flush_batch()?;
        self.file.flush().map_err(|e| e.to_string())?;
        self.file.get_ref().sync_all().map_err(|e| e.to_string())
    }
}

/// Only this operation's private stage is removed on failure. A process crash
/// may leave the stage behind; discovery never treats it as a generation.
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn stage(
    directory: &Path,
    compression: JsonlCompression,
) -> Result<(Stage, ArtifactWriter), String> {
    let path = directory.join(format!(".session-v4-{}.stage", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|e| e.to_string())?;
    Ok((
        Stage(path),
        ArtifactWriter {
            file: BufWriter::new(file),
            compression,
            batch: Vec::new(),
        },
    ))
}

#[derive(Clone, Debug, PartialEq)]
pub struct V4MigrationResult {
    pub path: PathBuf,
    pub published: bool,
    pub source_sha256: Option<String>,
    pub target_sha256: String,
    pub source_events: Option<u64>,
    pub validation: V4ArtifactValidation,
}

fn convert_record(
    line: &[u8],
    transform: &mut V3ToV4Transform,
    writer: &mut ArtifactWriter,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let value: Value =
        serde_json::from_slice(line).map_err(|e| format!("invalid V3 source record: {e}"))?;
    let mut emit = |row| {
        for row in transform.push(row)? {
            writer.record(
                &encode_v4_event(row, &V4Vocabulary::default())?,
                false,
                cancelled,
            )?;
        }
        Ok(())
    };
    match StorageRecord::from_json(value)? {
        StorageRecord::Event(row) => emit(row),
        packed => dsh_session::visit_decoded_storage_record_events(packed, |event| {
            emit(serde_json::to_value(event).map_err(|e| e.to_string())?)?;
            Ok(true)
        })
        .map(|_| ()),
    }
}

/// Convert one complete Rust V3 artifact into a separately named V4 successor.
/// The caller must hold the per-Session writer lease throughout. Child facts
/// must come from a complete storage catalog, including an explicitly empty
/// catalog; they must not be guessed from tool arguments.
///
/// Publication is a no-overwrite hard link after source revalidation and full
/// target validation. No source bytes or older generations are ever replaced.
pub fn migrate_v3_to_v4(
    lease: &SessionGenerationLease,
    expected_id: &str,
    children: Vec<Value>,
    compression: JsonlCompression,
    cancelled: &impl Fn() -> bool,
) -> Result<V4MigrationResult, String> {
    migrate_v3_impl(lease, expected_id, children, compression, None, cancelled)
}

/// Convert a prefix recovered under the caller's writer lease. The original
/// incomplete generation remains the publication authority and is retained.
pub fn migrate_recovered_v3_to_v4(
    lease: &SessionGenerationLease,
    expected_id: &str,
    children: Vec<Value>,
    compression: JsonlCompression,
    events: &[dsh_session::SessionEvent],
    cancelled: &impl Fn() -> bool,
) -> Result<V4MigrationResult, String> {
    migrate_v3_impl(
        lease,
        expected_id,
        children,
        compression,
        Some(events),
        cancelled,
    )
}

fn migrate_v3_impl(
    lease: &SessionGenerationLease,
    expected_id: &str,
    children: Vec<Value>,
    compression: JsonlCompression,
    recovered: Option<&[dsh_session::SessionEvent]>,
    cancelled: &impl Fn() -> bool,
) -> Result<V4MigrationResult, String> {
    check_cancel(cancelled)?;
    let selected = select_generation(lease.directory(), expected_id)?
        .ok_or("Session has no source generation")?;
    let mut source = StableGenerationSource::open(&selected.path, cancelled)?;
    if selected.version == 4 {
        let validation =
            validate_open_v4(source.file(), selected.compression, expected_id, cancelled)?;
        source.assert_unchanged(cancelled)?;
        return Ok(V4MigrationResult {
            path: selected.path,
            published: false,
            source_sha256: None,
            target_sha256: source.sha256(),
            source_events: None,
            validation,
        });
    }
    if selected.version != 3 {
        return Err(format!(
            "V{} requires its adjacent historical migration before V4",
            selected.version
        ));
    }
    let (stage, mut writer) = stage(lease.directory(), compression)?;
    let mut transform = None;
    let mut children = Some(children);
    let mut fragment = Vec::new();
    if let Some(events) = recovered {
        let (header, cut) = decode_v3_header(selected.physical_header.clone(), V3Dialect::Rust)?;
        let mut target = header.clone();
        target["version"] = Value::from(4);
        writer.record(&encode_v4_header(target, 0)?, true, cancelled)?;
        let mut stage = V3ToV4Transform::new(header, children.take(), cut, V3Dialect::Rust)?;
        for event in events {
            check_cancel(cancelled)?;
            for row in
                stage.push(serde_json::to_value(event).map_err(|error| error.to_string())?)?
            {
                writer.record(
                    &encode_v4_event(row, &V4Vocabulary::default())?,
                    false,
                    cancelled,
                )?;
            }
        }
        transform = Some(stage);
    } else {
        visit_artifact(source.file(), selected.compression, cancelled, |part| {
            match part {
                ArtifactPart::Header(line) => {
                    let physical: Value =
                        serde_json::from_slice(line).map_err(|e| e.to_string())?;
                    if physical != selected.physical_header {
                        return Err("Session header changed after generation selection".into());
                    }
                    let (header, cut) = decode_v3_header(physical, V3Dialect::Rust)?;
                    let mut target = header.clone();
                    target["version"] = Value::from(4);
                    writer.record(&encode_v4_header(target, 0)?, true, cancelled)?;
                    transform = Some(V3ToV4Transform::new(
                        header,
                        children.take(),
                        cut,
                        V3Dialect::Rust,
                    )?);
                }
                ArtifactPart::Body(bytes) => {
                    let transform = transform.as_mut().ok_or("missing V3 header")?;
                    let mut start = 0;
                    for (end, byte) in bytes.iter().enumerate() {
                        if *byte != b'\n' {
                            continue;
                        }
                        check_cancel(cancelled)?;
                        if fragment.is_empty() {
                            convert_record(&bytes[start..end], transform, &mut writer, cancelled)?;
                        } else {
                            fragment.extend_from_slice(&bytes[start..end]);
                            convert_record(&fragment, transform, &mut writer, cancelled)?;
                            fragment.clear();
                            if fragment.capacity() > 64 * 1024 {
                                fragment = Vec::new();
                            }
                        }
                        start = end + 1;
                    }
                    fragment.extend_from_slice(&bytes[start..]);
                }
            }
            Ok(())
        })?;
    }
    if !fragment.is_empty() {
        return Err(
            "V3 migration source has an incomplete final JSONL record; original retained".into(),
        );
    }
    let (summary, catalogs) = transform.ok_or("missing V3 source header")?.finish()?;
    for row in catalogs {
        check_cancel(cancelled)?;
        writer.record(
            &encode_v4_event(row, &V4Vocabulary::default())?,
            false,
            cancelled,
        )?;
    }
    writer.finish()?;
    let mut target = StableGenerationSource::open(&stage.0, cancelled)?;
    let validation = validate_open_v4(target.file(), compression, expected_id, cancelled)?;
    if validation.scan.decoded.header != summary.header
        || validation.lifecycle.inherited_event_count != summary.inherited_event_count
        || validation.lifecycle.event_count != summary.event_count
    {
        return Err("V4 target does not match the completed adjacent transform".into());
    }
    target.assert_unchanged(cancelled)?;
    source.assert_unchanged(cancelled)?;
    if select_generation(lease.directory(), expected_id)?.as_ref() != Some(&selected) {
        return Err("Session generation changed before publication; original retained".into());
    }
    let final_path = generation_path(lease.directory(), 4, compression)?;
    check_cancel(cancelled)?;
    std::fs::hard_link(&stage.0, &final_path)
        .map_err(|e| format!("V4 successor was not published; existing files retained: {e}"))?;
    // Publication is the commit point. A later cancellation cannot undo it or
    // report that nothing happened; the next call finds and validates V4.
    let result = V4MigrationResult {
        path: final_path,
        published: true,
        source_sha256: Some(source.sha256()),
        target_sha256: target.sha256(),
        source_events: Some(summary.source_offsets.len() as u64),
        validation,
    };
    drop(target);
    drop(stage);
    #[cfg(unix)]
    File::open(lease.directory())
        .and_then(|directory| directory.sync_all())
        .map_err(|e| {
            format!(
                "V4 successor was published but directory sync failed; inspect before retry: {e}"
            )
        })?;
    Ok(result)
}

#[cfg(test)]
mod tests;
