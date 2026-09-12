//! User-uploaded files are immutable workspace inputs, never executable actions.

use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::api::sessions::PromptContentPart;

pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 16;

pub(crate) struct PreparedFile {
    name: String,
    data: Vec<u8>,
}

/// Validate the complete file batch before creating any workspace entries.
pub(crate) fn prepare(parts: &[PromptContentPart]) -> Result<Vec<PreparedFile>, String> {
    let mut files = Vec::new();
    let mut total = 0usize;
    for part in parts {
        let PromptContentPart::File { name, data, .. } = part else {
            continue;
        };
        if files.len() >= MAX_FILES {
            return Err("每条消息最多上传 16 个文件".into());
        }
        if name.is_empty()
            || name.len() > 240
            || name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            || name.ends_with(['.', ' '])
            || matches!(name.as_str(), "." | "..")
        {
            return Err("文件名无效或过长".into());
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&stem.as_str())
        {
            return Err("文件名属于系统保留名称".into());
        }
        if data.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
            return Err("单个文件不能超过 16 MiB".into());
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "文件编码无效")?;
        total = total.checked_add(decoded.len()).ok_or("附件总大小溢出")?;
        if decoded.len() > MAX_FILE_BYTES || total > MAX_MESSAGE_BYTES {
            return Err("附件超过单文件 16 MiB 或合计 64 MiB 的限制".into());
        }
        files.push(PreparedFile {
            name: name.clone(),
            data: decoded,
        });
    }
    Ok(files)
}

async fn directory(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let path = parent.join(name);
    match tokio::fs::create_dir(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("无法创建附件目录：{error}")),
    }
    let meta = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("附件目录不能是链接或文件".into());
    }
    let resolved = tokio::fs::canonicalize(&path)
        .await
        .map_err(|e| e.to_string())?;
    if resolved.parent() != Some(parent) {
        return Err("附件目录不在当前工作区内".into());
    }
    Ok(resolved)
}

/// Store the original bytes under the receiving workspace. A retry reuses
/// the same content path; existing different bytes and links are rejected.
pub(crate) async fn save(
    cwd: Option<&str>,
    session_id: &str,
    files: &[PreparedFile],
) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let cwd = cwd.ok_or("上传文件前请选择工作区")?;
    let root = tokio::fs::canonicalize(cwd)
        .await
        .map_err(|e| format!("工作区不可用：{e}"))?;
    let root = directory(&root, ".dsh-attachments").await?;
    let owner = format!("{:x}", Sha256::digest(session_id.as_bytes()));
    let root = directory(&root, &owner).await?;
    let mut prompts = Vec::new();
    for file in files {
        let mut hash = Sha256::new();
        hash.update(file.name.as_bytes());
        hash.update([0]);
        hash.update(&file.data);
        let object = directory(&root, &format!("{:x}", hash.finalize())).await?;
        let path = object.join(&file.name);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => {
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.len() != file.data.len() as u64
                    || tokio::fs::read(&path).await.map_err(|e| e.to_string())? != file.data
                {
                    return Err("已存在的附件内容不匹配，未覆盖文件".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let temporary = object.join(format!(".upload-{}", uuid::Uuid::new_v4()));
                let result = async {
                    let mut writer = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temporary)
                        .await?;
                    writer.write_all(&file.data).await?;
                    writer.sync_all().await?;
                    drop(writer);
                    // hard_link provides atomic no-overwrite publication on every platform.
                    tokio::fs::hard_link(&temporary, &path).await
                }
                .await;
                let _ = tokio::fs::remove_file(&temporary).await;
                result.map_err(|e| format!("附件保存失败：{e}"))?;
            }
            Err(error) => return Err(format!("无法读取附件状态：{error}")),
        }
        prompts.push(format!(
            "Attached file: {}\nPath: {}\nSize: {} bytes",
            file.name,
            path.display(),
            file.data.len()
        ));
    }
    Ok(prompts)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(name: &str, bytes: &[u8]) -> PromptContentPart {
        PromptContentPart::File {
            name: name.into(),
            media_type: None,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
    #[test]
    fn validation_rejects_paths_reserved_names_and_oversized_batches() {
        for name in [
            "../secret",
            "C:\\secret",
            "a/b",
            "NUL.txt",
            "x.",
            "",
            "a\nb",
        ] {
            assert!(prepare(&[file(name, b"data")]).is_err(), "{name}");
        }
        assert!(prepare(&vec![file("ok.txt", b""); 17]).is_err());
        assert!(
            prepare(&[PromptContentPart::File {
                name: "ok".into(),
                media_type: None,
                data: "!".into()
            }])
            .is_err()
        );
    }
    #[tokio::test]
    async fn bytes_survive_retries_and_same_names_remain_distinct() {
        let root = std::env::temp_dir().join(format!("dsh-file-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let inputs = prepare(&[file("设计.txt", b"first"), file("设计.txt", b"second")]).unwrap();
        let first = save(root.to_str(), "one", &inputs).await.unwrap();
        assert_ne!(first[0], first[1]);
        assert_eq!(first, save(root.to_str(), "one", &inputs).await.unwrap());
        assert_ne!(first, save(root.to_str(), "two", &inputs).await.unwrap());
        for (i, prompt) in first.iter().enumerate() {
            let path = prompt
                .lines()
                .nth(1)
                .unwrap()
                .strip_prefix("Path: ")
                .unwrap();
            assert_eq!(tokio::fs::read(path).await.unwrap(), inputs[i].data);
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
