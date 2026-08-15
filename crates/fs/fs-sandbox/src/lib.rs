//! Sandbox-enforcing filesystem backend. Rust port of
//! `@deepseek-ai/dsh-fs-sandbox`.

pub mod containment;
pub mod index;
pub mod invariant;

pub use containment::is_path_under;
pub use index::{Config, SandboxedFileSystem};
