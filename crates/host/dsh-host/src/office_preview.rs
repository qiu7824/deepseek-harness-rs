//! WPS-only export of an authorized immutable input copy; disk-backed cache.
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, Semaphore},
};

pub(crate) fn shared() -> Arc<OfficePreview> {
    // Hosts/tools own this service. A static strong reference would retain
    // cached temporary PDFs after Host shutdown and never run their cleanup.
    static SERVICE: std::sync::OnceLock<parking_lot::Mutex<std::sync::Weak<OfficePreview>>> =
        std::sync::OnceLock::new();
    let mut service = SERVICE.get_or_init(Default::default).lock();
    if let Some(service) = service.upgrade() {
        return service;
    }
    let instance = Arc::new(OfficePreview::default());
    *service = Arc::downgrade(&instance);
    instance
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(crate) struct PreviewPdf {
    pub path: PathBuf,
    pub bytes: u64,
    _directory: Cleanup,
}
impl PreviewPdf {
    pub async fn body(self: Arc<Self>) -> Result<axum::body::Body, String> {
        // File precedes lease so Windows closes the handle before last-owner cleanup.
        struct Stream {
            file: tokio::fs::File,
            _lease: Arc<PreviewPdf>,
        }
        let file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|e| e.to_string())?;
        let chunks =
            futures::stream::try_unfold(Stream { file, _lease: self }, |mut state| async move {
                let mut bytes = vec![0u8; 32 * 1024];
                let count = state.file.read(&mut bytes).await?;
                if count == 0 {
                    return Ok::<_, std::io::Error>(None);
                }
                bytes.truncate(count);
                Ok(Some((bytes, state)))
            });
        Ok(axum::body::Body::from_stream(chunks))
    }
}
/// Hash exactly the bounded bytes that will be validated and rendered.
/// Source changes after the snapshot cannot change its identity or PDF contents.
async fn snapshot(source: &Path, input: &Path, limit: u64) -> Result<String, String> {
    let mut source = tokio::fs::File::open(source)
        .await
        .map_err(|e| e.to_string())?;
    let metadata = source.metadata().await.map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Document exceeds the file size limit".into());
    }
    let mut output = tokio::fs::File::create(input)
        .await
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = source.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err("Document grew beyond the file size limit".into());
        }
        digest.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .await
            .map_err(|e| e.to_string())?;
    }
    output.flush().await.map_err(|e| e.to_string())?;
    Ok(format!("{:x}", digest.finalize()))
}
async fn directory() -> Result<Cleanup, String> {
    let directory = std::env::temp_dir().join(format!("dsh-wps-preview-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&directory)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Cleanup(directory))
}
pub(crate) async fn snapshot_pdf(source: &Path) -> Result<(String, Arc<PreviewPdf>), String> {
    let directory = directory().await?;
    let path = directory.0.join("preview.pdf");
    let identity = snapshot(source, &path, 64 * 1024 * 1024).await?;
    let check = path.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        dsh_task_runtime::office::validate_export_file_framing(&check)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok((
        identity,
        Arc::new(PreviewPdf {
            path,
            bytes,
            _directory: directory,
        }),
    ))
}
pub(crate) struct OfficePreview {
    queue: Semaphore,
    worker: Mutex<()>,
    cache: Mutex<VecDeque<(String, Arc<PreviewPdf>)>>,
}
impl Default for OfficePreview {
    fn default() -> Self {
        Self {
            queue: Semaphore::new(4),
            worker: Mutex::new(()),
            cache: Mutex::new(VecDeque::new()),
        }
    }
}
impl OfficePreview {
    pub async fn export(&self, source: &Path) -> Result<(String, Arc<PreviewPdf>), String> {
        let _slot = self
            .queue
            .try_acquire()
            .map_err(|_| "文档转换队列已满，请稍后重试")?;
        let ext = source
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "docx" | "xlsx" | "pptx") {
            return Err("此格式请使用本地 WPS 打开；内嵌预览支持 DOCX、XLSX 和 PPTX".into());
        }
        // Acquire before any input decoding. Queued callers retain paths only.
        let _worker = tokio::time::timeout(Duration::from_secs(180), self.worker.lock())
            .await
            .map_err(|_| "等待文档转换超时")?;
        let directory = directory().await?;
        let input = directory.0.join(format!("input.{ext}"));
        let identity = snapshot(source, &input, 32 * 1024 * 1024).await?;
        if let Some((_, pdf)) = self
            .cache
            .lock()
            .await
            .iter()
            .find(|(key, _)| key == &identity)
        {
            return Ok((identity, pdf.clone()));
        }
        let check = input.clone();
        tokio::task::spawn_blocking(move || {
            dsh_task_runtime::office::validate_office_file_for_automation(&check, &ext)
        })
        .await
        .map_err(|e| e.to_string())??;
        #[cfg(not(windows))]
        {
            return Err("此平台未配置 WPS 转换服务，请使用本地应用打开".into());
        }
        #[cfg(windows)]
        {
            let output = directory.0.join("preview.pdf");
            let script = directory.0.join("export.ps1");
            tokio::fs::write(&script, include_bytes!("office_preview.ps1"))
                .await
                .map_err(|e| e.to_string())?;
            let shell = Path::new(&std::env::var_os("SystemRoot").ok_or("Windows 系统目录不可用")?)
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let mut command = tokio::process::Command::new(shell);
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(script)
                .env("DSH_OFFICE_INPUT", &input)
                .env("DSH_OFFICE_OUTPUT", &output)
                .kill_on_drop(true)
                .creation_flags(0x08000000);
            let result = tokio::time::timeout(Duration::from_secs(90), command.output())
                .await
                .map_err(|_| "WPS 转换超过 90 秒，请检查 WPS 状态后重试")?
                .map_err(|e| e.to_string())?;
            if !result.status.success() {
                return Err(format!(
                    "WPS 转换失败：{}",
                    String::from_utf8_lossy(&result.stderr)
                        .chars()
                        .take(1200)
                        .collect::<String>()
                ));
            }
            let check = output.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                dsh_task_runtime::office::validate_export_file_framing(&check)
            })
            .await
            .map_err(|e| e.to_string())??;
            // Only the PDF remains in the disk cache; release source and script copies.
            tokio::fs::remove_file(&input)
                .await
                .map_err(|e| e.to_string())?;
            tokio::fs::remove_file(directory.0.join("export.ps1"))
                .await
                .map_err(|e| e.to_string())?;
            let pdf = Arc::new(PreviewPdf {
                path: output,
                bytes,
                _directory: directory,
            });
            let mut cache = self.cache.lock().await;
            while cache.len() >= 8
                || cache.iter().map(|(_, pdf)| pdf.bytes).sum::<u64>() + bytes > 64 * 1024 * 1024
            {
                if cache.pop_front().is_none() {
                    break;
                }
            }
            cache.push_back((identity.clone(), pdf.clone()));
            Ok((identity, pdf))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    #[tokio::test]
    async fn pdf_stream_keeps_snapshot_alive_without_whole_file_chunks() {
        let source_dir = directory().await.unwrap();
        let source = source_dir.0.join("source.pdf");
        let mut file = tokio::fs::File::create(&source).await.unwrap();
        file.write_all(b"%PDF-1.7\n").await.unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..144 {
            file.write_all(&chunk).await.unwrap();
        }
        file.write_all(b"\n%%EOF\n").await.unwrap();
        drop(file);
        let (identity, pdf) = snapshot_pdf(&source).await.unwrap();
        let owned_path = pdf.path.clone();
        let expected_bytes = pdf.bytes;
        let body = pdf.body().await.unwrap();
        // The exported snapshot must not follow later edits of the source.
        tokio::fs::write(&source, b"changed").await.unwrap();
        assert!(owned_path.exists());
        let mut stream = body.into_data_stream();
        let mut digest = Sha256::new();
        let mut count = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= 32 * 1024);
            count += chunk.len() as u64;
            digest.update(&chunk);
        }
        assert_eq!(count, expected_bytes);
        assert_eq!(format!("{:x}", digest.finalize()), identity);
        drop(stream);
        assert!(
            !owned_path.exists(),
            "last response owner releases temporary PDF"
        );
    }
    #[tokio::test]
    async fn cancelled_pdf_response_releases_file_and_snapshot() {
        let directory = directory().await.unwrap();
        let source = directory.0.join("source.pdf");
        tokio::fs::write(&source, b"%PDF-1.7\ncontent\n%%EOF\n")
            .await
            .unwrap();
        let (_, pdf) = snapshot_pdf(&source).await.unwrap();
        let path = pdf.path.clone();
        let body = pdf.body().await.unwrap();
        drop(body);
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn queued_exports_wait_before_reading_inputs() {
        let office = Arc::new(OfficePreview::default());
        let worker = office.worker.lock().await;
        let pending_office = office.clone();
        let pending =
            tokio::spawn(async move { pending_office.export(Path::new("missing.docx")).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while office.queue.available_permits() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !pending.is_finished(),
            "input access must wait behind worker admission"
        );
        pending.abort();
        assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
        drop(worker);
        assert_eq!(office.queue.available_permits(), 4);
    }
}
