//! Bounded request-shape evidence for Devin rejections. Never retain payloads.
use crate::{LlmFailure, devin_wire::Message};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_DETAIL_BYTES: usize = 8192;
const MAX_FIELDS: usize = 4;
const MAX_PATH: usize = 96;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn request_shape(chat: &Value) -> Value {
    // This boundary cannot prove whether an identifier came from the account
    // catalog or a manually entered model. Hash every UID, including ordinary
    // looking identifiers, rather than risk displaying user-supplied content.
    let model = chat["model"].as_str().unwrap_or_default();
    let mut schemas = Sha256::new();
    let mut schema_bytes = 0usize;
    let mut objects = 0usize;
    let mut combinators = 0usize;
    let tools: Vec<_> = chat["tools"].as_array().into_iter().flatten().collect();
    for tool in &tools {
        let schema = crate::tool_schema::object_root(&tool["function"]["parameters"]);
        objects += usize::from(schema["type"] == "object");
        combinators += ["oneOf", "anyOf", "allOf"]
            .iter()
            .filter(|key| schema.get(**key).is_some())
            .count();
        let encoded = schema.to_string();
        schema_bytes += encoded.len();
        // Hash exactly the ordered wire schemas, with length boundaries. Tool
        // names, descriptions outside the schema, and conversation data never
        // reach this digest or the displayable diagnostic.
        schemas.update((encoded.len() as u64).to_be_bytes());
        schemas.update(encoded.as_bytes());
    }
    let mut leading_system = true;
    let mut messages = 0usize;
    let mut signed = 0usize;
    for message in chat["messages"].as_array().into_iter().flatten() {
        if leading_system && message["role"] == "system" {
            continue;
        }
        leading_system = false;
        messages += 1;
        signed += usize::from(
            message["role"] == "assistant"
                && message
                    .pointer("/devin_state/signature")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
        );
    }
    json!({
        "modelUidHash":digest(model.as_bytes()),"modelUidChars":model.chars().count(),
        "toolCount":tools.len(),"messageCount":messages,"signedReplayCount":signed,
        "schema":{"objectRoots":objects,"rootCombinators":combinators,
            "bytes":schema_bytes,"sha256":format!("{:x}",schemas.finalize())}
    })
}

fn known_field_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH {
        return false;
    }
    // Only known request/protocol labels are displayable, including downstream
    // Claude schema labels. Arbitrary property names can contain user data.
    const ROOTS: &[&str] = &[
        "model",
        "chat_model_uid",
        "chatModelUid",
        "configuration",
        "tools",
        "chat_message_prompts",
        "chatMessagePrompts",
        "prompt",
        "metadata",
        "tool_choice",
        "toolChoice",
        "system_prompt_cache_options",
        "systemPromptCacheOptions",
    ];
    const SEGMENTS: &[&str] = &[
        "model",
        "chat_model_uid",
        "chatModelUid",
        "configuration",
        "tools",
        "chat_message_prompts",
        "chatMessagePrompts",
        "prompt",
        "metadata",
        "tool_choice",
        "toolChoice",
        "system_prompt_cache_options",
        "systemPromptCacheOptions",
        "num_completions",
        "numCompletions",
        "max_tokens",
        "maxTokens",
        "max_newlines",
        "maxNewlines",
        "temperature",
        "first_temperature",
        "firstTemperature",
        "top_k",
        "topK",
        "top_p",
        "topP",
        "stop_patterns",
        "stopPatterns",
        "seed",
        "service_tier",
        "serviceTier",
        "name",
        "description",
        "json_schema_string",
        "jsonSchemaString",
        "function",
        "parameters",
        "input_schema",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "oneOf",
        "anyOf",
        "allOf",
        "items",
        "enum",
        "const",
        "api_key",
        "apiKey",
        "user_jwt",
        "userJwt",
        "source",
        "tool_call_id",
        "toolCallId",
        "tool_calls",
        "toolCalls",
        "thinking",
        "signature",
        "images",
        "mime_type",
        "mimeType",
        "base64_data",
        "base64Data",
        "option_name",
        "optionName",
    ];
    for (index, part) in path.split('.').enumerate() {
        let (name, subscript) = part.split_once('[').unwrap_or((part, ""));
        if !SEGMENTS.contains(&name) || (index == 0 && !ROOTS.contains(&name)) {
            return false;
        }
        if !subscript.is_empty() {
            let Some(number) = subscript.strip_suffix(']') else {
                return false;
            };
            if number.is_empty()
                || number.len() > 4
                || !number.bytes().all(|byte| byte.is_ascii_digit())
            {
                return false;
            }
        } else if part.contains('[') {
            return false;
        }
    }
    true
}

