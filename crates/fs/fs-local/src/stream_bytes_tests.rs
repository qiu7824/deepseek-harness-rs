use super::*;
use futures::StreamExt;
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(data: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("dsh-binary-stream-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, data).unwrap();
        Self(path)
    }
    fn target(&self) -> LocalTarget {
        LocalTarget {
            display_path: self.0.to_string_lossy().into_owned(),
            target_key: fs_target_key(self.0.to_string_lossy().into_owned()),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn binary_stream_is_incremental_exact_and_cancelled_between_chunks() {
    let data = (0..192 * 1024 + 7)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<_>>();
    let fixture = Fixture::new(&data);
    let mut stream = stream_whole_bytes(
        &fixture.target(),
        None,
        data.len() as u64,
        &FsIoInternals::default(),
    )
    .await
    .unwrap();
    let mut offset = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= 64 * 1024);
        assert_eq!(chunk, data[offset..offset + chunk.len()]);
        offset += chunk.len();
    }
    assert_eq!(offset, data.len());
    let aborted = Arc::new(AtomicBool::new(false));
    let flag = aborted.clone();
    let signal: FsAbort = Arc::new(move || flag.load(Ordering::SeqCst));
    let mut stream = stream_whole_bytes(
        &fixture.target(),
        Some(&signal),
        data.len() as u64,
        &FsIoInternals::default(),
    )
    .await
    .unwrap();
    assert!(stream.next().await.unwrap().is_ok());
    aborted.store(true, Ordering::SeqCst);
    assert_eq!(
        stream.next().await.unwrap().unwrap_err().code,
        FsErrorCode::FsAborted
    );
}

#[tokio::test]
async fn binary_stream_rejects_oversize_and_growth_without_truncating_success() {
    let fixture = Fixture::new(b"1234");
    assert!(
        stream_whole_bytes(&fixture.target(), None, 3, &FsIoInternals::default())
            .await
            .is_err()
    );
    let mut stream = stream_whole_bytes(&fixture.target(), None, 4, &FsIoInternals::default())
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.0)
        .unwrap()
        .write_all(b"5")
        .unwrap();
    assert_eq!(
        stream.next().await.unwrap().unwrap_err().code,
        FsErrorCode::FsTooLarge
    );
    drop(stream);
    let mut stream = stream_whole_bytes(&fixture.target(), None, 5, &FsIoInternals::default())
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), b"12345");
    drop(stream);
    std::fs::remove_file(&fixture.0).expect("dropping a stream releases the file handle");
}
