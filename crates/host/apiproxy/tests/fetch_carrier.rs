//! Rust port of the carrier-layer behaviors of
//! `packages/host/apiproxy/tests/fetch-carrier.spec.ts` (HTTP status
//! discipline + envelope handling + SSE framing), exercised against a
//! stub [`ApiProxyCarrier`].

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use http::{Method, StatusCode};

use dsh_host_apiproxy::{
    AbortSignal, ApiProxyCarrier, Body, CarrierRequest, ClientResponse, DownloadResponse,
    FrameRequest, RpcReceipt, RpcRequest, RpcResponse, RpcResult, SessionLogQuery,
    to_fetch_handler,
};

struct StubApi {
    invoked: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
    response: std::sync::Mutex<RpcResponse<serde_json::Value>>,
}

impl StubApi {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            invoked: std::sync::Mutex::new(Vec::new()),
            response: std::sync::Mutex::new(RpcResponse {
                rpc_id: dsh_host_apiproxy::rpc_id("stub"),
                result: RpcResult::ok(serde_json::json!({"ok": true})),
            }),
        })
    }

    fn set_response(&self, response: RpcResponse<serde_json::Value>) {
        *self.response.lock().unwrap() = response;
    }
}

#[async_trait]
impl ApiProxyCarrier for StubApi {
    async fn invoke(
        &self,
        method: &str,
        request: RpcRequest<serde_json::Value>,
        _signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        self.invoked
            .lock()
            .unwrap()
            .push((method.to_string(), request.payload.clone()));
        let mut response = self.response.lock().unwrap().clone();
        response.rpc_id = request.rpc_id;
        response
    }

    fn events_mux(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = FrameRequest> + Send>> {
        let frame = FrameRequest {
            rpc_id: request.rpc_id,
            payload: serde_json::json!({"type": "session/event", "n": 1}),
        };
        let stream = futures::stream::unfold(
            (Some(frame), signal),
            |(frame, signal)| async move {
                if signal.aborted() {
                    return None;
                }
                match frame {
                    Some(frame) => Some((frame, (None, signal))),
                    None => None,
                }
            },
        );
        Box::pin(stream)
    }

    fn events_host(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = FrameRequest> + Send>> {
        let stream = futures::stream::unfold(
            (signal, request),
            |(signal, _request)| async move {
                if signal.aborted() {
                    return None;
                }
                None
            },
        );
        Box::pin(stream)
    }

    async fn respond(&self, _response: ClientResponse) -> RpcReceipt {
        RpcReceipt::Accepted {
            accepted: dsh_host_apiproxy::True,
        }
    }

    async fn session_log(
        &self,
        query: SessionLogQuery,
        _signal: AbortSignal,
    ) -> DownloadResponse {
        DownloadResponse {
            status: StatusCode::OK,
            headers: vec![("content-type".to_string(), "application/zip".to_string())],
            body: Some(format!("log for {}", query.session_id).into_bytes()),
        }
    }
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn post(path: &str, body: Option<&str>) -> CarrierRequest {
    CarrierRequest {
        method: Method::POST,
        path: path.to_string(),
        query: vec![],
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: body.map(|text| text.as_bytes().to_vec()),
    }
}

fn envelope(method: &str, rpc_id: &str, payload: serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": payload,
    }))
    .expect("envelope")
}

#[test]
fn unknown_paths_and_wrong_methods_are_404() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let response = handler
            .handle(CarrierRequest {
                method: Method::POST,
                path: "/nope".to_string(),
                query: vec![],
                headers: vec![],
                body: None,
            })
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = handler
            .handle(CarrierRequest {
                method: Method::GET,
                path: "/api/session.list".to_string(),
                query: vec![],
                headers: vec![],
                body: None,
            })
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = handler
            .handle(post(
                "/api/session.unknown",
                Some(&envelope("session.unknown", "r1", serde_json::json!({}))),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    });
}

#[test]
fn non_json_media_type_is_415_and_non_json_body_is_400() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let response = handler
            .handle(CarrierRequest {
                method: Method::POST,
                path: "/api/session.list".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
                body: Some(b"{}".to_vec()),
            })
            .await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let response = handler
            .handle(post("/api/session.list", Some("not json")))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    });
}

#[test]
fn unary_call_dispatches_with_the_envelope_and_answers_200() {
    run(async {
        let api = StubApi::new();
        let handler = to_fetch_handler(api.clone());
        let body = envelope("session.list", "r1", serde_json::json!({"cwd": "/x"}));
        let response = handler.handle(post("/api/session.list", Some(&body))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["type"], "server-response");
        assert_eq!(parsed["rpcId"], "r1");
        assert_eq!(parsed["result"]["ok"], true);
        assert_eq!(parsed["result"]["value"]["ok"], true);

        let invoked = api.invoked.lock().unwrap();
        assert_eq!(invoked.len(), 1);
        assert_eq!(invoked[0].0, "session.list");
        assert_eq!(invoked[0].1["cwd"], "/x");
    });
}