fn bad_request_fields(error: Option<&Value>) -> Vec<Value> {
    let mut fields = Vec::new();
    for detail in error
        .and_then(|error| error["details"].as_array())
        .into_iter()
        .flatten()
        .take(8)
    {
        if !matches!(
            detail["type"].as_str(),
            Some("google.rpc.BadRequest" | "type.googleapis.com/google.rpc.BadRequest")
        ) {
            continue;
        }
        let Some(encoded) = detail["value"].as_str() else {
            continue;
        };
        // Enforce the allocation cap before base64 decoding. Ignore debug,
        // descriptions and unknown detail types, which may echo full payloads.
        if encoded.len() > MAX_DETAIL_BYTES.div_ceil(3) * 4 {
            continue;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded));
        let Ok(bytes) = bytes else {
            continue;
        };
        if bytes.len() > MAX_DETAIL_BYTES {
            continue;
        }
        let Ok(message) = Message::parse(&bytes) else {
            continue;
        };
        let Ok(violations) = message.repeated(1) else {
            continue;
        };
        for violation in violations {
            if fields.len() == MAX_FIELDS {
                return fields;
            }
            let Ok(violation) = Message::parse(violation) else {
                continue;
            };
            let Ok(path) = violation.text(1) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            fields.push(if known_field_path(path) {
                json!({"path":path})
            } else {
                json!({"pathHash":digest(path.as_bytes())})
            });
        }
    }
    fields
}

pub(super) fn attach(
    mut failure: LlmFailure,
    chat: &Value,
    phase: &'static str,
    wire_bytes: (usize, usize),
    error: Option<&Value>,
    native_code: Option<&str>,
) -> LlmFailure {
    let mut summary = request_shape(chat);
    summary["phase"] = json!(phase);
    summary["requestBytes"] = json!(wire_bytes.0);
    summary["compressedBytes"] = json!(wire_bytes.1);
    let fields = bad_request_fields(error);
    if !fields.is_empty() {
        summary["badRequestFields"] = json!(fields);
    }
    // All summary strings are local constants, fixed-size hashes or strictly
    // allowlisted field paths. Append after provider redaction/truncation and
    // retain the native-code suffix at the end for existing presentation.
    let suffix = native_code.map(|code| format!(" [{code}]"));
    let suffix = suffix
        .as_deref()
        .filter(|suffix| failure.message.ends_with(*suffix));
    let suffix = suffix.unwrap_or_default().to_owned();
    failure
        .message
        .truncate(failure.message.len() - suffix.len());
    failure
        .message
        .push_str(&format!("\n[devin-diagnostic:{summary}]{suffix}"));
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devin_wire::Encoder;

    fn detail(paths: &[&str]) -> Value {
        let mut bytes = Encoder::default();
        for path in paths {
            let mut violation = Encoder::default();
            violation.text(1, path);
            violation.text(2, "PRIVATE DESCRIPTION AND PROMPT");
            bytes.bytes(1, &violation.0);
        }
        json!({"type":"google.rpc.BadRequest", "value":base64::engine::general_purpose::STANDARD.encode(bytes.0)})
    }

    #[test]
    fn detail_paths_do_not_accept_arbitrary_segments_or_unbounded_indices() {
        let mut paths = vec!["tools[0].input_schema", "metadata.api_key"];
        paths.extend([
            "tools[0].input_schema.properties.privateAccountName",
            "tools[12345].name",
            "tools[0][1].name",
            "tools[-1].name",
            "tools[0].name\nprivate prompt",
            "privateAccountName.temperature",
            "tools[].name",
            "tools[0].",
        ]);
        // Check adversarial paths one at a time so the global evidence cap
        // does not hide a permissive grammar bug in a later entry.
        for (index, path) in paths.iter().enumerate() {
            let fields = bad_request_fields(Some(&json!({"details":[detail(&[path])]})));
            assert_eq!(fields.len(), 1);
            if index < 2 {
                assert_eq!(fields[0], json!({"path":path}));
            } else {
                assert_eq!(fields[0], json!({"pathHash":digest(path.as_bytes())}));
                assert!(!fields[0].to_string().contains("private"));
            }
        }
    }

    #[test]
    fn details_ignore_oversized_malformed_unknown_and_debug_payloads() {
        let oversize = "A".repeat(MAX_DETAIL_BYTES.div_ceil(3) * 4 + 4);
        let error = json!({"details":[
            {"type":"google.rpc.BadRequest", "value":oversize},
            {"type":"google.rpc.BadRequest", "value":"invalid base64"},
            {"type":"google.rpc.BadRequest", "value":"CA=="},
            {"type":"privateDetailName", "value":detail(&["chat_model_uid"])["value"]},
            {"type":"google.rpc.BadRequest", "debug":{"fieldViolations":[{"field":"chat_model_uid"}]}},
            detail(&["configuration.max_tokens"])
        ]});
        assert_eq!(
            bad_request_fields(Some(&error)),
            json!([{"path":"configuration.max_tokens"}])
                .as_array()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn field_evidence_has_one_global_limit_including_hashed_paths() {
        let error = json!({"details":[
            detail(&["private-one", "private-two", "configuration.temperature"]),
            detail(&["tools[0].input_schema", "configuration.max_tokens", "private-three"])
        ]});
        let fields = bad_request_fields(Some(&error));
        assert_eq!(fields.len(), MAX_FIELDS);
        assert!(fields[0].get("pathHash").is_some());
        assert!(fields[1].get("pathHash").is_some());
        assert_eq!(fields[3]["path"], "tools[0].input_schema");
        assert!(!serde_json::to_string(&fields).unwrap().contains("private"));
    }
}
