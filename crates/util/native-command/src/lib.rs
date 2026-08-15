//! Shared no-shell command runner. Rust port of
//! `packages/util/native-command`.

pub mod index;
pub mod invariant;

pub use index::{
    NativeCommandAbort, NativeCommandFailure, NativeCommandOutput, run_native_command,
};
