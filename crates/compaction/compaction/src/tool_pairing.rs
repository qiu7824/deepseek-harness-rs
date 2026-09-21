//! Tool-pairing balance over a session surface. Rust port of
//! `packages/compaction/compaction/src/tool-pairing.ts`.
//!
//! # Deviations
//!
//! - Each Session owns its disposable balance cache. Session retirement
//!   releases it, and reused durable ids cannot inherit another log's cuts.

use std::collections::HashMap;

use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;

struct BalanceCache {
    generation: u64,
    cut_balanced: Vec<bool>,
    index_by_seq: HashMap<u64, usize>,
    in_progress_tool_calls: i64,
}

impl Default for BalanceCache {
    fn default() -> Self {
        Self {
            generation: 0,
            cut_balanced: vec![true],
            index_by_seq: HashMap::new(),
            in_progress_tool_calls: 0,
        }
    }
}

/// Return how one surface event changes the in-progress tool-call count (TS
/// `eventDelta`).
pub fn event_delta(event: &SessionEvent) -> i64 {
    match event.type_.as_str() {
        "assistant/message" => event
            .data
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(|value| value.as_str()) == Some("tool-call")
                    })
                    .count() as i64
            })
            .unwrap_or(0),
        "tool/result" => -1,
        _ => 0,
    }
}

fn event_for_seq<'a>(events: &'a [SessionEvent], seq: u64) -> Result<&'a SessionEvent, String> {
    events.get(seq as usize).filter(|event| event.seq == seq).ok_or_else(|| {
        format!(
            "tool-pairing balance: surface seq {seq} has no matching session event (corrupt surface)"
        )
    })
}

fn extend_cache(
    events: &[SessionEvent],
    cache: &mut BalanceCache,
    seqs: &[u64],
) -> Result<(), String> {
    let processed = cache.cut_balanced.len() - 1;
    let tail = &seqs[processed.min(seqs.len())..];
    // Validate the unseen tail before mutating the live cache.
    let mut pending_cuts: Vec<bool> = Vec::new();
    let mut in_progress = cache.in_progress_tool_calls;
    for seq in tail {
        in_progress += event_delta(event_for_seq(events, *seq)?);
        if in_progress < 0 {
            return Err(format!(
                "tool-pairing balance: tool/result at surface seq {seq} has no matching tool-call (corrupt surface)"
            ));
        }
        pending_cuts.push(in_progress == 0);
    }
    for (offset, seq) in tail.iter().enumerate() {
        cache.index_by_seq.insert(*seq, processed + offset);
    }
    cache.cut_balanced.extend(pending_cuts);
    cache.in_progress_tool_calls = in_progress;
    Ok(())
}

fn with_balance_cache<T>(
    session: &Session,
    read: impl FnOnce(&BalanceCache) -> Result<T, String>,
) -> Result<T, String> {
    let surface = session.surface().map_err(|error| format!("{error}"))?;
    let seqs = surface.nodes;
    let generation = surface.replace_generation;
    let cache = session.derived_cache::<Mutex<BalanceCache>>();
    let mut cached = cache.lock();
    if cached.generation != generation || cached.cut_balanced.len().saturating_sub(1) > seqs.len() {
        *cached = BalanceCache {
            generation,
            ..Default::default()
        };
    }
    if cached.cut_balanced.len() - 1 < seqs.len() {
        session.with_events(|events| extend_cache(events, &mut cached, &seqs))?;
    }
    read(&cached)
}

fn cut_balance(cache: &BalanceCache, seq: u64, offset: usize) -> Result<bool, String> {
    let index = cache
        .index_by_seq
        .get(&seq)
        .ok_or_else(|| format!("tool-pairing balance: surface seq {seq} not found"))?;
    cache
        .cut_balanced
        .get(index + offset)
        .copied()
        .ok_or_else(|| format!("tool-pairing balance: surface seq {seq} not found"))
}

/// Whether the cut immediately before a current surface sequence is
/// tool-pairing balanced (TS `toolPairingBalancedBefore`).
pub fn tool_pairing_balanced_before(session: &Session, seq: u64) -> Result<bool, String> {
    with_balance_cache(session, |cache| cut_balance(cache, seq, 0))
}

/// Whether the cut immediately after a current surface sequence is
/// tool-pairing balanced (TS `toolPairingBalancedAfter`).
pub fn tool_pairing_balanced_after(session: &Session, seq: u64) -> Result<bool, String> {
    with_balance_cache(session, |cache| cut_balance(cache, seq, 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SurfaceIntent, SurfaceOp, session_id};

    fn append(session: &Session, kind: &str, data: serde_json::Value) -> u64 {
        session
            .append(
                kind,
                data,
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap()
            .seq
            .get()
    }

    fn tool_call(session: &Session) -> u64 {
        append(
            session,
            "assistant/message",
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {"id":"assistant", "role":"assistant", "content":[
                    {"type":"tool-call", "id":"call-1", "name":"read", "arguments":"{}"}
                ], "source":{"kind":"model", "provider":"fixture", "model":"fixture"}}
            }),
        )
    }

    #[test]
    fn reused_durable_id_has_independent_balance_and_incremental_results() {
        let old = Session::create(session_id("same-id"), None, None, None).unwrap();
        let user = append(
            &old,
            "user/message",
            serde_json::json!({
                "id":"user", "role":"user", "content":[{"type":"text","text":"hello"}],
                "source":{"kind":"user"}
            }),
        );
        assert!(tool_pairing_balanced_after(&old, user).unwrap());
        let current = Session::create(session_id("same-id"), None, None, None).unwrap();
        let call = tool_call(&current);
        assert!(tool_pairing_balanced_before(&current, call).unwrap());
        assert!(!tool_pairing_balanced_after(&current, call).unwrap());
        let result = append(
            &current,
            "tool/result",
            serde_json::json!({
                "turn":1, "step":1, "message": {"id":"result", "role":"user", "content":[
                    {"type":"tool-result", "toolCallId":"call-1", "content":[], "isError":false}
                ], "source":{"kind":"tool", "callId":"call-1"}}
            }),
        );
        assert!(!tool_pairing_balanced_before(&current, result).unwrap());
        assert!(tool_pairing_balanced_after(&current, result).unwrap());
        assert!(tool_pairing_balanced_after(&old, user).unwrap());
    }

    #[test]
    fn retiring_sessions_release_balance_caches_without_another_lookup() {
        for _ in 0..100 {
            let session = Session::create(session_id("retired-id"), None, None, None).unwrap();
            let seq = tool_call(&session);
            assert!(!tool_pairing_balanced_after(&session, seq).unwrap());
            let cache = session.derived_cache::<Mutex<BalanceCache>>();
            let weak = std::sync::Arc::downgrade(&cache);
            drop(cache);
            assert!(weak.upgrade().is_some());
            drop(session);
            assert!(
                weak.upgrade().is_none(),
                "retired session retained a global balance cache"
            );
        }
    }
}
