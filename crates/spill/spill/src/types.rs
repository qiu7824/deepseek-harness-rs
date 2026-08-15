//! Vocabulary for the spill storage Service Definition. Rust port of
//! `packages/spill/spill/src/types.ts`. Types only — the abstract service
//! lives in [`crate::index`], implementations in sibling packages
//! (`dsh-spill-local` first).

use dsh_brand::Branded;
use dsh_llm::CallId;
use dsh_session::SessionId;

/// Marker for the spill locator brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum SpillLocatorTag {}

/// Opaque model-facing handle for one spilled artifact (TS `SpillLocator`).
/// A local backend may use a filesystem path; a remote or database backend
/// may use a URI or key. Consumers render it with [`SpillRef::retrieval_hint`]
/// but do not parse it.
pub type SpillLocator = Branded<SpillLocatorTag>;

/// Brand a string as a [`SpillLocator`].
pub fn spill_locator(locator: impl Into<String>) -> SpillLocator {
    SpillLocator::new(locator)
}

/// Save-time storage namespace for a spilled artifact (TS `SpillOwner`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpillOwner {
    pub session_id: SessionId,
}

/// Tool and call that produced one spilled artifact — recorded by the backend
/// for a readable filename and inspection; purely descriptive, never
/// interpreted for access control (TS `SpillSource`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpillSource {
    /// The tool whose result was spilled (e.g. `web_fetch`).
    pub tool_name: String,
    /// The model-issued call id the result belongs to.
    pub call_id: CallId,
    /// A short human label for the artifact (e.g. `result`).
    pub label: String,
}

/// One request to persist text to a spill artifact (TS `SaveTextSpill`).
#[derive(Debug, Clone, PartialEq)]
pub struct SaveTextSpill {
    pub owner: SpillOwner,
    pub source: SpillSource,
    /// A caller-suggested base name (e.g. `web_fetch.txt`); the backend
    /// sanitizes it to a single safe path segment before use — it is a hint,
    /// never a path.
    pub suggested_name: String,
    /// The full text to persist (UTF-8).
    pub content: String,
}

/// A saved spill artifact: its locator, byte length, and backend-specific
/// retrieval guidance (TS `SpillRef`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpillRef {
    pub locator: SpillLocator,
    pub bytes: u64,
    pub retrieval_hint: String,
}
