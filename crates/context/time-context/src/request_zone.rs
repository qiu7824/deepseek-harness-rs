//! Browser-zone derivation and model-facing policy text for one open request
//! turn. Rust port of `packages/context/time-context/src/request-zone.ts`.

use dsh_llm::{MessageSource, UserMessage};

use crate::timestamp::{TimeZoneError, canonical_time_zone};

/// Browser-zone facts derived from user-rpc messages in one open turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserTimeZoneContext {
    Resolved { time_zone: String },
    Mixed { time_zones: Vec<String> },
    Missing,
}

/// Read and validate a Host-canonicalized browser zone from one ordinary
/// user-rpc message (TS `browserTimeZone`).
fn browser_time_zone(message: &UserMessage) -> Result<Option<String>, TimeZoneError> {
    let MessageSource::User {
        rpc_id,
        client_time_zone,
    } = &message.source
    else {
        return Ok(None);
    };
    // The TS contract requires a string rpcId AND a string clientTimeZone.
    let (Some(_rpc_id), Some(zone)) = (rpc_id, client_time_zone) else {
        return Ok(None);
    };
    Ok(Some(canonical_time_zone(zone)?))
}

/// Derive the unique, mixed, or missing browser zone for one open turn.
/// Returns sorted, duplicate-free facts; the first invalid zone fails the
/// whole derivation (TS `deriveBrowserTimeZoneContext`).
pub fn derive_browser_time_zone_context(
    messages: &[UserMessage],
) -> Result<BrowserTimeZoneContext, TimeZoneError> {
    let mut time_zones: Vec<String> = Vec::new();
    for message in messages {
        if let Some(time_zone) = browser_time_zone(message)? {
            if !time_zones.contains(&time_zone) {
                time_zones.push(time_zone);
            }
        }
    }
    time_zones.sort();
    match time_zones.as_slice() {
        [] => Ok(BrowserTimeZoneContext::Missing),
        [time_zone] => Ok(BrowserTimeZoneContext::Resolved {
            time_zone: time_zone.clone(),
        }),
        _ => Ok(BrowserTimeZoneContext::Mixed { time_zones }),
    }
}

/// Render the model instruction for one browser-zone context (TS
/// `renderBrowserTimeZoneContext`).
pub fn render_browser_time_zone_context(context: &BrowserTimeZoneContext) -> String {
    match context {
        BrowserTimeZoneContext::Resolved { time_zone } => format!("Browser time zone for this request: {time_zone}. Interpret otherwise-unqualified dates and times in this zone."),
        BrowserTimeZoneContext::Mixed { time_zones } => format!(
            "Browser time zone for this request: mixed {}. Ask the user to clarify otherwise-unqualified dates and times.",
            serde_json::to_string(time_zones).expect("zones")
        ),
        BrowserTimeZoneContext::Missing => "Browser time zone for this request: unavailable. Ask the user to clarify otherwise-unqualified dates and times.".to_string(),
    }
}
