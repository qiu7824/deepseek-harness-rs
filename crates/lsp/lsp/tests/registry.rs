use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dsh_lsp::{
    Lsp, LspOperation, LspPosition, LspProvider, LspProviderId, LspProviderQuery, LspQueryRequest,
    LspQueryResult,
};

struct RecordingProvider {
    id: LspProviderId,
    extensions: BTreeMap<String, String>,
    seen: Mutex<Vec<LspProviderQuery>>,
}

fn provider(id: &str, extensions: &[(&str, &str)]) -> Arc<RecordingProvider> {
    Arc::new(RecordingProvider {
        id: LspProviderId::new(id),
        extensions: extensions
            .iter()
            .map(|(extension, language)| ((*extension).to_string(), (*language).to_string()))
            .collect(),
        seen: Mutex::new(Vec::new()),
    })
}

fn request(file_path: &str) -> LspQueryRequest {
    LspQueryRequest {
        operation: LspOperation::GoToDefinition,
        file_path: file_path.to_string(),
        position: LspPosition {
            line: 2,
            character: 4,
        },
        workspace_root: "/workspace".to_string(),
    }
}

#[async_trait]
impl LspProvider for RecordingProvider {
    fn id(&self) -> &LspProviderId {
        &self.id
    }

    fn extension_to_language(&self) -> &BTreeMap<String, String> {
        &self.extensions
    }

    async fn query(&self, request: LspProviderQuery) -> Result<LspQueryResult, dsh_lsp::LspError> {
        self.seen.lock().expect("seen lock").push(request);
        Ok(LspQueryResult::Locations {
            locations: Vec::new(),
            resolved_workspace_uri: "file:///workspace".to_string(),
        })
    }
}

#[tokio::test]
async fn routes_by_final_extension_and_derives_the_language_id() {
    let provider = provider("typescript", &[("TS", "typescript")]);
    let lsp = Lsp::new();
    let _registration = lsp
        .register_provider(provider.clone())
        .expect("provider registers");

    let result = lsp
        .query(request("src/main.ts"))
        .await
        .expect("query routes");

    assert!(matches!(result, LspQueryResult::Locations { .. }));
    let seen = provider.seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].language_id, "typescript");
    assert_eq!(seen[0].file_path, "src/main.ts");
}

#[tokio::test]
async fn a_conflicting_registration_publishes_no_partial_routes() {
    let lsp = Lsp::new();
    let _typescript = lsp
        .register_provider(provider("typescript", &[(".ts", "typescript")]))
        .expect("typescript registers");

    let error = lsp
        .register_provider(provider(
            "python-and-typescript",
            &[(".py", "python"), (".ts", "typescript")],
        ))
        .err()
        .expect("the extension conflict must reject");

    assert_eq!(error.code(), "LSP_CONFLICT");
    let error = lsp
        .query(request("main.py"))
        .await
        .expect_err("the free extension must not leak from a rejected registration");
    assert_eq!(error.code(), "LSP_UNAVAILABLE");
}
