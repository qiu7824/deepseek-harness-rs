//! Lossless, indexed cold event storage. The file is private to this runtime
//! and is removed when its final open handle closes; the durable log remains
//! the source of truth.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::{SessionEvent, SessionSeq};

static NEXT_ARCHIVE: AtomicU64 = AtomicU64::new(0);

/// A contiguous event range whose original payloads and seqs are read on demand.
/// Only one eight-byte file offset per event is retained in memory.
pub struct EventArchive {
    file: Mutex<File>,
    offsets: Vec<u64>,
    first_seq: SessionSeq,
}

/// Incrementally builds an archive without collecting event payloads.
pub struct EventArchiveBuilder {
    writer: BufWriter<File>,
    offsets: Vec<u64>,
    first_seq: SessionSeq,
    failed: bool,
}

/// A range owns its logical cursor, so callbacks may make other indexed
/// reads without changing a sequential reader's position or holding its
/// file mutex through arbitrary visitor code.
struct ArchiveRange<'a> {
    archive: &'a EventArchive,
    cancelled: &'a dyn Fn() -> bool,
    position: u64,
    end: u64,
}

impl Read for ArchiveRange<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::Error::other("Session archive read cancelled"));
        }
        let length = (self.end - self.position).min(buffer.len() as u64) as usize;
        if length == 0 {
            return Ok(0);
        }
        let mut file = self.archive.file.lock();
        file.seek(SeekFrom::Start(self.position))?;
        let count = file.read(&mut buffer[..length])?;
        self.position += count as u64;
        Ok(count)
    }
}

fn sequence_at(first: SessionSeq, index: usize) -> Result<SessionSeq, String> {
    let offset = u64::try_from(index).map_err(|_| "Session archive index overflow")?;
    let seq = first
        .get()
        .checked_add(offset)
        .ok_or("Session archive sequence overflow")?;
    SessionSeq::new(seq)
}

fn temporary_file(directory: &Path) -> Result<File, String> {
    for _ in 0..32 {
        let ordinal = NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = directory.join(format!(
            "dsh-events-{}-{time:x}-{ordinal:x}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_DELETE_ON_CLOSE: no orphan containing event bodies
            // survives normal shutdown or process termination.
            options.custom_flags(0x0400_0000).share_mode(0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                #[cfg(unix)]
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create Session event archive: {error}")),
        }
    }
    Err("cannot allocate a unique Session event archive".into())
}

impl EventArchiveBuilder {
    pub fn new(directory: &Path) -> Result<Self, String> {
        Self::new_at_seq(directory, SessionSeq::ZERO)
    }

    /// Archive a slice without renumbering events or their source references.
    pub fn new_at_seq(directory: &Path, first_seq: SessionSeq) -> Result<Self, String> {
        Ok(Self {
            writer: BufWriter::with_capacity(64 * 1024, temporary_file(directory)?),
            offsets: vec![0],
            first_seq,
            failed: false,
        })
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, event: &SessionEvent) -> Result<(), String> {
        self.push_cancellable(event, &|| false)
    }

    /// Stage a complete event while checking cancellation between bounded writes.
    pub fn push_cancellable(
        &mut self,
        event: &SessionEvent,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), String> {
        if self.failed {
            return Err("Session archive write already failed".into());
        }
        let expected = sequence_at(self.first_seq, self.len())?;
        if event.seq != expected {
            return Err(format!(
                "Session archive expected seq {}, got {}",
                expected, event.seq
            ));
        }
        // Stream serialization through a byte counter. Large result bodies
        // never acquire a second complete serialized allocation.
        struct Counter<'a> {
            writer: &'a mut BufWriter<File>,
            cancelled: &'a dyn Fn() -> bool,
            bytes: u64,
        }
        impl Write for Counter<'_> {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                if (self.cancelled)() {
                    return Err(std::io::Error::other("Session archive write cancelled"));
                }
                let written = self.writer.write(&buffer[..buffer.len().min(64 * 1024)])?;
                self.bytes += written as u64;
                Ok(written)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                if (self.cancelled)() {
                    return Err(std::io::Error::other("Session archive write cancelled"));
                }
                self.writer.flush()
            }
        }
        let mut output = Counter {
            writer: &mut self.writer,
            cancelled,
            bytes: 0,
        };
        let result = serde_json::to_writer(&mut output, event)
            .map_err(|error| error.to_string())
            .and_then(|()| output.write_all(b"\n").map_err(|error| error.to_string()));
        let written = output.bytes;
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        let next = self
            .offsets
            .last()
            .unwrap()
            .checked_add(written)
            .ok_or_else(|| {
                self.failed = true;
                "Session archive byte offset overflow".to_string()
            })?;
        self.offsets.push(next);
        Ok(())
    }

    pub fn finish(mut self) -> Result<EventArchive, String> {
        if self.failed {
            return Err(
                "Session archive write failed; incomplete events cannot be published".into(),
            );
        }
        self.writer.flush().map_err(|error| error.to_string())?;
        let file = self
            .writer
            .into_inner()
            .map_err(|error| error.to_string())?;
        self.offsets.shrink_to_fit();
        Ok(EventArchive {
            file: Mutex::new(file),
            offsets: self.offsets,
            first_seq: self.first_seq,
        })
    }
}

