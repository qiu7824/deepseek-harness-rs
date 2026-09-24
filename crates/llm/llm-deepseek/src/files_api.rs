use dsh_brand::Branded;
use serde_json::Value;
use std::time::Duration;

pub const MIN_FILE_EXPIRY_SECONDS: u64 = 3_600;
pub const MAX_FILE_EXPIRY_SECONDS: u64 = 2_592_000;
pub const MAX_FILE_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MESSAGES_FILES_BETA: &str = "files-api-2025-04-14";

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
    Transport,
    Protocol,
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

async fn read_bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| e.without_url().to_string())?
        {
            if bytes.len() + chunk.len() > 65536 {
                return Err("[phase:files_response_body] response exceeded 64 KiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "[phase:files_response_body] response body deadline exceeded".to_string())?
}

async fn read_metadata(
    response: reqwest::Response,
    operation: &str,
) -> Result<Value, DeepSeekFilesError> {
    let status = response.status().as_u16();
    let bytes = read_bounded_body(response)
        .await
        .map_err(|message| DeepSeekFilesError {
            code: FilesErrorCode::Transport,
            status: Some(status),
            message: format!("Files API {operation} response failed (HTTP {status}): {message}"),
        })?;
    serde_json::from_slice(&bytes).map_err(|_| DeepSeekFilesError {
        code: FilesErrorCode::Protocol,
        status: Some(status),
        message: format!("Files API returned malformed metadata for {operation} (HTTP {status})"),
    })
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

fn parse_messages_file(value: &Value) -> Result<DeepSeekFileObject, String> {
    if value["type"] != "file" || !value["mime_type"].is_string() {
        return Err(invalid());
    }
    let id = value["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let filename = value["filename"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid)?;
    let bytes = value["size_bytes"].as_u64().ok_or_else(invalid)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(value["created_at"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?
            .timestamp();
    let created_at = u64::try_from(created_at).map_err(|_| invalid())?;
    Ok(DeepSeekFileObject {
        id: deepseek_file_id(id),
        bytes,
        created_at,
        filename: filename.into(),
        expires_at: None,
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

#[derive(Clone)]
pub struct DeepSeekFilesClient {
    messages: bool,
    base_url: String,
    api_key: String,
    timeout: Duration,
    client: reqwest::Client,
}

impl DeepSeekFilesClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Self {
        Self {
            messages: false,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            timeout,
            client: dsh_http_proxy::builder()
                .expect("valid outbound proxy policy")
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("file upload HTTP client"),
        }
    }

    pub fn messages(base_url: &str, api_key: &str, timeout: Duration) -> Self {
        let endpoint = crate::anthropic_transport::endpoint(base_url, "files");
        let mut client = Self::new(endpoint.strip_suffix("/files").unwrap(), api_key, timeout);
        client.messages = true;
        client
    }

    pub(crate) fn api_root(&self) -> &str {
        &self.base_url
    }

    fn file_url(&self, file_id: &DeepSeekFileId) -> Result<reqwest::Url, DeepSeekFilesError> {
        let error = || DeepSeekFilesError {
            code: FilesErrorCode::Protocol,
            status: None,
            message: "Invalid Files API URL".into(),
        };
        let mut url =
            reqwest::Url::parse(&format!("{}/files", self.base_url)).map_err(|_| error())?;
        url.path_segments_mut()
            .map_err(|_| error())?
            .push(file_id.as_str());
        Ok(url)
    }

    async fn accept(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, DeepSeekFilesError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = match read_bounded_body(response).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(message) => message,
        };
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
        let request = if self.messages {
            request
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", MESSAGES_FILES_BETA)
        } else {
            request.bearer_auth(&self.api_key)
        };
        let response = request
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::Transport,
                status: None,
                message: format!(
                    "DeepSeek Files API transport failed: {}",
                    error.without_url()
                ),
            })?;
        self.accept(response).await
    }

    fn parse_response(&self, value: &Value) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        (if self.messages {
            parse_messages_file(value)
        } else {
            parse_file_object(value)
        })
        .map_err(|message| DeepSeekFilesError {
            code: FilesErrorCode::Protocol,
            status: None,
            message,
        })
    }

    pub async fn retrieve(
        &self,
        file_id: &DeepSeekFileId,
    ) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        let response = self.send(self.client.get(self.file_url(file_id)?)).await?;
        let value = read_metadata(response, "retrieve").await?;
        self.parse_response(&value)
    }

    pub async fn delete(&self, file_id: &DeepSeekFileId) -> Result<(), DeepSeekFilesError> {
        let response = self
            .send(self.client.delete(self.file_url(file_id)?))
            .await?;
        let value = read_metadata(response, "delete").await?;
        if value.get("id").and_then(Value::as_str) == Some(file_id.as_str())
            && (if self.messages {
                value["type"] == "file_deleted"
            } else {
                value["object"] == "file" && value["deleted"] == true
            })
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
        let bytes = data.len() as u64;
        self.upload_stream(Box::pin(std::io::Cursor::new(data)), bytes, media_type, filename, expires_after_seconds).await
    }

    pub async fn upload_stream(
        &self, reader: dsh_attachment::AttachmentReader, bytes: u64,
        media_type: &str, filename: &str, expires_after_seconds: u64,
    ) -> Result<DeepSeekFileObject, DeepSeekFilesError> {
        if bytes > MAX_FILE_UPLOAD_BYTES as u64
            || !(MIN_FILE_EXPIRY_SECONDS..=MAX_FILE_EXPIRY_SECONDS).contains(&expires_after_seconds)
        {
            return Err(DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: "DeepSeek Files upload parameters are invalid".to_string(),
            });
        }
        let stream = async_stream::try_stream! {
            use tokio::io::AsyncReadExt;
            let mut reader = reader;
            let mut total = 0u64;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let count = reader.read(&mut buffer).await?;
                total += count as u64;
                if total > bytes || (count == 0 && total != bytes) {
                    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Image upload length changed"))?;
                }
                if count == 0 { break; }
                yield bytes::Bytes::copy_from_slice(&buffer[..count]);
            }
        };
        let stream: std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>> = Box::pin(stream);
        let part = reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), bytes)
            .file_name(filename.to_string())
            .mime_str(media_type)
            .map_err(|error| DeepSeekFilesError {
                code: FilesErrorCode::FilesApi,
                status: None,
                message: error.to_string(),
            })?;
        let form = reqwest::multipart::Form::new()
            .text("expires_after[anchor]", "created_at")
            .text("expires_after[seconds]", expires_after_seconds.to_string())
            .part("file", part);
        let form = if self.messages {
            form
        } else {
            form.text("purpose", "user_data")
        };
        let response = self
            .send(
                self.client
                    .post(format!("{}/files", self.base_url))
                    .multipart(form),
            )
            .await?;
        let value = read_metadata(response, "upload").await?;
        let mut parsed = self.parse_response(&value)?;
        if self.messages {
            parsed.expires_at = parsed.created_at.checked_add(expires_after_seconds);
        }
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
