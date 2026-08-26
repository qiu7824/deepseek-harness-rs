//! Read-only enumeration of durable subagent children and descendant trees
//! straight from the live session store and optional session persistence.
//! Rust port of `packages/subagent/subagent/src/list-children.ts`.
//!
//! # Deviations
//!
//! - The abort predicate replaces `AbortSignal`.
//! - The projection-cache rung uses the cached snapshot when present;
//!   cache-read failures fall through to the authoritative re-fold.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionHeader, SessionId, SessionStore};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_projection::SessionProjectionRegistry;
use dsh_session_projection_cache::SessionProjectionCache;

use crate::error::SubagentError;
use crate::projection::{SubagentIdentityProjection, same_lifecycle};

/// Concurrent cold inspections per listing.
const COLD_READ_CONCURRENCY: usize = 4;

/// One entry of a `list_children` result (TS `SubagentListEntry`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SubagentListEntry {
    Child {
        id: SessionId,
        activity: String,
        #[serde(rename = "hasChildren")]
        has_children: bool,
        #[serde(flatten)]
        identity: SubagentIdentityProjection,
    },
    Diagnostic {
        id: SessionId,
        reason: String,
    },
}

/// One entry of a descendant listing (TS `SubagentDescendantListEntry`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubagentDescendantListEntry {
    #[serde(flatten)]
    pub entry: SubagentListEntry,
    #[serde(rename = "parentId")]
    pub parent_id: SessionId,
    pub depth: u64,
}

#[derive(Clone)]
struct CorpusRecord {
    header: SessionHeader,
    live: Option<Session>,
}

struct ListingRuntime {
    projections: Arc<SessionProjectionRegistry>,
    persistence: Option<Arc<dyn SessionPersistenceApi>>,
    cache: Option<Arc<SessionProjectionCache>>,
    corpus: HashMap<String, CorpusRecord>,
    subagent_parents: HashSet<String>,
}

/// Enumerate one parent's origin-classified direct children.
pub async fn list_children(
    ctx: &Context,
    parent_session_id: &SessionId,
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<Vec<SubagentListEntry>, SubagentError> {
    let listing = prepare_listing(ctx, signal).await?;
    let mut candidates: Vec<&CorpusRecord> = listing
        .corpus
        .values()
        .filter(|record| {
            record.header.parent_session.as_ref() == Some(parent_session_id)
                && record.header.origin.as_deref() == Some("subagent")
        })
        .collect();
    candidates.sort_by(|a, b| compare_corpus_records(a, b));
    let rows = resolve_candidate_rows(candidates, &listing, signal).await?;
    Ok(rows.into_iter().flatten().collect())
}

/// Enumerate every session-backed subagent below one root in stable
/// pre-order.
pub async fn list_descendants(
    ctx: &Context,
    root_session_id: &SessionId,
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<Vec<SubagentDescendantListEntry>, SubagentError> {
    let listing = prepare_listing(ctx, signal).await?;
    let positioned = descendant_candidates(&listing.corpus, root_session_id);
    let rows = resolve_candidate_rows(
        positioned.iter().map(|position| &position.record).collect(),
        &listing,
        signal,
    )
    .await?;
    let mut entries = Vec::new();
    for (position, row) in positioned.iter().zip(rows) {
        if let Some(row) = row {
            entries.push(SubagentDescendantListEntry {
                entry: row,
                parent_id: position.parent_id.clone(),
                depth: position.depth,
            });
        }
    }
    Ok(entries)
}

/// Resolve listing services once and build one live-preferred corpus.
async fn prepare_listing(
    ctx: &Context,
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<ListingRuntime, SubagentError> {
    let projections = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            SubagentError::new(
                "SUBAGENT_CONTROL_PROJECTIONS_UNAVAILABLE",
                "listing subagents requires the sessionProjections registry (load @deepseek-ai/dsh-session-projection)",
            )
        })?;
    let sessions = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| {
            SubagentError::new(
                "SUBAGENT_CONTROL_SESSION_STORE_UNAVAILABLE",
                "listing subagents requires the session store (load @deepseek-ai/dsh-session)",
            )
        })?;
    assert_listing_not_cancelled(signal)?;
    let persistence = ctx
        .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
        .map(|slot| slot.as_ref().clone());
    let cache = ctx
        .get_typed::<Arc<SessionProjectionCache>>("sessionProjectionCache", false)
        .map(|slot| slot.as_ref().clone());
    let mut corpus: HashMap<String, CorpusRecord> = HashMap::new();
    if let Some(persistence) = &persistence {
        match persistence.list().await {
            Ok(headers) => {
                for header in headers {
                    corpus.insert(
                        header.id.as_str().to_string(),
                        CorpusRecord { header, live: None },
                    );
                }
            }
            Err(error) => {
                assert_listing_not_cancelled(signal)?;
                return Err(SubagentError::new("CANCELLED", error));
            }
        }
        assert_listing_not_cancelled(signal)?;
    }
    for session in sessions.list() {
        corpus.insert(
            session.header().id.as_str().to_string(),
            CorpusRecord {
                header: session.header().clone(),
                live: Some(session),
            },
        );
    }
    let mut subagent_parents: HashSet<String> = HashSet::new();
    for record in corpus.values() {
        if record.header.origin.as_deref() == Some("subagent")
            && let Some(parent) = &record.header.parent_session
        {
            subagent_parents.insert(parent.as_str().to_string());
        }
    }
    Ok(ListingRuntime {
        projections,
        persistence,
        cache,
        corpus,
        subagent_parents,
    })
}

