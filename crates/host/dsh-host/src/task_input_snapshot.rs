//! Immutable acceptance snapshots from the authorized filesystem provider.
use dsh_fs::FsError;
use futures::{StreamExt, stream::BoxStream};
use sha2::{Digest, Sha256};
use std::fs::File;
use tokio::io::AsyncWriteExt;

pub(super) struct Snapshot {
    file: Option<File>,
    pub identity: String,
    pub bytes: u64,
}
impl Snapshot {
    pub fn reader(&self) -> Result<File, String> {
        self.file
            .as_ref()
            .ok_or("Acceptance snapshot was not retained")?
            .try_clone()
            .map_err(|e| e.to_string())
    }
}

pub(super) async fn capture(
    mut stream: BoxStream<'static, Result<Vec<u8>, FsError>>,
    signal: dsh_tools::AbortPredicate,
    retain: bool,
    limit: u64,
) -> Result<Snapshot, String> {
    let mut file = if retain {
        Some(tokio::fs::File::from_std(
            tempfile::tempfile().map_err(|e| e.to_string())?,
        ))
    } else {
        None
    };
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    loop {
        if signal() {
            return Err("Task validation cancelled".into());
        }
        let chunk = loop {
            tokio::select! {
                chunk = stream.next() => break chunk,
                _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    if signal() { return Err("Task validation cancelled".into()); }
                }
            }
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| e.to_string())?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > limit {
            return Err("Task validation input exceeds the file byte limit".into());
        }
        hash.update(&chunk);
        if let Some(file) = &mut file {
            for piece in chunk.chunks(64 * 1024) {
                if signal() {
                    return Err("Task validation cancelled".into());
                }
                file.write_all(piece).await.map_err(|e| e.to_string())?;
            }
        }
    }
    let file = if let Some(mut file) = file {
        file.flush().await.map_err(|e| e.to_string())?;
        Some(file.into_std().await)
    } else {
        None
    };
    Ok(Snapshot {
        file,
        identity: format!("{:x}", hash.finalize()),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Seek},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    #[tokio::test]
    async fn snapshots_and_hash_only_reads_have_identical_evidence() {
        for retain in [false, true] {
            let chunks = vec![Ok(vec![1; 65536]), Ok(vec![2; 32768])];
            let snapshot = capture(
                Box::pin(futures::stream::iter(chunks)),
                Arc::new(|| false),
                retain,
                100000,
            )
            .await
            .unwrap();
            let expected = [vec![1; 65536], vec![2; 32768]].concat();
            assert_eq!(snapshot.bytes, expected.len() as u64);
            assert_eq!(snapshot.identity, dsh_task_runtime::digest(&expected));
            if retain {
                let mut file = snapshot.reader().unwrap();
                file.rewind().unwrap();
                let mut actual = Vec::new();
                file.read_to_end(&mut actual).unwrap();
                assert_eq!(actual, expected);
            } else {
                assert!(snapshot.reader().is_err());
            }
        }
    }
    #[tokio::test]
    async fn pending_provider_stream_is_cancellable_and_oversize_is_not_success() {
        let aborted = Arc::new(AtomicBool::new(false));
        let flag = aborted.clone();
        let cancelling = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            capture(
                Box::pin(futures::stream::pending()),
                Arc::new(move || aborted.load(Ordering::SeqCst)),
                true,
                100,
            ),
        )
        .await
        .unwrap();
        assert!(result.err().unwrap().contains("cancelled"));
        cancelling.await.unwrap();
        let result = capture(
            Box::pin(futures::stream::iter([Ok(vec![0; 101])])),
            Arc::new(|| false),
            true,
            100,
        )
        .await;
        assert!(result.err().unwrap().contains("byte limit"));
    }
}
