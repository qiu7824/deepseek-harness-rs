//! ZIP metadata and actual expansion budgets applied before Office processing.
use crate::Result;
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const MAX_METADATA: usize = 8 * 1024 * 1024;
pub(super) const MAX_EXPANDED: u64 = 64 * 1024 * 1024;

pub(super) struct MetadataReader<R> {
    input: R,
    budget: Arc<AtomicUsize>,
}
impl<R: Read> Read for MetadataReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let remaining = self.budget.load(Ordering::Relaxed);
        if remaining == 0 {
            return Err(io::Error::other("Office ZIP metadata budget exceeded"));
        }
        let capacity = bytes.len().min(remaining);
        let count = self.input.read(&mut bytes[..capacity])?;
        if remaining != usize::MAX {
            self.budget.fetch_sub(count, Ordering::Relaxed);
        }
        Ok(count)
    }
}
impl<R: Seek> Seek for MetadataReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.input.seek(position)
    }
}

fn entry_count<R: Read + Seek>(input: &mut R) -> Result<()> {
    let length = input.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let start = length.saturating_sub(22 + u16::MAX as u64);
    input
        .seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut tail = vec![0; (length - start) as usize];
    input.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let at = tail
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(at, magic)| {
            if magic != b"PK\x05\x06" || at + 22 > tail.len() {
                return None;
            }
            let comment = u16::from_le_bytes(tail[at + 20..at + 22].try_into().unwrap()) as usize;
            (at + 22 + comment == tail.len()).then_some(at)
        })
        .ok_or("Invalid Office ZIP end record")?;
    let eocd = &tail[at..at + 22];
    let mut count = u16::from_le_bytes(eocd[10..12].try_into().unwrap()) as u64;
    let on_disk = u16::from_le_bytes(eocd[8..10].try_into().unwrap()) as u64;
    if eocd[4..8] != [0, 0, 0, 0] || count != on_disk {
        return Err("Invalid or split Office ZIP directory".into());
    }
    let position = start + at as u64;
    let mut locator = [0; 20];
    if position >= 20 {
        input
            .seek(SeekFrom::Start(position - 20))
            .map_err(|e| e.to_string())?;
        input.read_exact(&mut locator).map_err(|e| e.to_string())?;
    }
    if &locator[..4] == b"PK\x06\x07" {
        let offset = u64::from_le_bytes(locator[8..16].try_into().unwrap());
        if offset.saturating_add(56) > position - 20 {
            return Err("Invalid Office ZIP64 end record".into());
        }
        input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut end = [0; 56];
        input.read_exact(&mut end).map_err(|e| e.to_string())?;
        if &end[..4] != b"PK\x06\x06" {
            return Err("Invalid Office ZIP64 end record".into());
        }
        count = u64::from_le_bytes(end[32..40].try_into().unwrap());
        if end[16..24] != [0; 8] || u64::from_le_bytes(end[24..32].try_into().unwrap()) != count {
            return Err("Invalid or split Office ZIP64 directory".into());
        }
    } else if count == u16::MAX as u64 {
        return Err("Invalid Office ZIP64 locator".into());
    }
    if count > 10_000 {
        return Err("Office entry budget exceeded".into());
    }
    input.rewind().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    fn zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn entry_counts_are_checked_before_zip_metadata_allocation() {
        let mut bytes = zip();
        let end = bytes.len() - 22;
        bytes[end + 8..end + 10].copy_from_slice(&10001u16.to_le_bytes());
        bytes[end + 10..end + 12].copy_from_slice(&10001u16.to_le_bytes());
        assert!(
            entry_count(&mut Cursor::new(bytes))
                .unwrap_err()
                .contains("entry budget")
        );
        let bytes = zip();
        let split = bytes.len() - 22;
        let mut crafted = bytes[..split].to_vec();
        let offset = crafted.len() as u64;
        let mut record = [0u8; 56];
        record[..4].copy_from_slice(b"PK\x06\x06");
        record[4..12].copy_from_slice(&44u64.to_le_bytes());
        record[24..32].copy_from_slice(&10001u64.to_le_bytes());
        record[32..40].copy_from_slice(&10001u64.to_le_bytes());
        crafted.extend_from_slice(&record);
        let mut locator = [0u8; 20];
        locator[..4].copy_from_slice(b"PK\x06\x07");
        locator[8..16].copy_from_slice(&offset.to_le_bytes());
        crafted.extend_from_slice(&locator);
        crafted.extend_from_slice(&bytes[split..]);
        // ZIP64 must be checked even if the legacy footer has a small count.
        assert!(
            entry_count(&mut Cursor::new(crafted))
                .unwrap_err()
                .contains("entry budget")
        );
    }
    #[test]
    fn forged_uncompressed_size_never_authorizes_actual_expansion() {
        let mut bytes = zip();
        let local = bytes.windows(4).position(|b| b == b"PK\x03\x04").unwrap();
        let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        bytes[local + 22..local + 26].copy_from_slice(&1u32.to_le_bytes());
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            super::super::validate_office_for_automation(&bytes, "docx")
                .unwrap_err()
                .contains("size differs")
        );
        let mut total = MAX_EXPANDED - 1;
        let mut entry = Expanded {
            input: Cursor::new(b"more"),
            total: &mut total,
            count: 0,
            expected: 4,
        };
        let mut buffer = [0; 100];
        assert!(
            entry
                .read(&mut buffer)
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
        assert_eq!(
            entry.count, 2,
            "read at most one byte beyond remaining aggregate budget"
        );
    }

    #[test]
    fn ordinary_and_small_zip64_packages_still_open() {
        let bytes = zip();
        assert_eq!(open(Cursor::new(&bytes)).unwrap().len(), 1);
        let split = bytes.len() - 22;
        let central = bytes
            .windows(4)
            .position(|part| part == b"PK\x01\x02")
            .unwrap();
        let mut extended = bytes[..split].to_vec();
        let offset = extended.len() as u64;
        let mut record = [0; 56];
        record[..4].copy_from_slice(b"PK\x06\x06");
        record[4..12].copy_from_slice(&44u64.to_le_bytes());
        record[12..14].copy_from_slice(&45u16.to_le_bytes());
        record[14..16].copy_from_slice(&45u16.to_le_bytes());
        record[24..32].copy_from_slice(&1u64.to_le_bytes());
        record[32..40].copy_from_slice(&1u64.to_le_bytes());
        record[40..48].copy_from_slice(&((split - central) as u64).to_le_bytes());
        record[48..56].copy_from_slice(&(central as u64).to_le_bytes());
        extended.extend_from_slice(&record);
        let mut locator = [0; 20];
        locator[..4].copy_from_slice(b"PK\x06\x07");
        locator[8..16].copy_from_slice(&offset.to_le_bytes());
        locator[16..20].copy_from_slice(&1u32.to_le_bytes());
        extended.extend_from_slice(&locator);
        extended.extend_from_slice(&bytes[split..]);
        let mut archive = open(Cursor::new(extended)).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert_eq!(xml, "<document/>");
    }
}

