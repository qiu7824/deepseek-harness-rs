//! Local bash executor over the subprocess seam. Rust port of
//! `packages/shell/bash-local`.

pub mod index;
pub mod invariant;

pub use index::{
    BashProcessFacts, Config, DEFAULT_GRACE_MS, DEFAULT_MAX_SPILL_BYTES, ENV_OVERRIDES,
    LocalBashExecutor, ResolvedConfig, assert_serviceable_bash_config, bash_config_schema,
};
