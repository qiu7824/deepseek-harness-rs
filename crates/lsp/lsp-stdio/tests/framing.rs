use dsh_lsp_stdio::{MessageDecoder, encode_message};
use serde_json::json;

#[test]
fn reassembles_a_frame_split_across_chunks() {
    let frame = encode_message(&json!({ "id": 1, "result": "ok" })).expect("encode");
    let mut decoder = MessageDecoder::new(1_000);

    assert!(decoder.push(&frame[..9]).expect("first chunk").is_empty());
    assert_eq!(
        decoder.push(&frame[9..]).expect("second chunk"),
        vec![json!({ "id": 1, "result": "ok" })]
    );
}

#[test]
fn decodes_multiple_frames_from_one_chunk() {
    let mut chunk = encode_message(&json!({ "id": 1 })).expect("first");
    chunk.extend(encode_message(&json!({ "id": 2 })).expect("second"));
    let mut decoder = MessageDecoder::new(1_000);

    assert_eq!(
        decoder.push(&chunk).expect("decode"),
        vec![json!({ "id": 1 }), json!({ "id": 2 })]
    );
}

#[test]
fn rejects_a_declared_body_over_the_limit_before_buffering_it() {
    let mut decoder = MessageDecoder::new(4);
    let error = decoder
        .push(b"Content-Length: 1000000\r\n\r\n")
        .expect_err("oversized frame must reject");

    assert!(
        error.to_string().contains("exceeds the 4-byte limit"),
        "{error}"
    );
}

#[test]
fn rejects_an_unterminated_header_before_its_buffer_can_grow_without_bound() {
    let mut decoder = MessageDecoder::new(1_000_000);
    let error = decoder
        .push(&vec![b'x'; 64 * 1024 + 1])
        .expect_err("unterminated oversized header must reject");

    assert!(
        error.to_string().contains("header exceeded 64 KiB"),
        "{error}"
    );
}
