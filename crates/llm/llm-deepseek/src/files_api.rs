use dsh_brand::Branded;
use serde_json::Value;
use std::time::Duration;

pub const MIN_FILE_EXPIRY_SECONDS: u64 = 3_600;
pub const MAX_FILE_EXPIRY_SECONDS: u64 = 2_592_000;
pub const MAX_FILE_UPLOAD_BYTES: usize = 128 * 1024 * 1024;

#[doc(hidden)]
pub enum DeepSeekFileIdTag {}

/// Opaque provider file identifier.
pub type DeepSeekFileId = Branded<DeepSeekFileIdTag>;

pub fn deepseek_file_id(value: impl Into<String>) -> DeepSeekFileId {
    DeepSeekFileId::new(value)
}

/// Validated object returned by the OpenAI-compatible Files API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekFileObject {
    pub id: DeepSeekFileId,
    pub bytes: u64,
    pub created_at: u64,
    pub filename: String,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesErrorCode {
    Auth,
    RateLimit,
    Server,
    FilesApi,
}

pub fn classify_files_status(status: u16) -> FilesErrorCode {
    match status {
        401 | 403 => FilesErrorCode::Auth,
        429 => FilesErrorCode::RateLimit,
        500..=599 => FilesErrorCode::Server,
        _ => FilesErrorCode::FilesApi,
    }
}

fn invalid() -> String {
    "DeepSeek Files API returned an invalid file object".to_string()
}

pub fn parse_file_object(value: &Value) -> Result<DeepSeekFileObject, String> {
    let object = value.as_object().ok_or_else(invalid)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)?;
    if object.get("object").and_then(Value::as_str) != Some("file")
        || object.get("purpose").and_then(Value::as_str) != Some("user_data")
    {
        return Err(invalid());
    }
    let bytes = object
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(invalid)?;
    let created_at = object
        .get("created_at")
        .and_then(Value::as_u64)
        .ok_or_else(invalid)?;
    let filename = object
        .get("filename")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)?;
    let expires_at = match object.get("expires_at") {
        None => None,
        Some(value) => Some(value.as_u64().ok_or_else(invalid)?),
    };
    Ok(DeepSeekFileObject {
        id: deepseek_file_id(id),
        bytes,
        created_at,
        filename: filename.to_string(),
        expires_at,
    })
}

#[derive(Debug)]
pub struct DeepSeekFilesError {
    pub code: FilesErrorCode,
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for DeepSeekFilesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeepSeekFilesError {}

pub struct DeepSeekFilesClient {
    base_url: String,
    api_key: String,
    timeout: Duration,
    client: reqwest::Client,
}

impl DeepSeekFilesClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            timeout,
            client: reqwest::Client::new(),
        }
    }

    async fn accept(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, DeepSeekFilesError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(DeepSeekFilesError {
            code: classify_files_status(status),
            status: Some(status),
            message: if body.is_empty() {
                format!("DeepSeek Files API error (HTTP {status})")
            } else {
                body
            },
        })
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, DeepSeekFilesError> {
        let response = request
            .bearer_auth(&self.api_key)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: format!("DeepSeek Files API transport failed: {error}"),
            })?;
        self.accept(response).await
    }

    fn parse_response(value: &Value) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        parse_file_object(value).map_err(|message| DeepSeekFilesError {
            code: FilesErrorCode::FilesApi,
            status: None,
            message,
        })
    }

    pub async fn retrieve(
        &self,
        file_id: &DeepSeekFileId,
    ) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        let response = self
            .send(
                self.client
                    .get(format!("{}/files/{}", self.base_url, file_id.as_str())),
            )
            .await?;
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: error.to_string(),
            })?;
        Self::parse_response(&value)
    }

    pub async fn delete(&self, file_id: &DeepSeekFileId) -> Result<(), DeepSeekFilesError> {
        let response = self
            .send(
                self.client
                    .delete(format!("{}/files/{}", self.base_url, file_id.as_str())),
            )
            .await?;
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: error.to_string(),
            })?;
        if value.get("id").and_then(Value::as_str) == Some(file_id.as_str())
            && value.get("object").and_then(Value::as_str) == Some("file")
            && value.get("deleted").and_then(Value::as_bool) == Some(true)
        {
            Ok(())
        } else {
            Err(DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: "DeepSeek Files API returned an invalid delete response".to_string(),
            })
        }
    }

    pub async fn upload(
        &self,
        data: Vec<u8>,
        media_type: &str,
        filename: &str,
        expires_after_seconds: u64,
    ) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        if data.len() > MAX_FILE_UPLOAD_BYTES
            || !(MIN_FILE_EXPIRY_SECONDS..=MAX_FILE_EXPIRY_SECONDS).contains(&expires_after_seconds)
        {
            return Err(DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: "DeepSeek Files upload parameters are invalid".to_string(),
            });
        }
        let part = reqwest::multipart::Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str(media_type)
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: error.to_string(),
            })?;
        let form = reqwest::multipart::Form::new()
            .text("purpose", "user_data")
            .text("expires_after[anchor]", "created_at")
            .text("expires_after[seconds]", expires_after_seconds.to_string())
            .part("file", part);
        let response = self
            .send(
                self.client
                    .post(format!("{}/files", self.base_url))
                    .multipart(form),
            )
            .await?;
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: error.to_string(),
            })?;
        let parsed = Self::parse_response(&value)?;
        if parsed.expires_at.is_none() {
            return Err(DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: "DeepSeek Files upload response omitted expiry".to_string(),
            });
        }
        Ok(parsed)
    }
}
