//! Composition-layer `session.attachment` over the real fetch carrier:
//! session-reference authorization and the image read.

use std::sync::Arc;

use cordis::Context;
use dsh_attachment::{
    AttachmentError, AttachmentStore, ImageAttachmentLimits, ImageAttachmentRef,
    ImageMediaType, SaveImageAttachment, StoredImageAttachment,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, SessionEvent, SessionStore, SurfaceOp, session_id,
};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Attachment store serving one fixed image.
struct StubStore {
    reference: ImageAttachmentRef,
    data: Vec<u8>,
}

#[async_trait::async_trait]
impl AttachmentStore for StubStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        static LIMITS: std::sync::OnceLock<ImageAttachmentLimits> =
            std::sync::OnceLock::new();
        LIMITS.get_or_init(|| ImageAttachmentLimits {
            max_image_bytes: 1024 * 1024,
            max_images_per_message: 8,
            max_message_image_bytes: 8 * 1024 * 1024,
            max_image_pixels: 4096 * 4096,
            media_types: vec![ImageMediaType::Png, ImageMediaType::Jpeg],
        })
    }

    async fn validate_image(&self, _input: &SaveImageAttachment) -> Result<(), AttachmentError> {
        Ok(())
    }

    async fn save_image(
        &self,
        _input: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        Ok(self.reference.clone())
    }

    async fn read_image(
        &self,
        reference: &ImageAttachmentRef,
        _signal: Option<&dsh_attachment::AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError> {
        if reference != &self.reference {
            return Err(AttachmentError {
                code: "ATTACHMENT_MISSING".to_string(),
                message: "missing".to_string(),
            });
        }
        Ok(StoredImageAttachment {
            reference: self.reference.clone(),
            data: self.data.clone(),
        })
    }
}

fn image_event(seq: u64, attachment_id: &str) -> SessionEvent {
    SessionEvent {
        type_: "user/message".to_string(),
        seq,
        time: seq as i64,
        data: serde_json::json!({
            "id": "u0",
            "role": "user",
            "source": { "kind": "user" },
            "content": [{
                "type": "image",
                "attachment": {
                    "attachmentId": attachment_id,
                    "mediaType": "image/png",
                    "bytes": 3,
                    "width": 1,
                    "height": 1
                }
            }],
        }),
        ignorable: None,
        surface_op: Some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    sessions: Arc<SessionStore>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let reference = ImageAttachmentRef {
            attachment_id: dsh_attachment::attachment_id("img-1"),
            media_type: ImageMediaType::Png,
            bytes: 3,
            width: 1,
            height: 1,
            name: None,
        };
        let store: Arc<dyn AttachmentStore> = Arc::new(StubStore {
            reference,
            data: vec![1, 2, 3],
        });
        ctx.register_service(store);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            sessions,
        }
    }

    async fn seed(&self, id: &str, events: Vec<SessionEvent>) {
        let _ = self
            .sessions
            .create(
                &self._ctx,
                Some(session_id(id)),
                Some(CreateSessionOptions {
                    seed: Some(events),
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");
    }

    async fn post(&self, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "session.attachment",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.attachment".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        serde_json::from_slice(&bytes).expect("json")
    }
}

#[test]
fn reads_a_referenced_image_as_base64() {
    run(async {
        let harness = Harness::new();
        harness.seed("img-src", vec![image_event(0, "img-1")]).await;
        let response = harness
            .post(serde_json::json!({ "sessionId": "img-src", "attachmentId": "img-1" }))
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        let value = &response["result"]["value"];
        assert_eq!(value["attachment"]["attachmentId"], "img-1");
        assert_eq!(value["attachment"]["mediaType"], "image/png");
        // base64 of [1, 2, 3].
        assert_eq!(value["data"], "AQID");
    });
}

#[test]
fn an_unreferenced_image_is_attachment_error() {
    run(async {
        let harness = Harness::new();
        harness.seed("img-src", vec![image_event(0, "img-1")]).await;
        let response = harness
            .post(serde_json::json!({ "sessionId": "img-src", "attachmentId": "ghost" }))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "attachment-error");
        assert_eq!(
            response["result"]["error"]["details"]["reason"],
            "ATTACHMENT_NOT_REFERENCED"
        );
    });
}
