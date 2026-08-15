//! Abstract credential seam (`ctx.credentials`). Rust port of
//! `@deepseek-ai/dsh-credentials`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{CredentialInfo, CredentialProvider, ResolvedCredential, credential_ref};
pub use types::{CredentialRef, CredentialRefTag};
