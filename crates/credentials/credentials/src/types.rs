//! Client-safe type surface of the credential-reference seam: the reference
//! brand. Types only — no runtime code. Rust port of
//! `packages/credentials/credentials/src/types.ts`.

use dsh_brand::Branded;

/// Marker for the credential reference brand.
#[doc(hidden)]
#[allow(dead_code)]
pub enum CredentialRefTag {}

/// Nominal reference to one credential: a POSIX-style environment-variable
/// name (TS `CredentialRef`).
pub type CredentialRef = Branded<CredentialRefTag>;

/// Construct the brand without the seam's runtime validation (used by the
/// [`crate::index::credential_ref`] entry after its pattern check).
pub(crate) fn credential_ref_raw(value: impl Into<String>) -> CredentialRef {
    CredentialRef::new(value)
}