impl EventArchive {
    pub fn first_seq(&self) -> SessionSeq {
        self.first_seq
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read(&self, index: usize) -> Result<Option<SessionEvent>, String> {
        if index >= self.len() {
            return Ok(None);
        }
        let mut file = self.file.lock();
        file.seek(SeekFrom::Start(self.offsets[index]))
            .map_err(|error| error.to_string())?;
        let length = self.offsets[index + 1] - self.offsets[index];
        let reader = BufReader::new((&mut *file).take(length));
        let event: SessionEvent =
            serde_json::from_reader(reader).map_err(|error| error.to_string())?;
        if event.seq != sequence_at(self.first_seq, index)? {
            return Err("Session archive sequence mismatch".into());
        }
        Ok(Some(event))
    }

    /// Visit an exact half-open index range, stopping when the callback
    /// returns false. Indexed reads made by the callback have independent
    /// cursors and cannot disturb this traversal.
    pub fn visit(
        &self,
        range: Range<usize>,
        mut visitor: impl FnMut(&SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        self.visit_owned(range, |event| visitor(&event))
    }

    /// Transfer each temporary decoded event to its consumer without changing
    /// the immutable archive or retaining decoded bodies between visits.
    pub fn visit_owned(
        &self,
        range: Range<usize>,
        visitor: impl FnMut(SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        self.visit_owned_cancellable(range, &|| false, visitor)
    }

    /// Check cancellation at event boundaries and bounded file-buffer refills.
    pub fn visit_owned_cancellable(
        &self,
        range: Range<usize>,
        cancelled: &impl Fn() -> bool,
        mut visitor: impl FnMut(SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        if range.start > range.end || range.end > self.len() {
            return Err("Session archive range exceeds its captured prefix".into());
        }
        if cancelled() {
            return Err("Session archive read cancelled".into());
        }
        if range.is_empty() {
            return Ok(());
        }
        let mut reader = BufReader::with_capacity(
            64 * 1024,
            ArchiveRange {
                archive: self,
                cancelled,
                position: self.offsets[range.start],
                end: self.offsets[range.end],
            },
        );
        for index in range {
            if cancelled() {
                return Err("Session archive read cancelled".into());
            }
            // Bound each deserializer to one indexed record. Its string
            // scratch is released before the visitor retains the event;
            // end-of-value validation cannot consume the next record.
            let length = self.offsets[index + 1] - self.offsets[index];
            let mut record = (&mut reader).take(length);
            let event: SessionEvent =
                serde_json::from_reader(&mut record).map_err(|error| error.to_string())?;
            if record.limit() != 0 {
                return Err("Session archive ended before its captured prefix".into());
            }
            if event.seq != sequence_at(self.first_seq, index)? {
                return Err("Session archive sequence mismatch".into());
            }
            let keep_going = visitor(event)?;
            if cancelled() {
                return Err("Session archive read cancelled".into());
            }
            if !keep_going {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurfaceOp;

    fn event(seq: u64) -> SessionEvent {
        SessionEvent {
            type_: "assistant/message".into(),
            seq: SessionSeq::new(seq).unwrap(),
            time: -12,
            data: serde_json::json!({"unknown": [null, true, {"text":"\"\\\n历史"}]}),
            surface_op: Some(SurfaceOp::Append),
            source_event_seqs: Some((0..seq).collect()),
            ignorable: Some(true),
        }
    }

    fn archive_records(records: &[Vec<u8>]) -> EventArchive {
        let mut builder = EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for bytes in records {
            builder.writer.write_all(bytes).unwrap();
            builder
                .offsets
                .push(builder.offsets.last().unwrap() + bytes.len() as u64);
        }
        builder.finish().unwrap()
    }

    #[test]
    fn record_scoped_decoders_preserve_escaped_payloads_whitespace_and_buffered_successors() {
        let seed: Vec<_> = (0..4)
            .map(|seq| {
                let mut value = event(seq);
                if seq % 2 == 0 {
                    value.data["escaped"] = "\"\\\n\r\t历史🙂".repeat(20_000).into();
                }
                value
            })
            .collect();
        let records: Vec<_> = seed
            .iter()
            .map(|value| {
                let mut bytes = b" \t\r\n".to_vec();
                serde_json::to_writer(&mut bytes, value).unwrap();
                bytes.extend_from_slice(b" \n\t\r");
                bytes
            })
            .collect();
        assert!(records[0].len() > 64 * 1024);
        let archive = archive_records(&records);
        let mut output = Vec::new();
        archive
            .visit_owned(0..4, |value| {
                assert_eq!(archive.read(0)?, Some(seed[0].clone()));
                output.push(value);
                Ok(true)
            })
            .unwrap();
        assert_eq!(output, seed);
        let mut partial = Vec::new();
        archive
            .visit_owned(1..4, |value| {
                partial.push(value);
                Ok(true)
            })
            .unwrap();
        assert_eq!(partial, seed[1..4]);
    }

    #[test]
    fn record_scoped_decoders_reject_extra_json_and_truncation_before_delivery() {
        let first = serde_json::to_vec(&event(0)).unwrap();
        let mut extra = first.clone();
        extra.extend_from_slice(b" {}\n");
        let mut truncated = first.clone();
        truncated.pop();
        truncated.push(b'\n');
        for damaged in [extra, truncated] {
            let archive = archive_records(&[damaged, serde_json::to_vec(&event(1)).unwrap()]);
            assert!(
                archive
                    .visit_owned(0..2, |_| panic!("invalid record reached visitor"))
                    .is_err()
            );
        }
        let mut complete = first;
        complete.push(b'\n');
        let archive = archive_records(&[complete]);
        archive.file.lock().set_len(archive.offsets[1] - 1).unwrap();
        assert_eq!(
            archive.visit_owned(0..1, |_| panic!(
                "truncated captured record reached visitor"
            )),
            Err("Session archive ended before its captured prefix".into())
        );
    }

    #[test]
    fn archive_round_trips_all_fields_and_supports_exact_bounded_ranges() {
        let mut builder = EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for seq in 0..9 {
            builder.push(&event(seq)).unwrap();
        }
        assert!(builder.push(&event(10)).is_err());
        let archive = builder.finish().unwrap();
        assert_eq!(archive.len(), 9);
        assert_eq!(archive.read(7).unwrap(), Some(event(7)));
        assert_eq!(archive.read(9).unwrap(), None);
        let mut visited = Vec::new();
        archive
            .visit(2..8, |value| {
                assert_eq!(archive.read(8).unwrap(), Some(event(8)));
                visited.push(value.clone());
                Ok(value.seq.get() < 4)
            })
            .unwrap();
        assert_eq!(visited, (2..5).map(event).collect::<Vec<_>>());
        let mut repeated = Vec::new();
        archive
            .visit(7..9, |value| {
                repeated.push(value.clone());
                Ok(true)
            })
            .unwrap();
        assert_eq!(repeated, vec![event(7), event(8)]);
        assert!(archive.visit(8..10, |_| Ok(true)).is_err());
    }

    #[test]
    fn archive_slices_preserve_original_sequences_and_references() {
        let first = SessionSeq::new(37).unwrap();
        let seed: Vec<_> = (37..41).map(event).collect();
        let mut builder = EventArchiveBuilder::new_at_seq(&std::env::temp_dir(), first).unwrap();
        assert!(builder.push(&event(0)).is_err());
        for event in &seed {
            builder.push(event).unwrap();
        }
        let archive = builder.finish().unwrap();
        assert_eq!(archive.first_seq(), first);
        assert_eq!(archive.len(), 4);
        assert_eq!(archive.read(0).unwrap(), Some(seed[0].clone()));
        assert_eq!(archive.read(3).unwrap(), Some(seed[3].clone()));
        assert_eq!(archive.read(4).unwrap(), None);
        let mut output = Vec::new();
        archive
            .visit_owned(1..3, |event| {
                assert_eq!(archive.read(0)?, Some(seed[0].clone()));
                output.push(event);
                Ok(true)
            })
            .unwrap();
        assert_eq!(output, seed[1..3]);
        let mut all = Vec::new();
        archive
            .visit(0..4, |event| {
                all.push(event.clone());
                Ok(true)
            })
            .unwrap();
        assert_eq!(all, seed);
    }

    #[test]
    fn archive_slice_sequences_reject_safe_integer_and_addition_overflow() {
        let largest = SessionSeq::new(9_007_199_254_740_991).unwrap();
        assert!(sequence_at(largest, 1).is_err());
        assert!(sequence_at(largest, usize::MAX).is_err());
        let mut last = event(0);
        last.seq = largest;
        let mut builder = EventArchiveBuilder::new_at_seq(&std::env::temp_dir(), largest).unwrap();
        builder.push(&last).unwrap();
        assert!(builder.push(&last).is_err());
        let archive = builder.finish().unwrap();
        assert_eq!(archive.read(0).unwrap(), Some(last));
        archive
            .visit_owned(0..1, |event| {
                assert_eq!(event.seq, largest);
                Ok(true)
            })
            .unwrap();
    }

    #[test]
    fn cancelled_archive_reads_stop_inside_large_events_and_cleanup_on_drop() {
        let directory = std::env::temp_dir().join(format!(
            "dsh-archive-cancel-{}-{}",
            std::process::id(),
            NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut large = event(0);
        large.data["large"] = "x".repeat(256 * 1024).into();
        let mut builder = EventArchiveBuilder::new(&directory).unwrap();
        builder.push(&large).unwrap();
        let archive = builder.finish().unwrap();
        let checks = std::cell::Cell::new(0);
        let error = archive
            .visit_owned_cancellable(
                0..1,
                &|| {
                    let count = checks.get() + 1;
                    checks.set(count);
                    count >= 5
                },
                |_| panic!("cancelled large event reached its visitor"),
            )
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert_eq!(archive.read(0).unwrap(), Some(large));
        assert!(
            archive
                .visit_owned_cancellable(0..0, &|| true, |_| Ok(true))
                .is_err()
        );
        drop(archive);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        {
            let mut builder = EventArchiveBuilder::new(&directory).unwrap();
            builder.push(&event(0)).unwrap();
            assert!(builder.push(&event(3)).is_err());
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn cancelled_archive_writes_cannot_publish_a_partial_event() {
        let directory = std::env::temp_dir().join(format!(
            "dsh-archive-write-cancel-{}-{}",
            std::process::id(),
            NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut large = event(0);
        large.data["large"] = "x".repeat(256 * 1024).into();
        let mut builder = EventArchiveBuilder::new(&directory).unwrap();
        let handle = builder.writer.get_ref().try_clone().unwrap();
        let error = builder
            .push_cancellable(&large, &|| handle.metadata().unwrap().len() >= 64 * 1024)
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert_eq!(builder.len(), 0);
        assert!(builder.push(&large).is_err());
        assert!(
            builder
                .finish()
                .err()
                .unwrap()
                .contains("incomplete events")
        );
        assert!((64 * 1024..256 * 1024).contains(&handle.metadata().unwrap().len()));
        drop(handle);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn owned_visits_keep_independent_cursors_and_leave_source_events_unchanged() {
        let seed: Vec<_> = (0..4)
            .map(|seq| {
                let mut event = event(seq);
                event.data["large"] = "x".repeat(96 * 1024).into();
                event
            })
            .collect();
        let mut builder = EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for event in &seed {
            builder.push(event).unwrap();
        }
        let archive = builder.finish().unwrap();
        let mut seen = Vec::new();
        archive
            .visit_owned(0..4, |mut value| {
                let index = value.seq.get() as usize;
                assert_eq!(value, seed[index]);
                archive.visit_owned(3..4, |nested| {
                    assert_eq!(nested, seed[3]);
                    Ok(true)
                })?;
                assert_eq!(archive.read(0)?, Some(seed[0].clone()));
                value.data["large"] = "local edit".into();
                assert_eq!(archive.read(index)?, Some(seed[index].clone()));
                seen.push(index);
                Ok(index < 2)
            })
            .unwrap();
        assert_eq!(seen, vec![0, 1, 2]);
        let mut repeated = Vec::new();
        archive
            .visit_owned(0..4, |event| {
                repeated.push(event);
                Ok(true)
            })
            .unwrap();
        assert_eq!(repeated, seed);
        assert!(
            archive
                .visit_owned(Range { start: 2, end: 1 }, |_| Ok(true))
                .is_err()
        );
        assert!(archive.visit_owned(0..5, |_| Ok(true)).is_err());
        assert_eq!(archive.visit_owned(4..4, |_| panic!("empty range")), Ok(()));
        assert_eq!(
            archive.visit_owned(0..4, |_| Err("visitor failed".into())),
            Err("visitor failed".into())
        );
    }

    #[test]
    fn owned_visits_reject_corrupted_archive_sequences() {
        let mut builder = EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        builder.push(&event(0)).unwrap();
        let archive = builder.finish().unwrap();
        {
            let mut file = archive.file.lock();
            let mut corrupted = event(0);
            corrupted.seq = SessionSeq::new(1).unwrap();
            file.seek(SeekFrom::Start(0)).unwrap();
            serde_json::to_writer(&mut *file, &corrupted).unwrap();
            file.write_all(b"\n").unwrap();
        }
        assert_eq!(
            archive.visit_owned(0..1, |_| panic!("invalid event reached visitor")),
            Err("Session archive sequence mismatch".into())
        );
        assert!(archive.visit(0..1, |_| Ok(true)).is_err());
    }

    #[test]
    fn archive_removes_its_private_file_when_the_last_handle_closes() {
        let path = std::env::temp_dir().join(format!(
            "dsh-archive-cleanup-{}-{}",
            std::process::id(),
            NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let mut builder = EventArchiveBuilder::new(&path).unwrap();
        builder.push(&event(0)).unwrap();
        let archive = builder.finish().unwrap();
        assert_eq!(archive.read(0).unwrap(), Some(event(0)));
        #[cfg(windows)]
        {
            let entries = std::fs::read_dir(&path)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(entries.len(), 1);
            let private = entries[0].path();
            assert!(
                std::fs::read(&private).is_err(),
                "a separate handle must not read private event bodies"
            );
            assert!(
                OpenOptions::new().write(true).open(&private).is_err(),
                "a separate handle must not modify private event bodies"
            );
        }
        drop(archive);
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        std::fs::remove_dir(&path).unwrap();
    }
}
