//! Runtime type, binary, and equality helpers (port of `src/types.ts`).

use std::collections::HashSet;

use serde_json::Value;

/// Binary source detection and base64/hex conversion helpers.
///
/// Port of the TS `Binary` namespace. The TS version juggles
/// `ArrayBuffer`/views; Rust bytes are always contiguous, so `fromSource`
/// collapses to identity.
pub mod binary {
    /// Encode bytes as base64 (standard alphabet with padding).
    pub fn to_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    /// Decode base64, ignoring invalid characters like Node's
    /// `Buffer.from(source, 'base64')`.
    pub fn from_base64(source: &str) -> Vec<u8> {
        fn value(ch: u8) -> Option<u32> {
            match ch {
                b'A'..=b'Z' => Some((ch - b'A') as u32),
                b'a'..=b'z' => Some((ch - b'a' + 26) as u32),
                b'0'..=b'9' => Some((ch - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        let mut out = Vec::new();
        let mut buffer = [0u32; 4];
        let mut count = 0usize;
        for ch in source.bytes() {
            if ch == b'=' {
                // padding: stop decoding further input (JS ignores rest)
                break;
            }
            let Some(v) = value(ch) else { continue };
            buffer[count] = v;
            count += 1;
            if count == 4 {
                let n = (buffer[0] << 18) | (buffer[1] << 12) | (buffer[2] << 6) | buffer[3];
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
                count = 0;
            }
        }
        // Final partial group (2-3 significant chars, no padding).
        match count {
            2 => out.push(((buffer[0] << 2) | (buffer[1] >> 4)) as u8),
            3 => {
                out.push(((buffer[0] << 2) | (buffer[1] >> 4)) as u8);
                out.push(((buffer[1] << 4) | (buffer[2] >> 2)) as u8);
            }
            _ => {}
        }
        out
    }

    /// Encode bytes as lowercase hex.
    pub fn to_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// Decode hex, mirroring the TS leniency: an odd trailing character is
    /// dropped and invalid characters decode to zero.
    pub fn from_hex(source: &str) -> Vec<u8> {
        let hex = if source.len().is_multiple_of(2) {
            source.to_string()
        } else {
            source[..source.len() - 1].to_string()
        };
        let bytes = hex.as_bytes();
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks(2) {
            let high = (pair[0] as char).to_digit(16).unwrap_or(0);
            let low = (pair[1] as char).to_digit(16).unwrap_or(0);
            out.push(((high << 4) | low) as u8);
        }
        out
    }
}

pub use binary::{from_base64, from_hex, to_base64, to_hex};

/// Decode a base64 string into binary data (TS `Binary.fromBase64`).
pub use from_base64 as base64_to_array_buffer;
/// Decode a hex string into binary data (TS `Binary.fromHex`).
pub use from_hex as hex_to_array_buffer;
/// Encode binary data as base64 (TS `Binary.toBase64`).
pub use to_base64 as array_buffer_to_base64;
/// Encode binary data as hex (TS `Binary.toHex`).
pub use to_hex as array_buffer_to_hex;

fn is_nullish(value: &Value) -> bool {
    value.is_null()
}

/// Deeply compare JSON values (port of `deepEqual`).
///
/// TS additionally handles `Date`/`RegExp`/`ArrayBuffer`, which have no
/// JSON representation here; `serde_json::Value` equality stands in for
/// the JS identity shortcut (`a === b`).
pub fn deep_equal(a: &Value, b: &Value, strict: bool) -> bool {
    if a == b {
        return true;
    }
    if !strict && is_nullish(a) && is_nullish(b) {
        return true;
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(item_a, item_b)| deep_equal(item_a, item_b, strict))
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: HashSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().all(|key| match (x.get(key), y.get(key)) {
                (Some(value_a), Some(value_b)) => deep_equal(value_a, value_b, strict),
                _ => false,
            })
        }
        _ => false,
    }
}