pub(super) fn open<R: Read + Seek>(mut input: R) -> Result<zip::ZipArchive<MetadataReader<R>>> {
    entry_count(&mut input)?;
    let budget = Arc::new(AtomicUsize::new(MAX_METADATA));
    let archive = zip::ZipArchive::new(MetadataReader {
        input,
        budget: budget.clone(),
    })
    .map_err(|e| e.to_string())?;
    budget.store(usize::MAX, Ordering::Relaxed);
    if archive.len() > 10_000 {
        return Err("Office entry budget exceeded".into());
    }
    Ok(archive)
}

pub(super) struct Expanded<'a, R> {
    pub input: R,
    pub total: &'a mut u64,
    pub count: u64,
    pub expected: u64,
}
impl<R: Read> Read for Expanded<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let allowed = MAX_EXPANDED
            .saturating_sub(*self.total)
            .saturating_add(1)
            .min(bytes.len() as u64) as usize;
        let count = self.input.read(&mut bytes[..allowed])?;
        self.count = self.count.saturating_add(count as u64);
        *self.total = self.total.saturating_add(count as u64);
        if *self.total > MAX_EXPANDED {
            return Err(io::Error::other("Office expanded input exceeds 64 MiB"));
        }
        if self.count > self.expected || (count == 0 && self.count != self.expected) {
            return Err(io::Error::other(
                "Office ZIP entry size differs from its metadata",
            ));
        }
        Ok(count)
    }
}
