//! Encode an owned JSON response as bounded chunks on its consumer's thread.

use serde_json::Value;

struct StringFragment;

impl serde_json::ser::Formatter for StringFragment {
    fn begin_string<W: ?Sized + std::io::Write>(&mut self, _: &mut W) -> std::io::Result<()> {
        Ok(())
    }

    fn end_string<W: ?Sized + std::io::Write>(&mut self, _: &mut W) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) const CHUNK_BYTES: usize = 64 * 1024;

enum Pending {
    Literal(&'static [u8]),
    Value(Value),
    String { text: String, offset: usize },
    Array(std::vec::IntoIter<Value>, bool),
    Object(serde_json::map::IntoIter, bool),
}

/// The stack owns the unencoded portion of the response. Polling produces at
/// most one chunk; dropping the iterator releases all remaining values without
/// starting a producer task or waiting for a blocked channel.
pub(super) struct JsonBody {
    pending: Vec<Pending>,
}

impl JsonBody {
    pub(super) fn history(rpc_id: String, value: Value) -> Self {
        // Match RpcMessage::ServerResponse and WireRpcResult::Ok field order.
        // The payload itself retains serde_json::Map's iteration order.
        Self {
            pending: vec![
                Pending::Literal(b"}}"),
                Pending::Value(value),
                Pending::Literal(b",\"result\":{\"ok\":true,\"value\":"),
                Pending::Value(Value::String(rpc_id)),
                Pending::Literal(b"{\"type\":\"server-response\",\"rpcId\":"),
            ],
        }
    }
}

impl Iterator for JsonBody {
    type Item = Result<Vec<u8>, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pending.is_empty() {
            return None;
        }
        let mut bytes = Vec::with_capacity(CHUNK_BYTES);
        // Non-string tokens are at most 128 bytes. Strings use the remaining
        // capacity's worst-case JSON escape expansion, including their quotes.
        while bytes.len() <= CHUNK_BYTES - 128 {
            let Some(pending) = self.pending.pop() else {
                break;
            };
            match pending {
                Pending::Literal(text) => bytes.extend_from_slice(text),
                Pending::Value(Value::Null) => bytes.extend_from_slice(b"null"),
                Pending::Value(Value::Bool(true)) => bytes.extend_from_slice(b"true"),
                Pending::Value(Value::Bool(false)) => bytes.extend_from_slice(b"false"),
                Pending::Value(Value::Number(number)) => {
                    serde_json::to_writer(&mut bytes, &number).expect("JSON number serializes");
                }
                Pending::Value(Value::String(text)) => {
                    bytes.push(b'"');
                    self.pending.push(Pending::String { text, offset: 0 });
                }
                Pending::Value(Value::Array(values)) => {
                    bytes.push(b'[');
                    self.pending.push(Pending::Array(values.into_iter(), true));
                }
                Pending::Value(Value::Object(values)) => {
                    bytes.push(b'{');
                    self.pending.push(Pending::Object(values.into_iter(), true));
                }
                Pending::Array(mut values, first) => {
                    if let Some(value) = values.next() {
                        if !first {
                            bytes.push(b',');
                        }
                        self.pending.push(Pending::Array(values, false));
                        self.pending.push(Pending::Value(value));
                    } else {
                        bytes.push(b']');
                    }
                }
                Pending::Object(mut values, first) => {
                    if let Some((key, value)) = values.next() {
                        if !first {
                            bytes.push(b',');
                        }
                        self.pending.push(Pending::Object(values, false));
                        self.pending.push(Pending::Value(value));
                        self.pending.push(Pending::Literal(b":"));
                        self.pending.push(Pending::Value(Value::String(key)));
                    } else {
                        bytes.push(b'}');
                    }
                }
                Pending::String { text, offset } => {
                    let mut end = text.len().min(offset + (CHUNK_BYTES - bytes.len() - 1) / 6);
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    // Delegate all escaping to the same serializer as the
                    // previous unary body. UTF-8 boundaries ensure every
                    // fragment is a complete string; suppress only its quotes
                    // and write directly into this chunk without a scratch Vec.
                    serde::Serialize::serialize(
                        &text[offset..end],
                        &mut serde_json::Serializer::with_formatter(&mut bytes, StringFragment),
                    )
                    .expect("JSON string serializes");
                    if end == text.len() {
                        bytes.push(b'"');
                    } else {
                        self.pending.push(Pending::String { text, offset: end });
                    }
                }
            }
        }
        debug_assert!(bytes.len() <= CHUNK_BYTES);
        Some(Ok(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::rpc::{RpcMessage, True, WireRpcResult, rpc_id};
    use serde_json::json;

    fn assert_wire(value: Value) {
        let id = "history-\"\\\n会话😀";
        let expected = serde_json::to_vec(&RpcMessage::ServerResponse {
            rpc_id: rpc_id(id),
            result: WireRpcResult::Ok {
                ok: True,
                value: Some(value.clone()),
            },
        })
        .unwrap();
        let mut actual = Vec::new();
        for chunk in JsonBody::history(id.into(), value) {
            let chunk = chunk.unwrap();
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= CHUNK_BYTES);
            assert_eq!(chunk.capacity(), CHUNK_BYTES);
            actual.extend(chunk);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn bounded_chunks_match_serde_for_unicode_escapes_and_numeric_extremes() {
        let scalars: String = (0..=0x10ffff).filter_map(char::from_u32).collect();
        assert_wire(json!({
            "text": scalars,
            "controls": (0..=31).map(char::from).collect::<String>().repeat(4096),
            "numbers": [u64::MAX, i64::MIN, -0.0, 1.0e-300, 1.0e300],
            "empty": [{}, [], "", null, true, false],
            "objects": [{"z": ["\\\"/", {"\n\t": "😀"}]}, {"a": 1}],
        }));
    }

    #[test]
    fn polling_does_not_preencode_or_consume_the_remaining_page() {
        let text = "x".repeat(4 * 1024 * 1024);
        let ptr = text.as_ptr();
        let mut body = JsonBody::history("backpressure".into(), Value::String(text));
        assert!(
            matches!(&body.pending[1], Pending::Value(Value::String(text)) if text.as_ptr() == ptr)
        );
        assert!(body.next().unwrap().unwrap().len() <= CHUNK_BYTES);
        assert!(
            matches!(body.pending.last(), Some(Pending::String { text, offset }) if text.as_ptr() == ptr && *offset < CHUNK_BYTES)
        );
        // Cancellation owns no producer, channel or extra copy of this payload.
        drop(body);
    }
}
