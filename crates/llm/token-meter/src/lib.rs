//! Replay-aware token meter for request and surface pressure. Rust port of
//! `@deepseek-ai/dsh-token-meter`.

pub mod breakdown_projection;
pub mod estimate;
pub mod index;
pub mod invariant;
pub mod surface_fold;
pub mod surface_projection;
pub mod types;
pub mod usage_projection;

pub use breakdown_projection::context_breakdown_projection_definition;
pub use estimate::{
    ROLE_OVERHEAD, estimate_content, estimate_header, estimate_message, estimate_system_tokens,
    estimate_tools_tokens,
};
pub use index::{TokenMeter, validate_config_keys};
pub use surface_fold::{SurfaceTokenFold, fold_surface_tokens};
pub use surface_projection::{ShadowPriceClaim, SurfaceTokensFold, fold_surface_projection};
pub use types::{
    ContextBreakdownProjection, ContextPressureProjection, TokenMeasurement,
    TokenMeasurementBaseline, TokenMeterConfig, TokenSurfaceNode, TokenUsageProjection,
};
pub use usage_projection::{
    context_pressure_projection_definition, token_usage_projection_definition,
};
