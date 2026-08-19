use std::sync::Arc;
use std::time::Duration;

use dsh_lsp::{LspPosition, LspRange};
use dsh_lsp_stdio::{ClientSpec, LspClient};
use dsh_subprocess::SubprocessRuntime;
use dsh_subprocess_local::LocalSubprocessRuntime;

fn client(runtime: Arc<dyn SubprocessRuntime>) -> LspClient {
    LspClient::spawn(
        runtime,
        ClientSpec {
            command: env!("CARGO_BIN_EXE_lsp-fixture").to_string(),
            args: Vec::new(),
            cwd: env!("CARGO_MANIFEST_DIR").to_string(),
            max_message_bytes: 1_000_000,
            max_stderr_bytes: 10_000,
            shutdown_timeout_ms: 1_000,
            kill_grace_ms: 500,
        },
    )
    .expect("spawn real fixture")
}

#[tokio::test(flavor = "multi_thread")]
async fn real_fixture_round_trips_all_operations_and_exits_cleanly() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let runtime: Arc<dyn SubprocessRuntime> = LocalSubprocessRuntime::new();
        let client = client(runtime);

        let initialized = client
            .initialize("file:///workspace")
            .await
            .expect("initialize response");
        assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-16");

        let locations = client
            .definition(
                "file:///workspace/src/main.rs",
                "rust",
                "fn main() {}",
                LspPosition {
                    line: 0,
                    character: 3,
                },
            )
            .await
            .expect("definition response");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri, "file:///workspace/src/definition.rs");
        assert_eq!(
            locations[0].range.start,
            LspPosition {
                line: 4,
                character: 2
            }
        );

        let references = client
            .references(
                "file:///workspace/src/main.rs",
                "rust",
                "fn main() {}",
                LspPosition {
                    line: 0,
                    character: 3,
                },
            )
            .await
            .expect("references response");
        assert_eq!(references.len(), 2);
        assert_eq!(references[1].uri, "file:///workspace/tests/reference.rs");

        let implementations = client
            .implementation(
                "file:///workspace/src/main.rs",
                "rust",
                "fn main() {}",
                LspPosition {
                    line: 0,
                    character: 3,
                },
            )
            .await
            .expect("implementation response");
        assert_eq!(implementations.len(), 1);
        assert_eq!(
            implementations[0].uri,
            "file:///workspace/src/implementation.rs"
        );

        let hover = client
            .hover(
                "file:///workspace/src/main.rs",
                "rust",
                "fn main() {}",
                LspPosition {
                    line: 0,
                    character: 3,
                },
            )
            .await
            .expect("hover response")
            .expect("hover value");
        assert!(
            hover.contents.starts_with("fn main() [pid="),
            "{}",
            hover.contents
        );
        assert_eq!(
            hover.range,
            Some(LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0
                },
                end: LspPosition {
                    line: 0,
                    character: 7
                },
            })
        );
        assert!(
            client
                .stderr_tail()
                .contains("fixture diagnostic on stderr")
        );

        client.shutdown().await.expect("bounded shutdown/exit");
    })
    .await
    .expect("stdio tracer must remain bounded");
}

#[tokio::test(flavor = "multi_thread")]
async fn request_error_still_closes_transient_document() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let runtime: Arc<dyn SubprocessRuntime> = LocalSubprocessRuntime::new();
        let client = client(runtime);
        client
            .initialize("file:///workspace")
            .await
            .expect("initialize");

        let uri = "file:///workspace/src/reopen.rs";
        let failure = client
            .hover(
                uri,
                "rust",
                "request-error",
                LspPosition {
                    line: 0,
                    character: 0,
                },
            )
            .await
            .expect_err("fixture returns a JSON-RPC error");
        assert!(failure.contains("fixture request error"), "{failure}");

        let hover = client
            .hover(
                uri,
                "rust",
                "normal",
                LspPosition {
                    line: 0,
                    character: 0,
                },
            )
            .await
            .expect("didClose after the error permits reopening")
            .expect("hover value");
        assert!(
            hover.contents.starts_with("fn main() [pid="),
            "{}",
            hover.contents
        );
        client.shutdown().await.expect("bounded shutdown/exit");
    })
    .await
    .expect("error cleanup tracer must remain bounded");
}
