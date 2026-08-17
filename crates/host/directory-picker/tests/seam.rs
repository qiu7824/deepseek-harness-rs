//! Rust port of the core `packages/host/directory-picker/tests/seam.spec.ts`
//! behaviors: registration identity under `ctx.directoryPicker` (with fiber
//! teardown removal) and the typed failure's business code + subject path.

use std::sync::Arc;

use cordis::Context;
use dsh_host_directory_picker::{
    AbortSignal, DirectoryPicker, DirectoryPickerCapability,
    DirectoryPickerError, DirectoryPickerErrorCode, DirectoryPickerNativeCapability,
    register,
};

/// Minimal concrete backend: all a subclass owes the abstract class is
/// capability().
struct StubPicker {
    stub: DirectoryPickerCapability,
}

impl StubPicker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stub: DirectoryPickerCapability::Native(DirectoryPickerNativeCapability::new(Arc::new(
                |_signal: AbortSignal| Box::pin(async { None }),
            ))),
        })
    }
}

impl DirectoryPicker for StubPicker {
    fn capability(&self) -> DirectoryPickerCapability {
        self.stub.clone()
    }
}

#[test]
fn registers_as_ctx_directory_picker_and_leaves_with_its_fiber() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let ctx = Context::root();
        let disposer = register(&ctx, StubPicker::new());

        let slot =
            ctx.get_typed::<Arc<dyn DirectoryPicker>>("directoryPicker", false);
        let slot = slot.expect("service visible after registration");
        assert_eq!(slot.capability().kind(), "native");

        disposer().await;
        assert!(
            ctx.get_typed::<Arc<dyn DirectoryPicker>>("directoryPicker", false)
                .is_none(),
            "service leaves with its registration disposer"
        );
    });
}

#[test]
fn carries_the_business_code_and_subject_path_on_error() {
    let failure = DirectoryPickerError::new(
        DirectoryPickerErrorCode::DirectoryExists,
        "/home/u/x",
        "/home/u/x already exists",
    );
    assert_eq!(failure.code, DirectoryPickerErrorCode::DirectoryExists);
    assert_eq!(failure.code.as_str(), "directory-exists");
    assert_eq!(failure.path, "/home/u/x");
    assert!(
        failure.message.contains("already exists"),
        "message: {}",
        failure.message
    );
    assert_eq!(failure.to_string(), "/home/u/x already exists");
}

#[test]
fn abort_signal_wakes_waiters_and_reports() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let signal = AbortSignal::new();
        assert!(!signal.aborted());
        let wait = tokio::spawn({
            let signal = signal.clone();
            async move {
                signal.cancelled().await;
            }
        });
        signal.abort();
        wait.await.expect("waiter resolves after abort");
        assert!(signal.aborted());
    });
}
