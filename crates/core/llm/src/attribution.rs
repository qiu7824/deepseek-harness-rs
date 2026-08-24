//! App-attribution vocabulary for provider requests. Rust port of
//! `packages/llm/llm/src/attribution.ts`.

/// Static public application identity sent to LLM providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// `User-Agent` product token (lowercase, hyphenated).
    pub product: String,
    /// Product version.
    pub version: String,
    /// Repository home URL, used as the `User-Agent` comment.
    pub url: String,
}

/// The harness's own identity: the default every adapter sends (TS
/// `APP_IDENTITY`). The version is the crate's own version (sourced from
/// Cargo metadata, never hand-copied).
pub fn app_identity() -> AppIdentity {
    AppIdentity {
        product: "deepseek-harness".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        url: "https://github.com/deepseek-ai/deepseek-harness".to_string(),
    }
}

/// The standard `User-Agent` value: `product/version (+url)` (TS
/// `userAgent`).
pub fn user_agent(identity: &AppIdentity) -> String {
    format!(
        "{}/{}(+{})",
        identity.product, identity.version, identity.url
    )
}

/// Build the attribution headers an adapter must send on every provider
/// request (TS `attributionHeaders`).
pub fn attribution_headers(identity: &AppIdentity) -> Vec<(String, String)> {
    vec![("user-agent".to_string(), user_agent(identity))]
}