/// Resolve projection-backed rows for aligned candidates with bounded cold
/// reads.
async fn resolve_candidate_rows(
    candidates: Vec<&CorpusRecord>,
    listing: &ListingRuntime,
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<Vec<Option<SubagentListEntry>>, SubagentError> {
    let mut rows: Vec<Option<SubagentListEntry>> = Vec::with_capacity(candidates.len());
    let mut cold_reads: Vec<(usize, SessionHeader)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let child_id = candidate.header.id.clone();
        let Some(live) = &candidate.live else {
            cold_reads.push((index, candidate.header.clone()));
            rows.push(None);
            continue;
        };
        // The registry's watermark cache serves the live value with zero log
        // reads; schema failures degrade to one corrupt diagnostic.
        let identity = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            listing.projection_snapshot(live)
        }));
        match identity {
            Ok(Some(identity)) => {
                rows.push(Some(child_row(
                    child_id,
                    identity,
                    "running",
                    listing
                        .subagent_parents
                        .contains(candidate.header.id.as_str()),
                )));
            }
            Ok(None) => rows.push(None),
            Err(_) => rows.push(Some(SubagentListEntry::Diagnostic {
                id: child_id,
                reason: "corrupt".to_string(),
            })),
        }
    }

    if let Some(persistence) = &listing.persistence {
        let mut cold: Vec<(usize, SessionHeader)> = cold_reads;
        let mut resolved: Vec<(usize, SubagentListEntry)> = Vec::new();
        for chunk in cold.chunks_mut(COLD_READ_CONCURRENCY) {
            let jobs: Vec<_> = chunk
                .iter()
                .map(|(index, header)| {
                    let persistence = persistence.clone();
                    let projections = listing.projections.clone();
                    let cache = listing.cache.clone();
                    let subagent_parents = listing.subagent_parents.clone();
                    let signal = signal.cloned();
                    let index = *index;
                    let header = header.clone();
                    async move {
                        let has_children = subagent_parents.contains(header.id.as_str());
                        let row = resolve_cold_identity(
                            &persistence,
                            cache.as_ref(),
                            &header,
                            has_children,
                            signal.as_ref(),
                        )
                        .await;
                        (index, row)
                    }
                })
                .collect();
            for (index, row) in futures::future::join_all(jobs).await {
                resolved.push((index, row));
            }
        }
        for (index, row) in resolved {
            rows[index] = Some(row);
        }
    }
    assert_listing_not_cancelled(signal)?;
    Ok(rows)
}

impl ListingRuntime {
    /// One consistent projection read over a live child; `None` when the
    /// fold served no identity.
    fn projection_snapshot(&self, session: &Session) -> Option<SubagentIdentityProjection> {
        let snapshot = self.projections.snapshot(session);
        let value = snapshot.values.get("subagent")?;
        serde_json::from_value(value.clone()).ok()
    }
}

