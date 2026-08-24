//! Server side of the fetch carrier: maps an ApiProxy carrier onto a pure
//! request → response function. Two-level parse: full form
//! (type/rpcId/method + path==method) → payload dispatched per method. HTTP
//! status expresses only the carrier (404 unknown path / 415 non-JSON media
//! type / 400 non-JSON body / 500 handler crash); business errors are
//! always 200 + ServerResponse.
//!
//! Rust port of `packages/host/apiproxy/src/fetch/handler.ts`.
//!
//! # Deviations
//!
//! - The WHATWG `Request`/`Response` pair collapses to a
//!   [`CarrierRequest`]/[`CarrierResponse`] pair (`Body` is plain bytes or
//!   a byte stream for SSE channels).
//! - The composition layer (`createApiProxy`) is a later milestone, so the
//!   carrier consumes the [`ApiProxyCarrier`] trait; the concrete
//!   composition implements it when it lands.
//! - Per-method payload schemas (the second parse level) arrive with the
//!   domain schema milestone; until then the dispatch accepts any JSON
//!   payload and the composition layer owns payload validation.
//! - Mid-stream impl failures surface as frames from the composition layer
//!   (Rust streams are infallible); the TS catch arm emitting a
//!   `stream/error` frame is the composition layer's responsibility.
//! - Repeated `sessionId` query params keep the first value (TS
//!   `Object.fromEntries` keeps the last; the wire schema milestone owns
//!   the exact boundary).

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use http::{Method, Response, StatusCode};
use serde::{Deserialize, Serialize};

use crate::api::rpc::{
    BadRequestDetails, ClientRequest, ClientRequestType, ClientResponse, False, RpcError,
    RpcErrorBody, RpcId, RpcMessage, RpcReceipt, RpcReceiptReason, RpcRequest, RpcResponse,
    RpcResult, ServerRequest, ServerRequestType, WireRpcResult, rpc_id,
};

/// The response body: plain bytes for unary answers, or a byte stream for
/// SSE channels.
pub enum Body {
    Bytes(Vec<u8>),
    Stream(Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>),
}

impl From<Vec<u8>> for Body {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<String> for Body {
    fn from(value: String) -> Self {
        Self::Bytes(value.into_bytes())
    }
}

/// A fully formed carrier response.
pub type CarrierResponse = Response<Body>;

/// The session-export query boundary (the domain schema milestone fills
/// this in; the handler only forwards it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionLogQuery {
    pub session_id: String,
    pub include_descendants: Option<bool>,
}

/// The download answer handed back by the composition layer.
pub struct DownloadResponse {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
}

/// One SSE-capable event channel frame: a server-initiated push whose
/// narrow request form the carrier completes (method = payload type).
pub type FrameRequest = RpcRequest<serde_json::Value>;

