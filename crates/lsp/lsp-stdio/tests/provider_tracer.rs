use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dsh_lsp::{
    LspOperation, LspPosition, LspProvider, LspProviderId, LspProviderQuery, LspQueryResult,
};
use dsh_lsp_stdio::{ClientSpec, LocalLspProvider};
use dsh_subprocess::SubprocessRuntime;
use dsh_subprocess_local::LocalSubprocessRuntime;

fn fixture_workspace() -> PathBuf {
    std::env::temp_dir().join(format!("dsh-lsp-provider-tracer-{}", std::process::id()))
}

fn hover_contents(result: LspQueryResult) -> String {
    match result {
        LspQueryResult::Hover { hover: Some(hover) } => hover.contents,
        other => panic!("expected hover, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn canonical_workspace_queries_single_flight_and_reuse_real_client() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let workspace = fixture_workspace();
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(workspace.join("src")).expect("workspace tree");
        std::fs::write(workspace.join("src/main.rs"), "fn main() {}").expect("workspace source");

        let runtime: Arc<dyn SubprocessRuntime> = LocalSubprocessRuntime::new();
        let provider = LocalLspProvider::new(
            LspProviderId::new("rust-fixture"),
            BTreeMap::from([(".rs".to_string(), "rust".to_string())]),
            runtime,
            ClientSpec {
                command: env!("CARGO_BIN_EXE_lsp-fixture").to_string(),
                args: Vec::new(),
                cwd: String::new(),
                max_message_bytes: 1_000_000,
                max_stderr_bytes: 10_000,
                shutdown_timeout_ms: 200,
                kill_grace_ms: 200,
            },
        );
        assert_eq!(provider.id().as_str(), "rust-fixture");
        assert_eq!(provider.extension_to_language()[".rs"], "rust");

        let canonical_spelling = workspace.to_string_lossy().into_owned();
        let equivalent_spelling = workspace.join("src/..").to_string_lossy().into_owned();
        let request = |workspace_root: String| LspProviderQuery {
            operation: LspOperation::Hover,
            file_path: "src/main.rs".to_string(),
            position: LspPosition {
                line: 0,
                character: 3,
            },
            workspace_root,
            language_id: "rust".to_string(),
            signal: None,
        };
        let (first, second) = tokio::join!(
            provider.query(request(canonical_spelling.clone())),
            provider.query(request(equivalent_spelling)),
        );
        let first = hover_contents(first.expect("first hover"));
        let second = hover_contents(second.expect("second hover"));
        assert_eq!(
            first, second,
            "same canonical workspace must reuse one process"
        );

        let navigation = provider
            .query(LspProviderQuery {
                operation: LspOperation::GoToDefinition,
                file_path: "src/main.rs".to_string(),
                position: LspPosition {
                    line: 0,
                    character: 3,
                },
                workspace_root: canonical_spelling,
                language_id: "rust".to_string(),
                signal: None,
            })
            .await
            .expect("definition through provider");
        match navigation {
            LspQueryResult::Locations {
                locations,
                resolved_workspace_uri,
            } => {
                assert_eq!(locations.len(), 1);
                assert!(
                    resolved_workspace_uri.starts_with("file:///"),
                    "{resolved_workspace_uri}"
                );
            }
            other => panic!("expected locations, got {other:?}"),
        }

        std::fs::write(workspace.join("src/hang.rs"), "hang").expect("hanging source");
        let cancellation = dsh_lsp::LspCancellation::default();
        let cancel_handle = cancellation.clone();
        let query = provider.query(LspProviderQuery {
            operation: LspOperation::Hover,
            file_path: "src/hang.rs".to_string(),
            position: LspPosition {
                line: 0,
                character: 0,
            },
            workspace_root: workspace.to_string_lossy().into_owned(),
            language_id: "rust".to_string(),
            signal: Some(cancellation),
        });
        let cancel = async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_handle.cancel();
        };
        let (cancelled, ()) = tokio::join!(query, cancel);
        assert_eq!(
            cancelled.expect_err("hanging query cancels").code(),
            "LSP_CANCELLED"
        );

        std::fs::write(
            workspace.join("src/large.rs"),
            vec![b'x'; 8 * 1024 * 1024 + 1],
        )
        .expect("oversized source");
        let oversized = provider
            .query(LspProviderQuery {
                operation: LspOperation::Hover,
                file_path: "src/large.rs".to_string(),
                position: LspPosition {
                    line: 0,
                    character: 0,
                },
                workspace_root: workspace.to_string_lossy().into_owned(),
                language_id: "rust".to_string(),
                signal: None,
            })
            .await
            .expect_err("oversized source is rejected");
        assert_eq!(oversized.code(), "LSP_DOCUMENT_TOO_LARGE");

        provider.dispose().await.expect("bounded provider disposal");
        let disposed = provider
            .query(request(workspace.to_string_lossy().into_owned()))
            .await
            .expect_err("disposed provider rejects new work");
        assert_eq!(disposed.code(), "LSP_DISPOSED");
        std::fs::remove_dir_all(&workspace).expect("remove fixture workspace");
    })
    .await
    .expect("provider reuse and disposal must remain bounded");
}
