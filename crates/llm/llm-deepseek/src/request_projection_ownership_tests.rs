use super::*;

fn request(content: Vec<dsh_llm::ContentBlock>) -> GenerateOptions {
    GenerateOptions {
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
        temperature: Some(0.4),
        max_tokens: Some(8192),
        stop: Some(vec!["END".into()]),
        signal: Some(Arc::new(|| false)),
        session_id: Some("projection-fixture".into()),
        purpose: None,
        agent_loop_request: true,
        telemetry: None,
    }
}

fn text_address(options: &GenerateOptions) -> *const u8 {
    match &options.messages[0].content[0] {
        dsh_llm::ContentBlock::Text { text } => text.as_ptr(),
        _ => panic!("expected fixture text"),
    }
}

#[test]
fn text_projection_moves_then_borrows_the_same_payload_and_preserves_wire_request() {
    let options = request(vec![dsh_llm::ContentBlock::Text {
        text: "x".repeat(1024 * 1024),
    }]);
    let address = text_address(&options);
    let encode = |options: &GenerateOptions| {
        serialize::serialize_request_with_prepared_images(
            options,
            &RequestDefaults::default(),
            ReasoningWireFormat::OpenAi,
            None,
            None,
            None,
        )
        .unwrap()
    };
    let expected = encode(&options);
    let estimated = project_estimated_request(options).unwrap();
    assert_eq!(
        text_address(&estimated),
        address,
        "text-only estimation copied its owned context"
    );
    let image_meta = std::collections::HashMap::new();
    for representation in [
        dsh_llm::RequestImageRepresentation::Raw,
        dsh_llm::RequestImageRepresentation::Base64,
    ] {
        let exact = project_exact_request(&estimated, &image_meta, representation);
        assert!(
            matches!(exact, std::borrow::Cow::Borrowed(_)),
            "text-only exact projection allocated a second request"
        );
        assert_eq!(text_address(&exact), address);
        assert_eq!(encode(&exact), expected);
        assert!(exact.agent_loop_request);
        assert_eq!(exact.session_id.as_deref(), Some("projection-fixture"));
        assert!(!(exact.signal.as_ref().unwrap())());
        require_durable_image_offload(&estimated, &exact).unwrap();
    }
}

fn image(index: usize, bytes: u64) -> dsh_llm::ContentBlock {
    dsh_llm::ContentBlock::Image {
        attachment: dsh_llm::ImageAttachmentRef {
            attachment_id: format!("image-{index}"),
            media_type: Some("image/png".into()),
            bytes: Some(bytes),
            width: Some(100),
            height: Some(100),
            name: None,
        },
    }
}

#[test]
fn owned_estimation_still_requires_durable_offload_before_a_session_request() {
    let options = request(
        (0..DEFAULT_MAX_IMAGES_PER_REQUEST + 1)
            .map(|index| image(index, 1))
            .collect(),
    );
    let error = match project_estimated_request(options) {
        Ok(_) => panic!("over-limit images must not disappear only in the adapter request"),
        Err(error) => error,
    };
    assert_eq!(error.code, "IMAGE_OFFLOAD_REQUIRED");
    assert_eq!(
        error.offload_images,
        Some(DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM)
    );
}

#[test]
fn exact_projection_preserves_byte_quanta_newest_images_and_metadata() {
    let options = request((0..3).map(|index| image(index, 1)).collect());
    // 9 MiB encodes exactly to 12 MiB. With 10 MiB, base64 padding
    // makes the excess slightly greater than 20 MiB, so the existing
    // 10 MiB removal quantum advances to 30 MiB and removes all 3.
    for (bytes, expected_retained) in [(9 * 1024 * 1024, 1), (10 * 1024 * 1024, 0)] {
        let metadata = (0..3)
            .map(|index| {
                let id = format!("image-{index}");
                (
                    id.clone(),
                    serialize::PreparedImageMeta {
                        attachment_id: id,
                        bytes,
                        width: 100,
                        height: 100,
                    },
                )
            })
            .collect();
        let projected = project_exact_request(
            &options,
            &metadata,
            dsh_llm::RequestImageRepresentation::Base64,
        );
        assert!(matches!(projected, std::borrow::Cow::Owned(_)));
        let retained = request_image_attachments(&projected);
        assert_eq!(retained.len(), expected_retained);
        if expected_retained == 1 {
            assert_eq!(retained[0].attachment_id, "image-2");
        }
        assert_eq!(projected.temperature, options.temperature);
        assert_eq!(projected.max_tokens, options.max_tokens);
        assert_eq!(projected.stop, options.stop);
        assert_eq!(projected.session_id, options.session_id);
        assert_eq!(projected.agent_loop_request, options.agent_loop_request);
        assert!(Arc::ptr_eq(
            projected.signal.as_ref().unwrap(),
            options.signal.as_ref().unwrap()
        ));
        assert_eq!(
            request_image_attachments(&options).len(),
            3,
            "projection must not mutate the original request"
        );
        let error = require_durable_image_offload(&options, &projected).unwrap_err();
        assert_eq!(error.code, "IMAGE_OFFLOAD_REQUIRED");
        assert_eq!(error.offload_images, Some(3 - expected_retained));
    }
}
