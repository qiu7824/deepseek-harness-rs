use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dsh_lsp::{
    LspError, LspOperation, LspProvider, LspProviderId, LspProviderQuery, LspQueryResult,
};
use dsh_subprocess::SubprocessRuntime;

use crate::{ClientSpec, LspClient};

struct ProviderState {
    disposed: bool,
    clients: HashMap<PathBuf, Arc<LspClient>>,
}

/// Local language-server provider with one lazily initialized process per
/// canonical workspace. A lifecycle gate linearizes first creation, queries,
/// and disposal so no query can race a server shutdown.
pub struct LocalLspProvider {
    id: LspProviderId,
    extensions: BTreeMap<String, String>,
    runtime: Arc<dyn SubprocessRuntime>,
    spec: ClientSpec,
    gate: tokio::sync::Mutex<ProviderState>,
}

impl LocalLspProvider {
    pub fn new(
        id: LspProviderId,
        extensions: BTreeMap<String, String>,
        runtime: Arc<dyn SubprocessRuntime>,
        spec: ClientSpec,
    ) -> Self {
        Self {
            id,
            extensions,
            runtime,
            spec,
            gate: tokio::sync::Mutex::new(ProviderState {
                disposed: false,
                clients: HashMap::new(),
            }),
        }
    }

    pub async fn dispose(&self) -> Result<(), LspError> {
        let mut state = self.gate.lock().await;
        if state.disposed {
            return Ok(());
        }
        state.disposed = true;
        let clients = std::mem::take(&mut state.clients);
        let mut failures = Vec::new();
        for (_, client) in clients {
            if let Err(error) = client.shutdown().await {
                failures.push(error);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(LspError::new(
                format!("language-server shutdown failed: {}", failures.join("; ")),
                "LSP_DISPOSED",
            ))
        }
    }
}

#[dsh_lsp::async_trait]
impl LspProvider for LocalLspProvider {
    fn id(&self) -> &LspProviderId {
        &self.id
    }

    fn extension_to_language(&self) -> &BTreeMap<String, String> {
        &self.extensions
    }

    async fn query(&self, request: LspProviderQuery) -> Result<LspQueryResult, LspError> {
        let workspace = canonical_directory(&request.workspace_root)?;
        let file = canonical_file_inside(&workspace, &request.file_path)?;
        let workspace_uri = file_uri(&workspace)?;
        let file_uri = file_uri(&file)?;
        let text = std::fs::read_to_string(&file).map_err(|error| {
            LspError::new(
                format!("cannot read LSP source file {}: {error}", file.display()),
                "LSP_UNAVAILABLE",
            )
        })?;

        let mut state = self.gate.lock().await;
        if state.disposed {
            return Err(LspError::new(
                "local LSP provider is disposed",
                "LSP_DISPOSED",
            ));
        }
        let client = if let Some(client) = state.clients.get(&workspace) {
            client.clone()
        } else {
            let mut spec = self.spec.clone();
            spec.cwd = workspace.to_string_lossy().into_owned();
            let client = Arc::new(LspClient::spawn(self.runtime.clone(), spec).map_err(
                |error| {
                    LspError::new(
                        format!("cannot start language server: {error}"),
                        "LSP_UNAVAILABLE",
                    )
                },
            )?);
            if let Err(error) = client.initialize(&workspace_uri).await {
                let _ = client.shutdown().await;
                return Err(LspError::new(
                    format!("language server initialization failed: {error}"),
                    "LSP_UNAVAILABLE",
                ));
            }
            state.clients.insert(workspace.clone(), client.clone());
            client
        };

        match request.operation {
            LspOperation::GoToDefinition => client
                .definition(&file_uri, &request.language_id, &text, request.position)
                .await
                .map(|locations| LspQueryResult::Locations {
                    locations,
                    resolved_workspace_uri: workspace_uri,
                })
                .map_err(query_error),
            LspOperation::FindReferences => client
                .references(&file_uri, &request.language_id, &text, request.position)
                .await
                .map(|locations| LspQueryResult::Locations {
                    locations,
                    resolved_workspace_uri: workspace_uri,
                })
                .map_err(query_error),
            LspOperation::GoToImplementation => client
                .implementation(&file_uri, &request.language_id, &text, request.position)
                .await
                .map(|locations| LspQueryResult::Locations {
                    locations,
                    resolved_workspace_uri: workspace_uri,
                })
                .map_err(query_error),
            LspOperation::Hover => client
                .hover(&file_uri, &request.language_id, &text, request.position)
                .await
                .map(|hover| LspQueryResult::Hover { hover })
                .map_err(query_error),
        }
    }
}

fn canonical_directory(raw: &str) -> Result<PathBuf, LspError> {
    let path = std::fs::canonicalize(raw).map_err(|error| {
        LspError::new(
            format!("cannot resolve LSP workspace {raw:?}: {error}"),
            "LSP_UNAVAILABLE",
        )
    })?;
    if !path.is_dir() {
        return Err(LspError::new(
            format!("LSP workspace is not a directory: {}", path.display()),
            "LSP_UNAVAILABLE",
        ));
    }
    Ok(path)
}

fn canonical_file_inside(workspace: &Path, raw: &str) -> Result<PathBuf, LspError> {
    let requested = Path::new(raw);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace.join(requested)
    };
    let file = std::fs::canonicalize(&candidate).map_err(|error| {
        LspError::new(
            format!("cannot resolve LSP file {}: {error}", candidate.display()),
            "LSP_UNAVAILABLE",
        )
    })?;
    if !file.starts_with(workspace) || !file.is_file() {
        return Err(LspError::new(
            format!(
                "LSP file must be a regular file inside workspace {}",
                workspace.display()
            ),
            "LSP_UNAVAILABLE",
        ));
    }
    Ok(file)
}

fn file_uri(path: &Path) -> Result<String, LspError> {
    let mut raw = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = raw.strip_prefix("//?/") {
        raw = stripped.to_string();
    }
    let encoded = percent_encode_path(&raw);
    if encoded.starts_with('/') {
        Ok(format!("file://{encoded}"))
    } else if encoded.as_bytes().get(1) == Some(&b':') {
        Ok(format!("file:///{encoded}"))
    } else {
        Err(LspError::new(
            format!("cannot encode absolute file URI for {}", path.display()),
            "LSP_UNAVAILABLE",
        ))
    }
}

fn percent_encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn query_error(error: String) -> LspError {
    LspError::new(error, "LSP_UNAVAILABLE")
}