#[test]
fn mismatched_method_is_200_with_a_bad_request_error() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let body = envelope("session.create", "r1", serde_json::json!({}));
        let response = handler.handle(post("/api/session.list", Some(&body))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["result"]["ok"], false);
        assert_eq!(parsed["result"]["error"]["code"], "bad-request");
        assert!(parsed["result"]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("does not match path"));
    });
}

#[test]
fn malformed_envelope_salvages_rpc_id_or_uses_the_sentinel() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        // Not even an envelope, but a salvageable rpcId.
        let response = handler
            .handle(post("/api/session.list", Some(r#"{"rpcId":"r9"}"#)))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["rpcId"], "r9");
        assert_eq!(parsed["result"]["error"]["code"], "bad-request");

        // Unreadable id: the fixed sentinel keeps the response valid.
        let response = handler
            .handle(post("/api/session.list", Some(r#"{"rpcId":42}"#)))
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["rpcId"], "invalid-request");
        assert_eq!(parsed["result"]["error"]["code"], "bad-request");
    });
}

#[test]
fn respond_returns_a_plain_receipt_not_an_rpc_message() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-response",
            "rpcId": "r2",
            "result": { "ok": true, "value": null },
        }))
        .expect("body");
        let response = handler.handle(post("/api/respond", Some(&body))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        assert_eq!(
            String::from_utf8(bytes).expect("utf8"),
            r#"{"accepted":true}"#
        );

        // A malformed client response is a bad-response receipt.
        let response = handler
            .handle(post("/api/respond", Some(r#"{"nope":1}"#)))
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        assert_eq!(
            String::from_utf8(bytes).expect("utf8"),
            r#"{"accepted":false,"reason":"bad-response"}"#
        );
    });
}

#[test]
fn sse_channels_open_with_a_comment_and_frame_as_server_requests() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let response = handler
            .handle(CarrierRequest {
                method: Method::GET,
                path: "/api/events.mux".to_string(),
                query: vec![],
                headers: vec![],
                body: None,
            })
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .map(|value| value.to_str().expect("header")),
            Some("text/event-stream")
        );
        assert_eq!(
            response
                .headers()
                .get("cache-control")
                .map(|value| value.to_str().expect("header")),
            Some("no-cache")
        );
        let Body::Stream(stream) = response.into_body() else {
            panic!("SSE answers are stream bodies");
        };
        let chunks: Vec<Vec<u8>> = futures::StreamExt::collect(stream).await;
        let bytes: Vec<u8> = chunks.into_iter().flatten().collect();
        let text = String::from_utf8(bytes).expect("utf8");
        // Open comment first, then exactly one frame.
        assert!(text.starts_with(": connected\n\n"), "{text}");
        let data_lines: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("data: "))
            .collect();
        assert_eq!(data_lines.len(), 1);
        let frame: serde_json::Value =
            serde_json::from_str(data_lines[0].trim_start_matches("data: ")).expect("frame");
        assert_eq!(frame["type"], "server-request");
        assert_eq!(frame["method"], "session/event");
        assert_eq!(frame["payload"]["n"], 1);
    });
}

#[test]
fn session_export_forwards_the_query_and_serves_head_empty() {
    run(async {
        let handler = to_fetch_handler(StubApi::new());
        let request = CarrierRequest {
            method: Method::GET,
            path: "/api/session.export".to_string(),
            query: vec![("sessionId".to_string(), "s1".to_string())],
            headers: vec![],
            body: None,
        };
        let response = handler.handle(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .map(|value| value.to_str().expect("header")),
            Some("application/zip")
        );
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        assert_eq!(String::from_utf8(bytes).expect("utf8"), "log for s1");

        // HEAD: headers only, no body.
        let request = CarrierRequest {
            method: Method::HEAD,
            path: "/api/session.export".to_string(),
            query: vec![("sessionId".to_string(), "s1".to_string())],
            headers: vec![],
            body: None,
        };
        let response = handler.handle(request).await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        assert!(bytes.is_empty(), "HEAD carries no body");

        // Missing query: 400.
        let request = CarrierRequest {
            method: Method::GET,
            path: "/api/session.export".to_string(),
            query: vec![],
            headers: vec![],
            body: None,
        };
        let response = handler.handle(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    });
}
