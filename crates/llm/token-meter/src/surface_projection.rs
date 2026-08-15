//! The O(1) surface-token fold shared by the token-meter projection units.
//! Rust port of `packages/llm/token-meter/src/surface-projection.ts`.

use serde::{Deserialize, Serialize};

use dsh_session::SessionEvent;
use dsh_session::surface::{derive_event_message, is_surface_event};

use crate::estimate::estimate_message;

/// One armed shadow price (plain JSON — part of the persisted unit state
/// while armed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowPriceClaim {
    pub start: u64,
    pub end: u64,
    pub tokens: u64,
}

/// One event's effect on a running surface-token total.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTokensFold {
    pub delta_tokens: i64,
    pub claim: Option<ShadowPriceClaim>,
}

/// Fold one committed event onto a running surface-token total (TS
/// `foldSurfaceProjection`).
pub fn fold_surface_projection(
    claim: Option<&ShadowPriceClaim>,
    event: &SessionEvent,
) -> Result<SurfaceTokensFold, String> {
    if event.type_ == "compaction/summary" || event.type_ == "compaction/prune" {
        let shadowed_range = event.data.get("shadowedRange");
        let shadowed_tokens = event.data.get("shadowedTokenCount");
        let (Some(range), Some(tokens)) = (shadowed_range, shadowed_tokens) else {
            return Ok(SurfaceTokensFold { delta_tokens: 0, claim: None });
        };
        let start = range.get("start").and_then(|value| value.as_u64());
        let end = range.get("end").and_then(|value| value.as_u64());
        let tokens = tokens.as_u64();
        let (Some(start), Some(end), Some(tokens)) = (start, end, tokens) else {
            return Ok(SurfaceTokensFold { delta_tokens: 0, claim: None });
        };
        return Ok(SurfaceTokensFold {
            delta_tokens: 0,
            claim: Some(ShadowPriceClaim { start, end, tokens }),
        });
    }
    if !is_surface_event(event) {
        return Ok(SurfaceTokensFold { delta_tokens: 0, claim: None });
    }
    let message = derive_event_message(event);
    let tokens = match message {
        Some(message) => estimate_message(&message),
        None => 0,
    };
    match &event.surface_op {
        Some(dsh_session::SurfaceOp::Append) => Ok(SurfaceTokensFold {
            delta_tokens: tokens as i64,
            claim: None,
        }),
        Some(dsh_session::SurfaceOp::Replace { start, end }) => {
            // Sessions recorded before the shadow-price protocol log
            // replacements with no adjacent metering event; fold neutrally.
            let Some(claim) = claim else {
                return Ok(SurfaceTokensFold { delta_tokens: 0, claim: None });
            };
            if claim.start != *start || claim.end != *end {
                return Err(format!(
                    "token surface: replace at seq {} over range {start}-{end} has no adjacent shadow price (armed claim covers {}-{})",
                    event.seq, claim.start, claim.end
                ));
            }
            Ok(SurfaceTokensFold {
                delta_tokens: tokens as i64 - claim.tokens as i64,
                claim: None,
            })
        }
        None => Ok(SurfaceTokensFold { delta_tokens: 0, claim: None }),
    }
}
