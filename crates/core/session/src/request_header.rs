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
    let keep_defaults = header
        .adapter_defaults
        .as_ref()
        .is_some_and(|defaults| defaults.reasoning_effort == Some(true) || defaults.max_tokens == Some(true));
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
    let reasoning_matches = a_defaults.and_then(|d| d.reasoning_effort)
        == b_defaults.and_then(|d| d.reasoning_effort);
    let max_tokens_matches = a_defaults.and_then(|d| d.max_tokens)
        == b_defaults.and_then(|d| d.max_tokens);
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::LlmCallConfig;

    fn header(system: Option<&str>, tools: usize) -> EpochHeader {
        EpochHeader {
            config: LlmCallConfig {
                provider: "p".to_string(),
                model: "m".to_string(),
                ..Default::default()
            },
            adapter_defaults: None,
            system: system.map(str::to_string),
            tools: if tools == 0 {
                None
            } else {
                Some(
                    (0..tools)
                        .map(|index| ToolSchema {
                            name: format!("tool-{index}"),
                            description: "d".to_string(),
                            parameters: serde_json::json!({"type": "object"}),
                        })
                        .collect(),
                )
            },
        }
    }

    #[test]
    fn canonical_form_drops_empty_optionals() {
        let canonical = canonical_header(&header(None, 0));
        assert!(canonical.system.is_none());
        assert!(canonical.tools.is_none());

        let with_system = header(Some(""), 0);
        let canonical = canonical_header(&with_system);
        assert!(canonical.system.is_none(), "empty system prompt becomes absent");

        let with_content = header(Some("be helpful"), 2);
        let canonical = canonical_header(&with_content);
        assert_eq!(canonical.system.as_deref(), Some("be helpful"));
        assert_eq!(canonical.tools.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn header_equality() {
        let a = header(Some("s"), 1);
        let b = header(Some("s"), 1);
        assert!(header_equals(&a, &b));

        let different_system = header(Some("other"), 1);
        assert!(!header_equals(&a, &different_system));

        let no_system = header(None, 1);
        assert!(!header_equals(&a, &no_system));

        let different_tools = header(Some("s"), 2);
        assert!(!header_equals(&a, &different_tools));
    }

    #[test]
    fn fold_tracks_latest_snapshot() {
        let events = vec![
            SessionEvent {
                type_: "request/header".to_string(),
                seq: 0,
                time: 0,
                data: serde_json::json!({
                    "header": {
                        "config": {"provider": "p", "model": "first"},
                        "system": "one",
                    },
                    "reason": "initial",
                }),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "turn/start".to_string(),
                seq: 1,
                time: 0,
                data: serde_json::json!({"turn": 1}),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
            SessionEvent {
                type_: "request/header".to_string(),
                seq: 2,
                time: 0,
                data: serde_json::json!({
                    "header": {
                        "config": {"provider": "p", "model": "second"},
                    },
                    "reason": "change",
                }),
                ignorable: None,
                surface_op: None,
                source_event_seqs: None,
            },
        ];
        let folded = fold_request_header(&events, None).unwrap();
        assert_eq!(folded.config.model, "second");
        assert!(folded.system.is_none(), "second header drops the system prompt");
    }
}
