//! Generic stdio LSP client/provider.

pub mod client;
pub mod framing;
pub mod provider;

pub use client::{ClientSpec, LspClient};
pub use framing::{FramingError, MessageDecoder, encode_message};
pub use provider::LocalLspProvider;
