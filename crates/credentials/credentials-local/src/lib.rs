//! File-backed credentials provider (`$DSH_HOME/.credentials.yaml`). Rust
//! port of `@deepseek-ai/dsh-credentials-local`.

pub mod document;
pub mod index;
pub mod invariant;

pub use document::{parse_credentials_document, render_document, serialize_scalar};
pub use index::{
    CREDENTIALS_FILENAME, Config, DocumentReader, DocumentWriter, LocalCredentialProvider,
    ResolvedSpec, WatchControl, WatchSignal, WatchSink, WatcherFactory, notify_watcher_factory,
    resolve_spec,
};
