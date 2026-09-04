//! Public configuration and measurement vocabulary for replay token
//! metering. Rust port of `packages/llm/token-meter/src/types.ts`.

use serde::{Deserialize, Serialize};

use dsh_llm::TokenUsage;

/// Token-meter plugin configuration; the fixed estimator has no settings.
#[derive(Debug, Clone, Default)]
pub struct TokenMeterConfig {}

/// The baseline from which a signed surface delta produces current pressure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TokenMeasurementBaseline {
    #[serde(rename = "none")]
    None { tokens: u64 },
    #[serde(rename = "estimated")]
    Estimated { tokens: u64 },
    #[serde(rename = "usage")]
    Usage { tokens: u64, usage: TokenUsage },
}

/// Detached immutable request-pressure and surface snapshot at one consumed
/// log revision.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenMeasurement {
    /// Number of durable events consumed.
    pub log_revision: u64,
    pub baseline: TokenMeasurementBaseline,
    pub surface_delta_tokens: i64,
    pub total_tokens: u64,
    pub surface_tokens: u64,
    pub nodes: Vec<TokenSurfaceNode>,
}

/// One token-priced node in the current ordered session surface.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSurfaceNode {
    pub seq: u64,
    pub tokens: u64,
    pub heuristic_tokens: u64,
}

/// Durable cumulative provider usage for a complete session log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageProjection {
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

/// Approximate context occupancy for a status display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPressureProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// True when the denominator is a conservative runtime budget rather
    /// than provider-declared context capacity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_estimated: Option<bool>,
}

/// Heuristic composition of the next request's context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBreakdownProjection {
    pub system_tokens: u64,
    pub tools_tokens: u64,
    pub message_tokens: u64,
}
