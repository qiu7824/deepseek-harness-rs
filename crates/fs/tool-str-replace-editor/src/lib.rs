//! Model-facing `str_replace_editor` over the Harness filesystem seam.
//! Rust port of `packages/fs/tool-str-replace-editor`.
//!
//! # Deviations
//!
//! - Absolute-path checking uses `std::path::Path::is_absolute` (platform
//!   semantics, matching `node:path`); the POSIX-style suggestion text is
//!   retained verbatim.
//! - `FsError` failures from the `fs/edit-intent` waterfall arrive as
//!   panics (the Rust waterfall has no error channel); the tool catches
//!   them and restores the structured `{ name, code }` error info.

pub mod index;
pub mod invariant;

pub use index::{Config, NAME, ToolStrReplaceEditorPlugin, apply};
