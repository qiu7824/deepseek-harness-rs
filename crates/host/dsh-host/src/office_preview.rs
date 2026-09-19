//! WPS-only export of an authorized immutable input copy; bounded worker/cache.
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, path::Path, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

pub(crate) struct OfficePreview {
    queue: Semaphore,
    worker: Mutex<()>,
    cache: Mutex<VecDeque<(String, Arc<Vec<u8>>)>>,
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
    pub async fn export(&self, source: &Path) -> Result<(String, Arc<Vec<u8>>), String> {
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
        if tokio::fs::metadata(source)
            .await
            .map_err(|e| e.to_string())?
            .len()
            > 32 * 1024 * 1024
        {
            return Err("Office 文件超过 32 MiB 转换上限".into());
        }
        let bytes = tokio::fs::read(source).await.map_err(|e| e.to_string())?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("Office 文件超过转换上限".into());
        }
        dsh_task_runtime::office::validate_office_for_automation(&bytes, &ext)?;
        let identity = format!("{:x}", Sha256::digest(&bytes));
        let _worker = tokio::time::timeout(Duration::from_secs(180), self.worker.lock())
            .await
            .map_err(|_| "等待文档转换超时")?;
        if let Some((_, pdf)) = self
            .cache
            .lock()
            .await
            .iter()
            .find(|(key, _)| key == &identity)
        {
            return Ok((identity, pdf.clone()));
        }
        #[cfg(not(windows))]
        {
            let _ = bytes;
            return Err("此平台未配置 WPS 转换服务，请使用本地应用打开".into());
        }
        #[cfg(windows)]
        {
            let directory =
                std::env::temp_dir().join(format!("dsh-wps-preview-{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir(&directory)
                .await
                .map_err(|e| e.to_string())?;
            struct Cleanup(std::path::PathBuf);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    let _ = std::fs::remove_dir_all(&self.0);
                }
            }
            let _cleanup = Cleanup(directory.clone());
            let input = directory.join(format!("input.{ext}"));
            let output = directory.join("preview.pdf");
            let script = directory.join("export.ps1");
            tokio::fs::write(&input, bytes)
                .await
                .map_err(|e| e.to_string())?;
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
            if tokio::fs::metadata(&output)
                .await
                .map_err(|_| "WPS 未生成 PDF")?
                .len()
                > 64 * 1024 * 1024
            {
                return Err("转换后的 PDF 超过 64 MiB".into());
            }
            let pdf = Arc::new(tokio::fs::read(output).await.map_err(|e| e.to_string())?);
            dsh_task_runtime::office::validate_export_framing(&pdf)?;
            let mut cache = self.cache.lock().await;
            while cache.len() >= 8
                || cache.iter().map(|(_, value)| value.len()).sum::<usize>() + pdf.len()
                    > 64 * 1024 * 1024
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
