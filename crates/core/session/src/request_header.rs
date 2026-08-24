//! Request-header reconstruction utilities over full `request/header`
//! session events. Rust port of `packages/core/session/src/request-header.ts`.

use dsh_llm::{ToolSchema, call_config_equals};

use crate::types::{EpochHeader, RequestHeaderReason, SessionEvent};

/// The parsed payload of one `request/header` event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestHeaderPayload {
    pub header: EpochHeader,
    pub reason: RequestHeaderReason,
}

/// Normalize a header to canonical form: an empty system prompt and empty
/// tool list become absent fields, matching how requests are built.
pub fn canonical_header(header: &EpochHeader) -> EpochHeader {
    let keep_defaults = header.adapter_defaults.as_ref().is_some_and(|defaults| {
        defaults.reasoning_effort == Some(true) || defaults.max_tokens == Some(true)
    });
    EpochHeader {
        config: header.config.clone(),
        adapter_defaults: if keep_defaults {
            header.adapter_defaults.clone()
        } else {
            None
        },
        system: header
            .system
            .as_ref()
            .filter(|system| !system.is_empty())
            .cloned(),
        tools: header
            .tools
            .as_ref()
            .filter(|tools| !tools.is_empty())
            .cloned(),
    }
}

/// Canonical JSON equality for tool schemas assembled through the same
/// path (TS `sameSchema`).
fn same_schema(a: &ToolSchema, b: &ToolSchema) -> bool {
    a == b
}

/// Field-wise equality over canonical headers. Tool schemas compare in
/// order.
pub fn header_equals(a: &EpochHeader, b: &EpochHeader) -> bool {
    if !call_config_equals(&a.config, &b.config) {
        return false;
    }
    let a_defaults = a.adapter_defaults.as_ref();
    let b_defaults = b.adapter_defaults.as_ref();
    let reasoning_matches =
        a_defaults.and_then(|d| d.reasoning_effort) == b_defaults.and_then(|d| d.reasoning_effort);
    let max_tokens_matches =
        a_defaults.and_then(|d| d.max_tokens) == b_defaults.and_then(|d| d.max_tokens);
    if !reasoning_matches || !max_tokens_matches || a.system != b.system {
        return false;
    }
    match (&a.tools, &b.tools) {
        (Some(left), Some(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| same_schema(left, right))
        }
        (None, None) => true,
        _ => false,
    }
}

/// Extract the latest `request/header` payload from one event.
pub(crate) fn request_header_payload(event: &SessionEvent) -> Option<RequestHeaderPayload> {
    if event.type_ != "request/header" {
        return None;
    }
    serde_json::from_value::<RequestHeaderPayload>(event.data.clone()).ok()
}

/// Fold the header events of a log (or any prefix) into the
/// [`EpochHeader`] in force after the last snapshot.
pub fn fold_request_header(
    events: &[SessionEvent],
    from: Option<EpochHeader>,
) -> Option<EpochHeader> {
    let mut state = from;
    for event in events {
        if let Some(payload) = request_header_payload(event) {
            state = Some(canonical_header(&payload.header));
        }
    }
    state
}
