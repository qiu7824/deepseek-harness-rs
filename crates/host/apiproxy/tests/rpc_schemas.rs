//! Rust port of the core `packages/host/apiproxy/tests/rpc-schemas.spec.ts`
//! behaviors: the four wire full forms, every error-code branch with its
//! required details, result-branch hybrid rejection, and the carrier
//! receipt.

use dsh_host_apiproxy::{
    ClientRequest, ClientRequestType, ClientResponse, ClientResponseType, EmptyDetails, RpcError,
    RpcErrorBody, RpcErrorCode, RpcId, RpcMessage, RpcReceipt, RpcReceiptReason, RpcResult,
    ServerRequest, ServerRequestType, ServerResponse, ServerResponseType, True, WireRpcResult,
    rpc_id, transport_error,
};

#[test]
fn brands_a_raw_string_and_roundtrips_through_the_schema() {
    let id = rpc_id("abc");
    assert_eq!(id.as_str(), "abc");
    // No min-length: the id is an opaque echo token.
    let empty: RpcId = serde_json::from_str("\"\"").expect("empty id parses");
    assert_eq!(empty.as_str(), "");
    assert!(
        serde_json::from_str::<RpcId>("42").is_err(),
        "non-string id fails"
    );
}

#[test]
fn folds_thrown_values_into_the_internal_error_branch() {
    let folded: RpcResult<()> = transport_error("wire down");
    assert!(matches!(
        folded.error(),
        Some(RpcError::Internal(RpcErrorBody { message, .. })) if message == "wire down"
    ));
    let folded: RpcResult<()> = transport_error("raw");
    assert_eq!(
        folded.error().map(RpcError::code),
        Some(RpcErrorCode::Internal)
    );
}

