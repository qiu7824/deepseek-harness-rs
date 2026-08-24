use std::path::PathBuf;

use dsh_attachment::{AttachmentId, ImageVariantId};
use dsh_brand::Branded;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{DeepSeekFileId, deepseek_file_id};

#[doc(hidden)]
pub enum DeepSeekFileScopeTag {}
pub type DeepSeekFileScope = Branded<DeepSeekFileScopeTag>;

pub fn deepseek_file_scope(base_url: &str, api_key: &str) -> DeepSeekFileScope {
    let mut hasher = Sha256::new();
    hasher.update(base_url.trim_end_matches('/').as_bytes());
    hasher.update(b"\0");
    hasher.update(api_key.as_bytes());
    DeepSeekFileScope::new(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekUploadRecord {
    pub scope: DeepSeekFileScope,
    pub attachment_id: AttachmentId,
    pub variant_id: ImageVariantId,
    pub file_id: DeepSeekFileId,
    pub bytes: u64,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadIndexCommit {
    pub record: DeepSeekUploadRecord,
    pub accepted: bool,
    pub evicted: Vec<DeepSeekUploadRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIndex {
    format_version: u8,
    records: Vec<DeepSeekUploadRecord>,
}

pub struct DeepSeekUploadIndex {
    path: PathBuf,
}

impl DeepSeekUploadIndex {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    async fn load(&self) -> Result<StoredIndex, String> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(text) => match serde_json::from_str::<StoredIndex>(&text) {
                Ok(index) if index.format_version == 3 => Ok(index),
                Ok(_) | Err(_) => Ok(StoredIndex {
                    format_version: 3,
                    records: vec![],
                }),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(StoredIndex {
                format_version: 3,
                records: vec![],
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn save(&self, index: &StoredIndex) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(index).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        dsh_atomic_write::write_file_atomic(
            &self.path,
            &bytes,
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|error| error.to_string())
    }

    fn reusable(record: &DeepSeekUploadRecord, now: u64, refresh_margin: u64) -> bool {
        record.expires_at.saturating_sub(now) > refresh_margin
    }

    pub async fn get(
        &self,
        scope: &DeepSeekFileScope,
        variant: &ImageVariantId,
        now: u64,
        refresh_margin: u64,
    ) -> Result<Option<DeepSeekUploadRecord>, String> {
        Ok(self.load().await?.records.into_iter().find(|record| {
            record.scope == *scope
                && record.variant_id == *variant
                && Self::reusable(record, now, refresh_margin)
        }))
    }

    pub async fn invalidate_exact(
        &self,
        scope: &DeepSeekFileScope,
        variant: &ImageVariantId,
        file_id: &DeepSeekFileId,
    ) -> Result<bool, String> {
        let path = self.path.clone();
        dsh_atomic_write::with_file_lock(&path, async {
            let mut index = self.load().await.map_err(std::io::Error::other)?;
            let before = index.records.len();
            index.records.retain(|record| {
                !(record.scope == *scope
                    && record.variant_id == *variant
                    && record.file_id == *file_id)
            });
            if index.records.len() == before {
                return Ok::<_, std::io::Error>(false);
            }
            self.save(&index).await.map_err(std::io::Error::other)?;
            Ok(true)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
    }

    pub async fn commit(
        &self,
        candidate: DeepSeekUploadRecord,
        now: u64,
        refresh_margin: u64,
    ) -> Result<UploadIndexCommit, String> {
        let path = self.path.clone();
        dsh_atomic_write::with_file_lock(&path, async {
            let mut index = self.load().await.map_err(std::io::Error::other)?;
            if let Some(existing) = index.records.iter().find(|record| {
                record.scope == candidate.scope
                    && record.variant_id == candidate.variant_id
                    && Self::reusable(record, now, refresh_margin)
            }) {
                return Ok::<_, std::io::Error>(UploadIndexCommit {
                    record: existing.clone(),
                    accepted: false,
                    evicted: Vec::new(),
                });
            }
            let mut evicted = Vec::new();
            index.records.retain(|record| {
                if record.scope != candidate.scope {
                    return true;
                }
                let retain = Self::reusable(record, now, refresh_margin)
                    && record.variant_id != candidate.variant_id;
                if !retain {
                    evicted.push(record.clone());
                }
                retain
            });
            index.records.push(candidate.clone());
            self.save(&index).await.map_err(std::io::Error::other)?;
            Ok(UploadIndexCommit {
                record: candidate,
                accepted: true,
                evicted,
            })
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
    }
}

#[allow(dead_code)]
fn _brand_round_trip(value: &str) -> DeepSeekFileId {
    deepseek_file_id(value)
}
