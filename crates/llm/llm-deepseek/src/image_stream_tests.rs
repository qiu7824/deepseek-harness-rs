use super::*;
use dsh_attachment::*;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

struct LazyImage {
    remaining: usize,
    reads: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
impl AsyncRead for LazyImage {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let n = buffer.remaining().min(self.remaining).min(1001);
        buffer.initialize_unfilled_to(n)[..n].fill(0x5a);
        buffer.advance(n);
        self.remaining -= n;
        Poll::Ready(Ok(()))
    }
}
impl Drop for LazyImage {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct LazyStore {
    limits: ImageAttachmentLimits,
    reads: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl AttachmentStore for LazyStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }
    async fn validate_image(&self, _: &SaveImageAttachment) -> Result<(), AttachmentError> {
        unreachable!()
    }
    async fn save_image(
        &self,
        _: &SaveImageAttachment,
    ) -> Result<ImageAttachmentRef, AttachmentError> {
        unreachable!()
    }
    async fn read_image(
        &self,
        _: &ImageAttachmentRef,
        _: Option<&AttachmentAbort>,
    ) -> Result<StoredImageAttachment, AttachmentError> {
        panic!("provider must use the stream interface")
    }
    async fn open_image_request(
        &self,
        reference: &ImageAttachmentRef,
        policy: &RequestImagePolicy,
        _: Option<&AttachmentAbort>,
    ) -> Result<RequestImageStream, AttachmentError> {
        Ok(RequestImageStream {
            attachment_id: reference.attachment_id.clone(),
            variant_id: request_image_variant_id(reference, policy),
            media_type: ImageMediaType::Webp,
            width: 100,
            height: 100,
            bytes: 1_000_001,
            reader: Box::pin(LazyImage {
                remaining: 1_000_001,
                reads: self.reads.clone(),
                drops: self.drops.clone(),
            }),
        })
    }
    async fn open_image(
        &self,
        reference: &ImageAttachmentRef,
        _: Option<&AttachmentAbort>,
    ) -> Result<ImageAttachmentStream, AttachmentError> {
        Ok(ImageAttachmentStream {
            reference: reference.clone(),
            reader: Box::pin(LazyImage {
                remaining: reference.bytes as usize,
                reads: self.reads.clone(),
                drops: self.drops.clone(),
            }),
        })
    }
}
fn setup(
    session: bool,
) -> (
    GenerateOptions,
    Arc<dyn AttachmentStore>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let reads = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let store: Arc<dyn AttachmentStore> = Arc::new(LazyStore {
        reads: reads.clone(),
        drops: drops.clone(),
        limits: ImageAttachmentLimits {
            max_image_bytes: 0,
            max_message_image_bytes: 0,
            max_image_pixels: 40_000_000,
            max_images_per_message: 20,
            media_types: vec![ImageMediaType::Png],
        },
    });
    let content = (0..20)
        .map(|i| dsh_llm::ContentBlock::Image {
            attachment: dsh_llm::ImageAttachmentRef {
                attachment_id: format!("sha256:{i:064x}"),
                media_type: Some("image/png".into()),
                bytes: Some(1_000_001),
                width: Some(100),
                height: Some(100),
                name: None,
            },

            offloaded: None,
        })
        .collect();
    let options = GenerateOptions {
        provider: "fixture".into(),
        model: "fixture".into(),
        reasoning_effort: None,
        messages: vec![dsh_llm::create_user_message(
            content,
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )],
        system: None,
        tools: None,
        temperature: None,
        max_tokens: Some(128),
        stop: None,
        signal: None,
        session_id: session.then(|| "owned-session".into()),
        purpose: None,
        agent_loop_request: false,
        telemetry: None,
    };
    (options, store, reads, drops)
}
#[tokio::test]
async fn native_computer_screenshot_preserves_original_pixels_and_codec() {
    let (mut options, store, reads, _) = setup(false);
    let mut image = options.messages[0].content[0].clone();
    let dsh_llm::ContentBlock::Image { attachment, .. } = &mut image else {
        panic!("image fixture")
    };
    attachment.width = Some(1920);
    attachment.height = Some(1080);
    let id = attachment.attachment_id.clone();
    options.messages = vec![
        dsh_llm::create_message(
            dsh_llm::Role::Assistant,
            vec![dsh_llm::ContentBlock::ToolCall {
                id: dsh_llm::call_id("native-call"),
                name: dsh_llm::computer_protocol::TOOL_NAME.into(),
                arguments: "{}".into(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ),
        dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
            call_id: dsh_llm::call_id("native-call"),
            is_error: false,
            content: vec![image],
        }),
    ];
    let (urls, metadata) = resolve_image_urls(&options, Some(&store)).await.unwrap();
    assert_eq!((metadata[&id].width, metadata[&id].height), (1920, 1080));
    assert!(urls[&id].starts_with("data:image/png;base64,"));
    assert!(reads.load(Ordering::SeqCst) > 0);
}
#[tokio::test]
async fn durable_offload_is_decided_before_any_inline_image_bytes_are_read() {
    let (options, store, reads, drops) = setup(true);
    let failure = resolve_image_urls(&options, Some(&store))
        .await
        .unwrap_err();
    assert_eq!(failure.code, "IMAGE_OFFLOAD_REQUIRED");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 20);
}
#[tokio::test]
async fn inline_only_reads_retained_images_and_preserves_partial_base64_chunks() {
    use base64::Engine;
    let (options, store, reads, drops) = setup(false);
    let (urls, metadata) = resolve_image_urls(&options, Some(&store)).await.unwrap();
    assert_eq!(metadata.len(), 20);
    assert!(!urls.is_empty() && urls.len() < 20);
    assert!(reads.load(Ordering::SeqCst) > 0);
    assert_eq!(drops.load(Ordering::SeqCst), 20);
    for url in urls.values() {
        let data = base64::engine::general_purpose::STANDARD
            .decode(url.split_once(',').unwrap().1)
            .unwrap();
        assert_eq!(data.len(), 1_000_001);
        assert!(data.iter().all(|byte| *byte == 0x5a));
    }
}