#[test]
fn accepts_every_error_code_branch_with_its_required_details() {
    let cases: &[(&str, &str)] = &[
        (
            r#"{"code":"bad-request","message":"m","details":{"issues":[]}}"#,
            "bad-request",
        ),
        (
            r#"{"code":"cancelled","message":"m","details":{}}"#,
            "cancelled",
        ),
        (
            r#"{"code":"session-not-found","message":"m","details":{"sessionId":"s"}}"#,
            "session-not-found",
        ),
        (
            r#"{"code":"model-unavailable","message":"m","details":{"provider":"p","model":"m"}}"#,
            "model-unavailable",
        ),
        (
            r#"{"code":"session-conflict","message":"m","details":{"sessionId":"s","requestedCwd":"/a","existingCwd":"/b"}}"#,
            "session-conflict",
        ),
        (
            r#"{"code":"session-conflict","message":"m","details":{"sessionId":"s","requestedCwd":"/a"}}"#,
            "session-conflict",
        ),
        (
            r#"{"code":"invalid-time-zone","message":"m","details":{"value":"CST"}}"#,
            "invalid-time-zone",
        ),
        (
            r#"{"code":"workspace-attach-failed","message":"m","details":{"sessionId":"s","workspaceId":"w"}}"#,
            "workspace-attach-failed",
        ),
        (
            r#"{"code":"workspace-not-found","message":"m","details":{"workspaceId":"w"}}"#,
            "workspace-not-found",
        ),
        (
            r#"{"code":"workspace-invalid-path","message":"m","details":{"path":"/x"}}"#,
            "workspace-invalid-path",
        ),
        (
            r#"{"code":"workspace-name-conflict","message":"m","details":{"name":"x"}}"#,
            "workspace-name-conflict",
        ),
        (
            r#"{"code":"workspace-move-invalid","message":"m","details":{"workspaceId":"w","sessionId":"s"}}"#,
            "workspace-move-invalid",
        ),
        (
            r#"{"code":"directory-unreadable","message":"m","details":{"path":"/x"}}"#,
            "directory-unreadable",
        ),
        (
            r#"{"code":"directory-exists","message":"m","details":{"path":"/x"}}"#,
            "directory-exists",
        ),
        (
            r#"{"code":"directory-create-failed","message":"m","details":{"path":"/x"}}"#,
            "directory-create-failed",
        ),
        (
            r#"{"code":"directory-picker-unavailable","message":"m","details":{"capability":"native"}}"#,
            "directory-picker-unavailable",
        ),
        (
            r#"{"code":"agent-preset-read-only","message":"m","details":{"agentPreset":"p","reason":"r"}}"#,
            "agent-preset-read-only",
        ),
        (
            r#"{"code":"agent-preset-locked","message":"m","details":{"sessionId":"s","agentPreset":"p"}}"#,
            "agent-preset-locked",
        ),
        (
            r#"{"code":"agent-preset-conflict","message":"m","details":{"sessionId":"s","requestedPreset":"p"}}"#,
            "agent-preset-conflict",
        ),
        (
            r#"{"code":"agent-preset-not-found","message":"m","details":{"agentPreset":"p","available":[]}}"#,
            "agent-preset-not-found",
        ),
        (
            r#"{"code":"agent-preset-invalid","message":"m","details":{"agentPreset":"p","reason":"r"}}"#,
            "agent-preset-invalid",
        ),
        (
            r#"{"code":"agent-busy","message":"m","details":{"reason":"r"}}"#,
            "agent-busy",
        ),
        (
            r#"{"code":"attachment-error","message":"m","details":{"reason":"r"}}"#,
            "attachment-error",
        ),
        (
            r#"{"code":"queue-item-not-found","message":"m","details":{"itemId":"i"}}"#,
            "queue-item-not-found",
        ),
        (
            r#"{"code":"steer-unavailable","message":"m","details":{"itemId":"i"}}"#,
            "steer-unavailable",
        ),
        (
            r#"{"code":"command-error","message":"m","details":{}}"#,
            "command-error",
        ),
        (
            r#"{"code":"unknown-command","message":"m","details":{}}"#,
            "unknown-command",
        ),
        (
            r#"{"code":"settings-rejected","message":"m","details":{"ns":"n"}}"#,
            "settings-rejected",
        ),
        (
            r#"{"code":"settings-not-exposed","message":"m","details":{"ns":"n"}}"#,
            "settings-not-exposed",
        ),
        (
            r#"{"code":"settings-conflict","message":"m","details":{"ns":"n","expected":1,"actual":2}}"#,
            "settings-conflict",
        ),
        (
            r#"{"code":"credential-rejected","message":"m","details":{"ref":"r"}}"#,
            "credential-rejected",
        ),
        (
            r#"{"code":"model-discovery-failed","message":"m","details":{"settingsNs":"n"}}"#,
            "model-discovery-failed",
        ),
        (
            r#"{"code":"model-discovery-failed","message":"m","details":{"settingsNs":"n","baseURL":"http://x"}}"#,
            "model-discovery-failed",
        ),
        (
            r#"{"code":"title-invalid","message":"m","details":{"sessionId":"s"}}"#,
            "title-invalid",
        ),
        (
            r#"{"code":"fork-unavailable","message":"m","details":{"sessionId":"s"}}"#,
            "fork-unavailable",
        ),
        (
            r#"{"code":"subagent-parent-unavailable","message":"m","details":{"parentSessionId":"p"}}"#,
            "subagent-parent-unavailable",
        ),
        (
            r#"{"code":"subagent-not-found","message":"m","details":{"parentSessionId":"p","childSessionId":"c"}}"#,
            "subagent-not-found",
        ),
        (
            r#"{"code":"subagent-catalog-diagnostic","message":"m","details":{"parentSessionId":"p","childSessionId":"c","reason":"corrupt"}}"#,
            "subagent-catalog-diagnostic",
        ),
        (
            r#"{"code":"subagent-not-resumable","message":"m","details":{"childSessionId":"c"}}"#,
            "subagent-not-resumable",
        ),
        (
            r#"{"code":"subagent-unauthorized","message":"m","details":{"childSessionId":"c"}}"#,
            "subagent-unauthorized",
        ),
        (
            r#"{"code":"subagent-delivery-unavailable","message":"m","details":{"childSessionId":"c"}}"#,
            "subagent-delivery-unavailable",
        ),
        (
            r#"{"code":"internal","message":"m","details":{}}"#,
            "internal",
        ),
    ];
    for (json, code) in cases {
        let parsed: RpcError = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
        assert_eq!(parsed.code().as_str(), *code, "{json}");
        // Every branch roundtrips.
        let back = serde_json::to_string(&parsed).expect("serialize");
        let again: RpcError = serde_json::from_str(&back).expect("reparse");
        assert_eq!(again, parsed, "{json}");
    }
}

#[test]
fn rejects_a_known_code_with_missing_details_and_unknown_codes() {
    for json in [
        r#"{"code":"agent-busy","message":"m","details":{}}"#,
        r#"{"code":"title-invalid","message":"m","details":{}}"#,
        r#"{"code":"command-error","message":"m"}"#,
        r#"{"code":"nope","message":"m","details":{}}"#,
    ] {
        assert!(
            serde_json::from_str::<RpcError>(json).is_err(),
            "must reject {json}"
        );
    }
}

