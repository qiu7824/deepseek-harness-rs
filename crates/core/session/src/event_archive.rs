//! Lossless, indexed cold event storage. The file is private to this runtime
//! and is removed when its final open handle closes; the durable log remains
//! the source of truth.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::SessionEvent;

static NEXT_ARCHIVE: AtomicU64 = AtomicU64::new(0);

/// A complete contiguous event prefix whose payloads are read on demand.
/// Only one eight-byte file offset per event is retained in memory.
pub struct EventArchive {
    file: Mutex<File>,
    offsets: Vec<u64>,
}

/// Incrementally builds an archive without collecting event payloads.
pub struct EventArchiveBuilder {
    writer: BufWriter<File>,
    offsets: Vec<u64>,
}

/// A range owns its logical cursor, so callbacks may make other indexed
/// reads without changing a sequential reader's position or holding its
/// file mutex through arbitrary visitor code.
struct ArchiveRange<'a> {
    archive: &'a EventArchive,
    position: u64,
    end: u64,
}

impl Read for ArchiveRange<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
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
        Ok(Self {
            writer: BufWriter::with_capacity(64 * 1024, temporary_file(directory)?),
            offsets: vec![0],
        })
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, event: &SessionEvent) -> Result<(), String> {
        if event.seq.get() != self.len() as u64 {
            return Err(format!(
                "Session archive expected seq {}, got {}",
                self.len(),
                event.seq
            ));
        }
        // Stream serialization through a byte counter. Large result bodies
        // never acquire a second complete serialized allocation.
        struct Counter<'a> {
            writer: &'a mut BufWriter<File>,
            bytes: u64,
        }
        impl Write for Counter<'_> {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                let written = self.writer.write(buffer)?;
                self.bytes += written as u64;
                Ok(written)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.writer.flush()
            }
        }
        let mut output = Counter {
            writer: &mut self.writer,
            bytes: 0,
        };
        serde_json::to_writer(&mut output, event).map_err(|error| error.to_string())?;
        output.write_all(b"\n").map_err(|error| error.to_string())?;
        let next = self.offsets.last().unwrap() + output.bytes;
        self.offsets.push(next);
        Ok(())
    }

    pub fn finish(mut self) -> Result<EventArchive, String> {
        self.writer.flush().map_err(|error| error.to_string())?;
        let file = self
            .writer
            .into_inner()
            .map_err(|error| error.to_string())?;
        self.offsets.shrink_to_fit();
        Ok(EventArchive {
            file: Mutex::new(file),
            offsets: self.offsets,
        })
    }
}

impl EventArchive {
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
        if event.seq.get() != index as u64 {
            return Err("Session archive sequence mismatch".into());
        }
        Ok(Some(event))
    }

    /// Visit an exact half-open prefix range, stopping when the callback
    /// returns false. Indexed reads made by the callback have independent
    /// cursors and cannot disturb this traversal.
    pub fn visit(
        &self,
        range: Range<usize>,
        mut visitor: impl FnMut(&SessionEvent) -> Result<bool, String>,
    ) -> Result<(), String> {
        if range.start > range.end || range.end > self.len() {
            return Err("Session archive range exceeds its captured prefix".into());
        }
        if range.is_empty() {
            return Ok(());
        }
        let reader = BufReader::with_capacity(
            64 * 1024,
            ArchiveRange {
                archive: self,
                position: self.offsets[range.start],
                end: self.offsets[range.end],
            },
        );
        let mut events = serde_json::Deserializer::from_reader(reader).into_iter::<SessionEvent>();
        for index in range {
            let event = events
                .next()
                .ok_or("Session archive ended before its captured prefix")?
                .map_err(|error| error.to_string())?;
            if event.seq.get() != index as u64 {
                return Err("Session archive sequence mismatch".into());
            }
            if !visitor(&event)? {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionSeq, SurfaceOp};

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