/// Build origin-classified candidates from the complete tree without
/// recursion.
fn descendant_candidates(
    corpus: &HashMap<String, CorpusRecord>,
    root_session_id: &SessionId,
) -> Vec<PositionedCandidate> {
    let mut children: HashMap<String, Vec<&CorpusRecord>> = HashMap::new();
    for record in corpus.values() {
        let Some(parent) = &record.header.parent_session else {
            continue;
        };
        children
            .entry(parent.as_str().to_string())
            .or_default()
            .push(record);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| compare_corpus_records(a, b));
    }
    let mut positioned: Vec<PositionedCandidate> = Vec::new();
    let mut stack: Vec<PositionedCandidate> = children
        .get(root_session_id.as_str())
        .map(|siblings| {
            siblings
                .iter()
                .rev()
                .map(|record| PositionedCandidate {
                    record: (*record).clone(),
                    parent_id: root_session_id.clone(),
                    depth: 1,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut visited: HashSet<String> = HashSet::from([root_session_id.as_str().to_string()]);
    while let Some(position) = stack.pop() {
        let id = position.record.header.id.as_str().to_string();
        if visited.contains(&id) {
            continue;
        }
        visited.insert(id.clone());
        if position.record.header.origin.as_deref() == Some("subagent") {
            positioned.push(position.clone());
        }
        if let Some(descendants) = children.get(&id) {
            for record in descendants.iter().rev() {
                stack.push(PositionedCandidate {
                    record: (*record).clone(),
                    parent_id: position.record.header.id.clone(),
                    depth: position.depth + 1,
                });
            }
        }
    }
    positioned
}

#[derive(Clone)]
struct PositionedCandidate {
    record: CorpusRecord,
    parent_id: SessionId,
    depth: u64,
}

/// Compare siblings by durable creation time, then id.
fn compare_corpus_records(a: &CorpusRecord, b: &CorpusRecord) -> std::cmp::Ordering {
    a.header
        .created_at
        .cmp(&b.header.created_at)
        .then_with(|| a.header.id.as_str().cmp(b.header.id.as_str()))
}

/// Resolve one cold candidate down the remaining ladder.
async fn resolve_cold_identity(
    persistence: &Arc<dyn SessionPersistenceApi>,
    cache: Option<&Arc<SessionProjectionCache>>,
    header: &SessionHeader,
    has_children: bool,
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> SubagentListEntry {
    let child_id = header.id.clone();
    if let Some(cache) = cache
        && let Ok(Some(snapshot)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.cached_snapshot(header)
        }))
        && let Ok(metadata) = persistence.read_list_metadata(&child_id).await
        && same_lifecycle(&metadata.meta, header)
        && snapshot.as_of_seq == metadata.last_seq
    {
        let cached: Option<SubagentIdentityProjection> = snapshot
            .values
            .get("subagent")
            .and_then(|value| serde_json::from_value(value.clone()).ok());
        return match cached {
            Some(cached) => child_row(child_id, cached, "inactive", has_children),
            None => match snapshot.values.get("title").and_then(serde_json::Value::as_str) {
                Some(title) if !title.trim().is_empty() => child_row(
                    child_id,
                    SubagentIdentityProjection::OneShot {
                        label: Some(title.to_string()),
                        seq: header.seed_length.unwrap_or(0),
                    },
                    "inactive",
                    has_children,
                ),
                _ => SubagentListEntry::Diagnostic {
                    id: child_id,
                    reason: "unsupported".to_string(),
                },
            },
        };
    }
    if let Err(error) = assert_listing_not_cancelled(signal) {
        return SubagentListEntry::Diagnostic {
            id: child_id,
            reason: error.code.to_string(),
        };
    }
    let before = match persistence.read_snapshot(&child_id).await {
        Ok(Some(snapshot)) => snapshot,
        _ => {
            assert_listing_not_cancelled(signal).ok();
            return SubagentListEntry::Diagnostic {
                id: child_id,
                reason: "unavailable".to_string(),
            };
        }
    };
    if !same_lifecycle(&before.header, header) {
        return SubagentListEntry::Diagnostic {
            id: child_id,
            reason: "corrupt".to_string(),
        };
    }
    const IDENTITY_CHUNK_EVENTS: usize = 256;
    let mut from_seq = 0_u64;
    let mut descriptor = None;
    loop {
        if let Err(error) = assert_listing_not_cancelled(signal) {
            return SubagentListEntry::Diagnostic {
                id: child_id,
                reason: error.code.to_string(),
            };
        }
        let chunk = match persistence
            .read_event_chunk(&child_id, from_seq, IDENTITY_CHUNK_EVENTS)
            .await
        {
            Ok(chunk) => chunk,
            Err(_) => {
                return SubagentListEntry::Diagnostic {
                    id: child_id,
                    reason: "unavailable".to_string(),
                };
            }
        };
        descriptor = chunk
            .events
            .into_iter()
            .find(|event| event.type_ == "subagent/descriptor");
        if descriptor.is_some() {
            break;
        }
        match chunk.next_seq {
            Some(next) if next > from_seq => from_seq = next,
            _ => break,
        }
    }
    let after = match persistence.read_snapshot(&child_id).await {
        Ok(Some(snapshot)) => snapshot,
        _ => {
            return SubagentListEntry::Diagnostic {
                id: child_id,
                reason: "unavailable".to_string(),
            };
        }
    };
    if before.revision != after.revision || !same_lifecycle(&after.header, header) {
        return SubagentListEntry::Diagnostic {
            id: child_id,
            reason: "unavailable".to_string(),
        };
    }
    let identity = descriptor
        .as_ref()
        .and_then(crate::projection::descriptor_identity);
    match identity {
        Some(identity) => child_row(child_id, identity, "inactive", has_children),
        None => SubagentListEntry::Diagnostic {
            id: child_id,
            reason: "unsupported".to_string(),
        },
    }
}

/// Materialize one served identity as its child row.
fn child_row(
    id: SessionId,
    identity: SubagentIdentityProjection,
    activity: &str,
    has_children: bool,
) -> SubagentListEntry {
    SubagentListEntry::Child {
        id,
        activity: activity.to_string(),
        has_children,
        identity,
    }
}

/// Stop a listing at its next cancellation checkpoint.
fn assert_listing_not_cancelled(
    signal: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<(), SubagentError> {
    if signal.is_some_and(|signal| signal()) {
        return Err(SubagentError::new(
            "CANCELLED",
            "subagent listing was cancelled",
        ));
    }
    Ok(())
}
