use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::Stream;

use super::{SessionLogCompressionLevel, SessionLogZipEntry};
use crate::fetch::handler::AbortSignal;

const CHUNK_BYTES: usize = 64 * 1024;
const CAPACITY: usize = 4;
static NEXT_EXPORT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn stream_session_log_zip(
    mut entries: tokio::sync::mpsc::Receiver<Result<SessionLogZipEntry, String>>,
    compression_level: SessionLogCompressionLevel,
    signal: AbortSignal,
) -> Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>> {
    let sequence = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "dsh-session-export-{}-{sequence}.zip",
        std::process::id()
    ));
    let done = Arc::new(AtomicBool::new(false));
    let writer_done = done.clone();
    let writer_path = path.clone();
    let writer_signal = signal.clone();
    tokio::task::spawn_blocking(move || {
        let result: Result<(), String> = (|| {
            let file = std::fs::File::create(&writer_path).map_err(|error| error.to_string())?;
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(i64::from(compression_level)));
            loop {
                if writer_signal.aborted() {
                    return Err("session log export was cancelled".to_string());
                }
                let entry = match entries.try_recv() {
                    Ok(entry) => entry?,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                };
                if writer_signal.aborted() {
                    return Err("session log export was cancelled".to_string());
                }
                match entry {
                    SessionLogZipEntry::Text { path, content } => {
                        writer
                            .start_file(path, options)
                            .map_err(|error| error.to_string())?;
                        writer
                            .write_all(content.as_bytes())
                            .map_err(|error| error.to_string())?;
                    }
                    SessionLogZipEntry::Data { path, data } => {
                        writer
                            .start_file(path, options)
                            .map_err(|error| error.to_string())?;
                        writer.write_all(&data).map_err(|error| error.to_string())?;
                    }
                }
            }
            writer.finish().map_err(|error| error.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            writer_signal.abort();
        }
        writer_done.store(true, Ordering::Release);
    });

    let (sender, receiver) = tokio::sync::mpsc::channel(CAPACITY);
    tokio::spawn(async move {
        let mut offset = 0_u64;
        loop {
            if signal.aborted() {
                let _ = sender
                    .send(Err("session log export was cancelled or failed".to_string()))
                    .await;
                break;
            }
            let complete = done.load(Ordering::Acquire);
            let read_path = path.clone();
            let read = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
                let mut file = match std::fs::File::open(read_path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Vec::new());
                    }
                    Err(error) => return Err(error),
                };
                std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset))?;
                let mut chunk = vec![0_u8; CHUNK_BYTES];
                let count = file.read(&mut chunk)?;
                chunk.truncate(count);
                Ok(chunk)
            })
            .await;
            let chunk = match read {
                Ok(Ok(chunk)) => chunk,
                Ok(Err(error)) => {
                    let _ = sender
                        .send(Err(format!("session log export read failed: {error}")))
                        .await;
                    break;
                }
                Err(error) => {
                    let _ = sender
                        .send(Err(format!("session log export reader failed: {error}")))
                        .await;
                    break;
                }
            };
            if chunk.is_empty() {
                if complete {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                continue;
            }
            offset += chunk.len() as u64;
            if sender.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
        let _ = tokio::fs::remove_file(path).await;
    });

    Box::pin(futures::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|chunk| (chunk, receiver)) },
    ))
}
