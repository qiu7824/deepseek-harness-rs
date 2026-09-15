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
    #[serde(default)]
    pending_cleanup: Vec<CleanupRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CleanupRecord {
    record: DeepSeekUploadRecord,
    attempts: u32,
    next_attempt_at: u64,
}

#[derive(Clone)]
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
                    pending_cleanup: vec![],
                }),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(StoredIndex {
                format_version: 3,
                records: vec![],
                pending_cleanup: vec![],
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
        // The writer lock is a sibling file, so its parent must exist before
        // acquiring the lock on a first upload into a fresh profile.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        dsh_atomic_write::with_file_lock(&path, async {
            let mut index = self.load().await.map_err(std::io::Error::other)?;
            if let Some(existing) = index.records.iter().find(|record| {
                record.scope == candidate.scope
                    && record.variant_id == candidate.variant_id
                    && Self::reusable(record, now, refresh_margin)
            }) {
                let existing = existing.clone();
                if existing.file_id != candidate.file_id {
                    index.pending_cleanup.push(CleanupRecord {
                        record: candidate,
                        attempts: 0,
                        next_attempt_at: now,
                    });
                    self.save(&index).await.map_err(std::io::Error::other)?;
                }
                return Ok::<_, std::io::Error>(UploadIndexCommit {
                    record: existing,
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
            // Old requests may still reference evicted cache entries. Their
            // provider expiry is the earliest safe cleanup time.
            index.pending_cleanup.extend(
                evicted
                    .iter()
                    .filter(|record| record.file_id != candidate.file_id)
                    .cloned()
                    .map(|record| CleanupRecord {
                        next_attempt_at: record.expires_at,
                        record,
                        attempts: 0,
                    }),
            );
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

    pub async fn pending_cleanup(
        &self,
        scope: &DeepSeekFileScope,
        now: u64,
    ) -> Result<Vec<DeepSeekUploadRecord>, String> {
        let index = self.load().await?;
        Ok(index
            .pending_cleanup
            .iter()
            .filter(|entry| {
                entry.record.scope == *scope
                    && entry.next_attempt_at <= now
                    && !index.records.iter().any(|record| {
                        record.scope == *scope && record.file_id == entry.record.file_id
                    })
            })
            .take(32)
            .map(|entry| entry.record.clone())
            .collect())
    }
    pub async fn cleanup_result(
        &self,
        scope: &DeepSeekFileScope,
        id: &DeepSeekFileId,
        success: bool,
        now: u64,
    ) -> Result<(), String> {
        dsh_atomic_write::with_file_lock(&self.path, async {
            let mut index = self.load().await.map_err(std::io::Error::other)?;
            if success {
                index
                    .pending_cleanup
                    .retain(|entry| !(entry.record.scope == *scope && entry.record.file_id == *id));
            } else {
                for entry in &mut index.pending_cleanup {
                    if entry.record.scope == *scope && entry.record.file_id == *id {
                        entry.attempts = entry.attempts.saturating_add(1);
                        entry.next_attempt_at =
                            now.saturating_add((1000u64 << entry.attempts.min(12)).min(3_600_000));
                    }
                }
            }
            self.save(&index).await.map_err(std::io::Error::other)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
}

#[allow(dead_code)]
fn _brand_round_trip(value: &str) -> DeepSeekFileId {
    deepseek_file_id(value)
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;
    #[tokio::test]
    async fn cleanup_is_durable_scoped_and_backed_off_after_failure() {
        let directory = std::env::temp_dir().join(format!("dsh-cleanup-{}", uuid::Uuid::new_v4()));
        let index = DeepSeekUploadIndex::new(directory.join("files.json"));
        let scope = deepseek_file_scope("https://fixture.invalid", "fixture");
        let record = |id: &str| DeepSeekUploadRecord {
            scope: scope.clone(),
            attachment_id: dsh_attachment::attachment_id("sha256:fixture"),
            variant_id: dsh_attachment::image_variant_id("sha256:variant"),
            file_id: deepseek_file_id(id),
            bytes: 1,
            created_at: 0,
            expires_at: 1000,
        };
        index.commit(record("first"), 0, 0).await.unwrap();
        let duplicate = index.commit(record("unused"), 0, 0).await.unwrap();
        assert!(!duplicate.accepted);
        assert_eq!(index.pending_cleanup(&scope, 0).await.unwrap().len(), 1);
        index
            .cleanup_result(&scope, &deepseek_file_id("unused"), false, 0)
            .await
            .unwrap();
        assert!(index.pending_cleanup(&scope, 0).await.unwrap().is_empty());
        assert_eq!(index.pending_cleanup(&scope, 3000).await.unwrap().len(), 1);
        assert!(
            index
                .pending_cleanup(
                    &deepseek_file_scope("https://fixture.invalid", "other-account"),
                    3000
                )
                .await
                .unwrap()
                .is_empty()
        );
        index
            .cleanup_result(&scope, &deepseek_file_id("unused"), true, 3000)
            .await
            .unwrap();
        assert!(
            index
                .pending_cleanup(&scope, 4000)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(directory.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