/// The host-side capability the fetch carrier drives. The composition
/// layer (`createApiProxy`) implements this when it lands.
#[async_trait]
pub trait ApiProxyCarrier: Send + Sync {
    /// Invoke one client-request method (method ∈
    /// [`crate::api::rpc_map::CLIENT_REQUEST_METHODS`]).
    async fn invoke(
        &self,
        method: &str,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value>;

    /// The mux event channel (GET /api/events.mux).
    fn events_mux(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = FrameRequest> + Send>>;

    /// The host event channel (GET /api/events.host).
    fn events_host(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> Pin<Box<dyn Stream<Item = FrameRequest> + Send>>;

    /// Answer a pending server request (POST /api/respond).
    async fn respond(&self, response: ClientResponse) -> RpcReceipt;

    /// The host-only session download (GET/HEAD /api/session.export).
    async fn session_log(&self, query: SessionLogQuery, signal: AbortSignal) -> DownloadResponse;
}

/// Clonable, abortable cancellation flag standing in for the TS
/// `AbortSignal` parameter (the caller/connection lifetime). The concrete
/// carrier shell wires connection drops into this.
#[derive(Clone, Default)]
pub struct AbortSignal {
    inner: std::sync::Arc<AbortSignalInner>,
}

struct AbortSignalInner {
    aborted: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl Default for AbortSignalInner {
    fn default() -> Self {
        Self {
            aborted: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.inner
            .aborted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn aborted(&self) -> bool {
        self.inner.aborted.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolve once the signal aborts (immediately when already aborted).
    pub async fn cancelled(&self) {
        loop {
            if self.aborted() {
                return;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// Sentinel rpcId for error responses to envelopes whose own rpcId is
/// unreadable: the response must still be a valid ServerResponse. Fixed
/// value, documented here as wire contract.
pub const INVALID_REQUEST_RPC_ID: &str = "invalid-request";

/// Wrap a business error as a ServerResponse full form (rpcId backfilled;
/// an unreadable rpcId uses the invalid-request sentinel).
fn error_response(rpc_id: RpcId, error: RpcError) -> CarrierResponse {
    json_message(RpcMessage::ServerResponse {
        rpc_id,
        result: WireRpcResult::Err { ok: False, error },
    })
}

/// Complete the impl's narrow form into a ServerResponse full form.
fn full_response(narrow: RpcResponse<serde_json::Value>) -> CarrierResponse {
    json_message(RpcMessage::ServerResponse {
        rpc_id: narrow.rpc_id,
        result: match narrow.result {
            RpcResult::Ok { ok, value } => WireRpcResult::Ok {
                ok,
                value: Some(value),
            },
            RpcResult::Err { ok, error } => WireRpcResult::Err { ok, error },
        },
    })
}

fn json_message(message: RpcMessage) -> CarrierResponse {
    let bytes = serde_json::to_vec(&message).expect("rpc messages serialize");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Bytes(bytes))
        .expect("carrier response")
}

fn text_response(status: StatusCode, text: &str) -> CarrierResponse {
    Response::builder()
        .status(status)
        .body(Body::Bytes(text.as_bytes().to_vec()))
        .expect("carrier response")
}

/// SSE frame: complete the narrow `RpcRequest<frame>` into a ServerRequest
/// full form (method = frame type).
fn full_frame(narrow: FrameRequest) -> ServerRequest {
    let method = narrow
        .payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    ServerRequest {
        kind: ServerRequestType::ServerRequest,
        rpc_id: narrow.rpc_id,
        method,
        payload: narrow.payload,
    }
}

/// Serialize one frame as an SSE `data:` record.
fn encode_frame(frame: &ServerRequest) -> Vec<u8> {
    let json = serde_json::to_string(frame).expect("frames serialize");
    format!("data: {json}\n\n").into_bytes()
}

/// The SSE open comment line so clients/proxies see a live channel while
/// idle (not a frame, so client frame parsing skips it naturally).
const SSE_OPEN_LINE: &[u8] = b": connected\n\n";

/// Wrap a frame stream as an SSE body; stops when the signal aborts.
fn sse_response(
    frames: Pin<Box<dyn Stream<Item = FrameRequest> + Send>>,
    signal: AbortSignal,
) -> CarrierResponse {
    use futures::StreamExt;

    let stream = futures::stream::unfold((frames, signal), |(mut frames, signal)| async move {
        if signal.aborted() {
            return None;
        }
        match frames.next().await {
            Some(narrow) => {
                let bytes = encode_frame(&full_frame(narrow));
                Some((bytes, (frames, signal)))
            }
            None => None,
        }
    });
    // Leading open comment + the frame stream.
    let with_open =
        futures::stream::once(async { Ok(SSE_OPEN_LINE.to_vec()) }).chain(stream.map(Ok));
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::Stream(Box::pin(with_open)))
        .expect("carrier response")
}

/// Route lookup: narrow an arbitrary path segment to a registered
/// client-request method.
fn method_for(path: &str) -> Option<&'static str> {
    let methods = crate::api::rpc_map::CLIENT_REQUEST_METHODS;
    if let Ok(index) = methods.binary_search(&path) {
        return Some(methods[index]);
    }
    let canonical = path.replace('/', ".");
    methods
        .binary_search(&canonical.as_str())
        .ok()
        .map(|index| methods[index])
}

/// Invoke one unary route: payload validity is the composition layer's
/// second parse (later milestone); an impl crash is 500, carrier layer.
async fn handle_unary(
    api: std::sync::Arc<dyn ApiProxyCarrier>,
    method: &'static str,
    message: ClientRequest,
    signal: AbortSignal,
) -> CarrierResponse {
    let spawned = tokio::spawn(async move {
        api.invoke(
            method,
            RpcRequest {
                rpc_id: message.rpc_id.clone(),
                payload: message.payload,
            },
            signal,
        )
        .await
    });
    match spawned.await {
        Ok(response) => full_response(response),
        // The impl never returns business errors; reaching here means the
        // implementation itself crashed — 500, carrier layer.
        Err(_) => text_response(StatusCode::INTERNAL_SERVER_ERROR, "handler failure"),
    }
}

/// A minimal incoming-request view: method, path, query, headers, body.
#[derive(Debug, Clone)]
pub struct CarrierRequest {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl CarrierRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Wrap an ApiProxy carrier into a pure request → response function (the
/// isomorphic point: feed the returned function to the in-process client).
/// Paths outside /api/ return 404.
pub fn to_fetch_handler(api: std::sync::Arc<dyn ApiProxyCarrier>) -> FetchHandler {
    FetchHandler { api }
}

/// The handler closure form.
pub struct FetchHandler {
    api: std::sync::Arc<dyn ApiProxyCarrier>,
}

impl FetchHandler {
    pub async fn handle(&self, request: CarrierRequest) -> CarrierResponse {
        let path = request.path.as_str();

        // No-envelope read channels (SSE GET streams + host-only download):
        // physical routes that answer directly, without a wire envelope.
        if path == "/api/events.mux" && request.method == Method::GET {
            let signal = AbortSignal::new();
            let frames = self.api.events_mux(
                RpcRequest {
                    rpc_id: rpc_id(uuid()),
                    payload: serde_json::json!({}),
                },
                signal.clone(),
            );
            return sse_response(frames, signal);
        }
        if path == "/api/events.host" && request.method == Method::GET {
            let signal = AbortSignal::new();
            let frames = self.api.events_host(
                RpcRequest {
                    rpc_id: rpc_id(uuid()),
                    payload: serde_json::json!({}),
                },
                signal.clone(),
            );
            return sse_response(frames, signal);
        }
        if path == "/api/session.export"
            && (request.method == Method::GET || request.method == Method::HEAD)
        {
            let Some((_, session_id)) = request.query.iter().find(|(key, _)| key == "sessionId")
            else {
                return text_response(
                    StatusCode::BAD_REQUEST,
                    "missing or invalid sessionId query parameter",
                );
            };
            let include_descendants = request
                .query
                .iter()
                .find(|(key, _)| key == "includeDescendants")
                .and_then(|(_, value)| value.parse::<bool>().ok());
            let response = self
                .api
                .session_log(
                    SessionLogQuery {
                        session_id: session_id.clone(),
                        include_descendants,
                    },
                    AbortSignal::new(),
                )
                .await;
            let mut builder = Response::builder().status(response.status);
            for (name, value) in response.headers {
                builder = builder.header(name, value);
            }
            let body = if request.method == Method::GET {
                response.body.unwrap_or_else(|| Body::Bytes(Vec::new()))
            } else {
                Body::Bytes(Vec::new())
            };
            return builder.body(body).expect("carrier response");
        }

        if request.method != Method::POST || !path.starts_with("/api/") {
            return text_response(StatusCode::NOT_FOUND, "not found");
        }

        // Cross-site write fence: only the JSON media type is accepted;
        // anything else is forced into a preflight this server never
        // answers. 415 = carrier layer, like the 400 below.
        let media_type = request.header("content-type").map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_lowercase()
        });
        if media_type.as_deref() != Some("application/json") {
            return text_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "content type must be application/json",
            );
        }

        // 400 = carrier layer (body is not even JSON); valid JSON with a
        // bad shape goes 200 + bad-request.
        let Some(bytes) = &request.body else {
            return text_response(StatusCode::BAD_REQUEST, "body is not JSON");
        };
        let body: serde_json::Value = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(_) => return text_response(StatusCode::BAD_REQUEST, "body is not JSON"),
        };

        if path == "/api/respond" {
            let Ok(response) = serde_json::from_value::<ClientResponse>(body.clone()) else {
                return receipt_response(RpcReceipt::Rejected {
                    accepted: False,
                    reason: RpcReceiptReason::BadResponse,
                });
            };
            let receipt = self.api.respond(response).await;
            return receipt_response(receipt);
        }

        let Some(method) = method_for(path.strip_prefix("/api/").unwrap_or_default()) else {
            return text_response(StatusCode::NOT_FOUND, "not found");
        };

        let message: ClientRequest = match serde_json::from_value::<RpcMessage>(body.clone()) {
            Ok(RpcMessage::ClientRequest {
                rpc_id,
                method: envelope_method,
                payload,
            }) => {
                let canonical_envelope = envelope_method.replace('/', ".");
                if canonical_envelope != method {
                    return error_response(
                        rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: format!(
                                "method \"{envelope_method}\" does not match path \"{method}\""
                            ),
                            details: BadRequestDetails { issues: vec![] },
                        }),
                    );
                }
                ClientRequest {
                    kind: ClientRequestType::ClientRequest,
                    rpc_id,
                    method: method.to_string(),
                    payload,
                }
            }
            _ => {
                // Best effort at correlation: salvage a string rpcId
                // from the raw body; otherwise the fixed sentinel keeps
                // the response a valid ServerResponse.
                let raw_id = body.get("rpcId").and_then(serde_json::Value::as_str);
                let rpc_id = raw_id
                    .map(|id| rpc_id(id.to_string()))
                    .unwrap_or_else(|| rpc_id(INVALID_REQUEST_RPC_ID));
                return error_response(
                    rpc_id,
                    RpcError::BadRequest(RpcErrorBody {
                        message: "invalid client-request message".to_string(),
                        details: BadRequestDetails { issues: vec![] },
                    }),
                );
            }
        };

        handle_unary(self.api.clone(), method, message, AbortSignal::new()).await
    }
}

/// The carrier receipt is NOT an `RpcMessage` — it answers as its own
/// plain JSON shape (TS `Response.json(await api.respond(...))`).
fn receipt_response(receipt: RpcReceipt) -> CarrierResponse {
    let bytes = serde_json::to_vec(&receipt).expect("receipts serialize");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Bytes(bytes))
        .expect("carrier response")
}

/// Mint a fresh correlation id (uuid v4 shaped; the composition layer may
/// substitute a counter).
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let ptr = std::ptr::addr_of!(bytes) as u64;
    let mut state = nanos ^ ptr;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 8) as u8
    };
    for byte in bytes.iter_mut() {
        *byte = next();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}
