//! Package-owned invariant companion for `dsh-tool-ralph`.
//!
//! Ralph owns no independent event stream. Workflow and subagent owners
//! validate the runs and child lifecycles it starts.

/// Invariant companion plugin name.
pub const NAME: &str = "tool-ralph-invariant";

/// Package ownership key used by the invariant registry.
pub const PACKAGE_NAME: &str = "dsh-tool-ralph";

/// Intentionally empty: this package owns no independent runtime invariant.
pub fn install() {}
