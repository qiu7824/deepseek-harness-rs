//! Service Definition for the credential-reference capability seam
//! (`ctx.credentials`). Rust port of
//! `packages/credentials/credentials/src/index.ts`. Settings and composition
//! files carry *references* to secrets — environment-variable names — while
//! providers own the actual values and their storage. Consumers resolve a
//! reference once per operation, so a changed credential reaches the next
//! operation without any plugin restart, and configuration surfaces describe
//! a reference without ever seeing its value.
//!
//! # Deviations
//!
//! - The TS `notifyUpdated` runs listeners synchronously in a contained
//!   dispatch (sync throws caught, async rejections logged, `INVARIANT`
//!   failures rethrown). The Rust [`CredentialProvider::notify_updated`] is
//!   `async` — listeners are futures, so it awaits each contained within
//!   `catch_unwind`; every listener still runs, failures log, and
//!   `INVARIANT` failures surface after all listeners ran.

use std::sync::OnceLock;

use cordis::{Context, DispatchMode, Service, arc};
use dsh_invariants::InvariantError;

use crate::types::{CredentialRef, credential_ref_raw};

/// One resolved credential value and the source layer that supplied it (TS
/// `ResolvedCredential`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCredential {
    /// The non-empty secret value.
    pub value: String,
    /// Provider-defined source layer id (the local provider uses `env`,
    /// `file`, `project-env`, and `user-env`).
    pub source: String,
}

/// Source and writability facts for one reference, safe for configuration
/// UIs — never the value (TS `CredentialInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialInfo {
    /// Whether [`CredentialProvider::resolve`] would currently return a
    /// value.
    pub configured: bool,
    /// Source layer currently supplying the value; absent while
    /// unconfigured.
    pub source: Option<String>,
    /// Whether [`CredentialProvider::set`] would currently succeed for this
    /// reference.
    pub writable: bool,
}

/// The seam's reference-shape rule (TS `REF_PATTERN`): a POSIX shell
/// identifier.
fn ref_pattern() -> &'static regex::Regex {
    static REF: OnceLock<regex::Regex> = OnceLock::new();
    REF.get_or_init(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("REF_PATTERN"))
}

/// Brand a raw string as a [`CredentialRef`], rejecting every other shape
/// (the TS `credentialRef` throw).
pub fn credential_ref(value: &str) -> CredentialRef {
    if !ref_pattern().is_match(value) {
        panic!(
            "credential ref \"{value}\" must match {}",
            ref_pattern().as_str()
        );
    }
    credential_ref_raw(value)
}

/// Abstract credential service (TS `CredentialProvider`). Providers
/// implement the four operations over their source layers; one seam-wide
/// rule binds them all: an empty stored value is absent everywhere —
/// `resolve` skips it, `describe` reports it unconfigured — so a blank never
/// masquerades as a configured secret.
#[async_trait::async_trait]
pub trait CredentialProvider: Send + Sync + 'static {
    /// Resolve one reference to its current value. Resolution is per call:
    /// consumers re-resolve at each operation and must not cache across
    /// operations — that per-operation read is what makes a changed
    /// credential reach the next operation without a restart.
    async fn resolve(&self, reference: &CredentialRef) -> Option<ResolvedCredential>;

    /// Describe one reference for configuration surfaces without exposing
    /// the value.
    async fn describe(&self, reference: &CredentialRef) -> CredentialInfo;

    /// Durably store one value in the provider-managed writable source.
    /// Rejects while a read-only source shadows the reference — the write
    /// would appear to succeed while resolution keeps returning the
    /// shadowing value — and rejects an empty value (use
    /// [`CredentialProvider::unset`]).
    async fn set(&self, reference: &CredentialRef, value: &str) -> Result<(), String>;

    /// Remove one reference from the provider-managed writable source;
    /// removing an absent reference is a no-op. Rejects while a read-only
    /// source shadows the reference, like [`CredentialProvider::set`].
    async fn unset(&self, reference: &CredentialRef) -> Result<(), String>;

    /// Fan `credentials/updated` out with contained listener failures: every
    /// listener runs, and a panic or async rejection is logged without
    /// changing the committed operation's outcome — except `INVARIANT`-coded
    /// failures, which surface as an error after every listener ran.
    /// Providers call this only after the write or reload actually
    /// committed, so a broken observer can never make a durable change look
    /// failed.
    async fn notify_updated(&self, ctx: &Context, reference: &CredentialRef) -> Result<(), String> {
        let args = vec![arc(reference.clone())];
        let mut invariant_failure: Option<String> = None;
        for (listener_ctx, callback) in
            ctx.collect(DispatchMode::Emit, "credentials/updated", &args)
        {
            let future = callback(&listener_ctx, args.clone());
            let outcome =
                futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(future)).await;
            match outcome {
                Ok(_) => {}
                Err(payload) => {
                    let message = match payload.downcast::<InvariantError>() {
                        Ok(error) => {
                            invariant_failure.get_or_insert_with(|| error.to_string());
                            continue;
                        }
                        Err(payload) => match payload.downcast::<String>() {
                            Ok(message) => *message,
                            Err(payload) => match payload.downcast::<&'static str>() {
                                Ok(message) => message.to_string(),
                                Err(payload) => format!("<unknown panic payload {payload:?}>"),
                            },
                        },
                    };
                    ctx.named_logger(None).warn(vec![arc(format!(
                        "credentials: a credentials/updated listener for \"{reference}\" failed: {message}"
                    ))]);
                }
            }
        }
        match invariant_failure {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
}

impl Service for dyn CredentialProvider {
    fn service_name(&self) -> &'static str {
        "credentials"
    }
}
