//! Tool-pairing balance over a session surface. Rust port of
//! `packages/compaction/compaction/src/tool-pairing.ts`.
//!
//! # Deviations
//!
//! - The TS `WeakMap<Session, BalanceCache>` becomes a global map keyed by
//!   the session pointer (the established weak-ish key convention; sessions
//!   live for the process lifetime in practice).

use std::collections::HashMap;
use std::sync::Arc;

use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;

#[derive(Clone)]
struct BalanceCache {
    generation: u64,
    cut_balanced: Vec<bool>,
    index_by_seq: HashMap<u64, usize>,
    in_progress_tool_calls: i64,
}

static BALANCE_CACHE: std::sync::OnceLock<Mutex<HashMap<String, BalanceCache>>> =
    std::sync::OnceLock::new();

fn balance_cache_by_session() -> &'static Mutex<HashMap<String, BalanceCache>> {
    BALANCE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

fn balance_cache(session: &Session) -> Result<BalanceCache, String> {
    let surface = session.surface().map_err(|error| format!("{error}"))?;
    let seqs = surface.nodes;
    let generation = surface.replace_generation;
    // Keyed by session id: stable per store and unique per session (the TS
    // WeakMap identity convention).
    let key = session.id().as_str().to_string();
    let mut caches = balance_cache_by_session().lock();
    let cached = caches.get(&key).cloned();
    let mut rebuilt = match cached {
        Some(cached)
            if cached.generation == generation
                && cached.cut_balanced.len().saturating_sub(1) <= seqs.len() =>
        {
            cached
        }
        _ => BalanceCache {
            generation,
            cut_balanced: vec![true],
            index_by_seq: HashMap::new(),
            in_progress_tool_calls: 0,
        },
    };
    if rebuilt.cut_balanced.len() - 1 < seqs.len() {
        let events = session.events();
        extend_cache(&events, &mut rebuilt, &seqs)?;
    }
    caches.insert(key, rebuilt.clone());
    Ok(rebuilt)
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
    let cache = balance_cache(session)?;
    cut_balance(&cache, seq, 0)
}

/// Whether the cut immediately after a current surface sequence is
/// tool-pairing balanced (TS `toolPairingBalancedAfter`).
pub fn tool_pairing_balanced_after(session: &Session, seq: u64) -> Result<bool, String> {
    let cache = balance_cache(session)?;
    cut_balance(&cache, seq, 1)
}
