//! The measurement service's positional surface fold. Rust port of
//! `packages/llm/token-meter/src/surface-fold.ts`.

use dsh_session::{SessionEvent, derive_event_message};
use dsh_session::surface::is_surface_event;

use crate::estimate::estimate_message;
use crate::types::TokenSurfaceNode;

/// One surface event's placement and cost against the surface preceding it.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTokenFold {
    pub tokens: u64,
    pub nodes: Vec<TokenSurfaceNode>,
    pub delta_tokens: i64,
}

/// Fold one surface event onto a priced surface (TS `foldSurfaceTokens`).
pub fn fold_surface_tokens(
    nodes: &[TokenSurfaceNode],
    event: &SessionEvent,
) -> Result<SurfaceTokenFold, String> {
    if !is_surface_event(event) {
        return Ok(SurfaceTokenFold {
            tokens: 0,
            nodes: nodes.to_vec(),
            delta_tokens: 0,
        });
    }
    let message = derive_event_message(event);
    let tokens = match message {
        Some(message) => estimate_message(&message),
        None => 0,
    };
    match &event.surface_op {
        None => Ok(SurfaceTokenFold {
            tokens,
            nodes: nodes.to_vec(),
            delta_tokens: 0,
        }),
        Some(dsh_session::SurfaceOp::Append) => {
            let mut next = nodes.to_vec();
            next.push(TokenSurfaceNode { seq: event.seq, tokens });
            Ok(SurfaceTokenFold {
                tokens,
                nodes: next,
                delta_tokens: tokens as i64,
            })
        }
        Some(dsh_session::SurfaceOp::Replace { start, end }) => {
            let start_idx = nodes.iter().position(|node| node.seq == *start);
            let end_idx = nodes.iter().position(|node| node.seq == *end);
            let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) else {
                return Err(format!(
                    "token surface: replace at seq {} has invalid current range {start}-{end}",
                    event.seq
                ));
            };
            if start_idx > end_idx {
                return Err(format!(
                    "token surface: replace at seq {} has invalid current range {start}-{end}",
                    event.seq
                ));
            }
            let removed: u64 = nodes[start_idx..=end_idx].iter().map(|node| node.tokens).sum();
            let mut next = nodes.to_vec();
            next.splice(start_idx..=end_idx, [TokenSurfaceNode { seq: event.seq, tokens }]);
            Ok(SurfaceTokenFold {
                tokens,
                nodes: next,
                delta_tokens: tokens as i64 - removed as i64,
            })
        }
    }
}
