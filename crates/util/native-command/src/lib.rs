//! Shared no-shell command runner. Rust port of
//! `packages/util/native-command`.

pub mod index;
pub mod invariant;

pub use index::{
    NativeCommandAbort, NativeCommandFailure, NativeCommandLimits, NativeCommandOutput,
    run_native_command, run_native_command_bounded,
};
