//! Whole-log conversation counts and wall times projection (sessionStats)
//! for the DeepSeek Harness. Rust port of
//! `@deepseek-ai/dsh-session-stats`.

pub mod index;
pub mod invariant;
pub mod projection;
pub mod types;

pub use index::{INJECT, NAME, StatsPlugin, apply};
pub use projection::session_stats_projection_definition;
pub use types::SessionStatsProjection;
