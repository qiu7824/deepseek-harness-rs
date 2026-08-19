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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_standard_user_agent() {
        let identity = app_identity();
        assert_eq!(identity.product, "deepseek-harness");
        let headers = attribution_headers(&identity);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "user-agent");
        assert!(headers[0].1.starts_with("deepseek-harness/"));
        assert!(
            headers[0]
                .1
                .ends_with("(+https://github.com/deepseek-ai/deepseek-harness)")
        );
    }
}
