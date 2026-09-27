//! Non-secret, process-local ownership of one captured authentication route.

use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RequestAuthenticationIdentity {
    Anonymous,
    ApiKey,
    Account {
        #[serde(rename = "authProvider")]
        auth_provider: String,
        #[serde(rename = "accountScope")]
        account_scope: String,
        #[serde(rename = "loginGeneration")]
        login_generation: String,
    },
}

/// Clones retain the revocation boundary even after an account is removed or
/// logged in again. This value contains no credential and is never persisted.
#[derive(Debug, Clone)]
pub struct RequestAuthentication {
    identity: RequestAuthenticationIdentity,
    revoked: Arc<AtomicBool>,
}

impl RequestAuthentication {
    pub fn new(identity: RequestAuthenticationIdentity) -> Self {
        Self {
            identity,
            revoked: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn identity(&self) -> &RequestAuthenticationIdentity {
        &self.identity
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }
}

impl Default for RequestAuthentication {
    fn default() -> Self {
        Self::new(RequestAuthenticationIdentity::Anonymous)
    }
}

pub type RequestAuthenticationObserver =
    Arc<dyn Fn(RequestAuthentication) -> Result<(), crate::LlmError> + Send + Sync>;

/// Capture the initiating turn before asynchronous route preparation. The
/// observer is inherited by auxiliary calls as well as conversation requests.
pub struct RequestAuthenticationObservers {
    pub capture: Arc<dyn Fn() -> Option<RequestAuthenticationObserver> + Send + Sync>,
}
impl cordis::Service for RequestAuthenticationObservers {
    fn service_name(&self) -> &'static str {
        "llmAuthenticationObservers"
    }
}