#[test]
fn result_accepts_both_branches_and_rejects_hybrids() {
    let ok: RpcResult<serde_json::Value> =
        serde_json::from_str(r#"{"ok":true,"value":{"n":1}}"#).expect("ok branch");
    assert!(ok.is_ok());
    assert_eq!(ok.value().expect("value")["n"], 1);

    let err: RpcResult<serde_json::Value> = serde_json::from_str(
        r#"{"ok":false,"error":{"code":"internal","message":"m","details":{}}}"#,
    )
    .expect("err branch");
    assert_eq!(
        err.error().map(RpcError::code),
        Some(RpcErrorCode::Internal)
    );

    // Hybrids: ok true without a value, ok true with an error instead.
    assert!(
        serde_json::from_str::<RpcResult<serde_json::Value>>(r#"{"ok":true,"error":{}}"#).is_err()
    );
    assert!(serde_json::from_str::<RpcResult<serde_json::Value>>(r#"{"ok":true}"#).is_err());
}

#[test]
fn wide_result_omits_the_value_field_for_void_results() {
    let wide: WireRpcResult =
        serde_json::from_str(r#"{"ok":true}"#).expect("void result has no value field");
    assert!(matches!(&wide, WireRpcResult::Ok { value: None, .. }));
    let serialized = serde_json::to_string(&wide).expect("serialize");
    assert_eq!(serialized, r#"{"ok":true}"#, "value field stays absent");
}

#[test]
fn four_wire_full_forms_discriminate_on_type() {
    let request = RpcMessage::ClientRequest {
        rpc_id: rpc_id("r1"),
        method: "session.list".to_string(),
        payload: serde_json::json!({}),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains(r#""type":"client-request""#), "{json}");
    let back: RpcMessage = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, request);

    let response = RpcMessage::ServerResponse {
        rpc_id: rpc_id("r1"),
        result: WireRpcResult::Ok {
            ok: True,
            value: Some(serde_json::json!({"n": 1})),
        },
    };
    let json = serde_json::to_string(&response).expect("serialize");
    assert!(json.contains(r#""type":"server-response""#), "{json}");
    let back: RpcMessage = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, response);

    let push = RpcMessage::ServerRequest {
        rpc_id: rpc_id("r2"),
        method: "session.event".to_string(),
        payload: serde_json::json!({}),
    };
    let json = serde_json::to_string(&push).expect("serialize");
    assert!(json.contains(r#""type":"server-request""#), "{json}");

    let answer = RpcMessage::ClientResponse {
        rpc_id: rpc_id("r2"),
        result: WireRpcResult::Ok {
            ok: True,
            value: None,
        },
    };
    let json = serde_json::to_string(&answer).expect("serialize");
    assert!(json.contains(r#""type":"client-response""#), "{json}");

    // Unknown type fails.
    assert!(
        serde_json::from_str::<RpcMessage>(
            r#"{"type":"nope","rpcId":"r","method":"m","payload":{}}"#
        )
        .is_err()
    );
}

#[test]
fn narrow_forms_carry_rpc_id_and_payload() {
    let request = dsh_host_apiproxy::RpcRequest {
        rpc_id: rpc_id("r1"),
        payload: serde_json::json!({"cwd": "/x"}),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert_eq!(
        json, r#"{"rpcId":"r1","payload":{"cwd":"/x"}}"#,
        "rpcId stays explicit, camelCase on the wire"
    );
    let back: dsh_host_apiproxy::RpcRequest<serde_json::Value> =
        serde_json::from_str(&json).expect("reparse");
    assert_eq!(back.rpc_id.as_str(), "r1");
}

#[test]
fn carrier_receipt_accepts_and_rejects() {
    let accepted: RpcReceipt = serde_json::from_str(r#"{"accepted":true}"#).expect("accepted");
    assert!(matches!(accepted, RpcReceipt::Accepted { .. }));
    let rejected: RpcReceipt =
        serde_json::from_str(r#"{"accepted":false,"reason":"not-pending"}"#).expect("rejected");
    assert!(matches!(
        rejected,
        RpcReceipt::Rejected {
            reason: RpcReceiptReason::NotPending,
            ..
        }
    ));
    let bad: RpcReceipt =
        serde_json::from_str(r#"{"accepted":false,"reason":"bad-response"}"#).expect("bad");
    assert!(matches!(
        bad,
        RpcReceipt::Rejected {
            reason: RpcReceiptReason::BadResponse,
            ..
        }
    ));
    assert!(serde_json::from_str::<RpcReceipt>(r#"{"accepted":false,"reason":"nope"}"#).is_err());
    assert!(serde_json::from_str::<RpcReceipt>(r#"{"accepted":false}"#).is_err());
}

#[test]
fn typed_standalone_forms_serialize_with_their_literals() {
    let form = ClientRequest {
        kind: ClientRequestType::ClientRequest,
        rpc_id: rpc_id("r"),
        method: "m".to_string(),
        payload: serde_json::Value::Null,
    };
    let json = serde_json::to_string(&form).expect("serialize");
    assert_eq!(
        json,
        r#"{"type":"client-request","rpcId":"r","method":"m","payload":null}"#
    );

    let _ = ServerResponse {
        kind: ServerResponseType::ServerResponse,
        rpc_id: rpc_id("r"),
        result: WireRpcResult::Ok {
            ok: True,
            value: None,
        },
    };
    let _ = ServerRequest {
        kind: ServerRequestType::ServerRequest,
        rpc_id: rpc_id("r"),
        method: "m".to_string(),
        payload: serde_json::Value::Null,
    };
    let _ = ClientResponse {
        kind: ClientResponseType::ClientResponse,
        rpc_id: rpc_id("r"),
        result: WireRpcResult::Ok {
            ok: True,
            value: None,
        },
    };
    let _ = EmptyDetails {};
}
