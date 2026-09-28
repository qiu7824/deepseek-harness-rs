use crate::ImageAttachmentRef;
use serde_json::Value;

/// Read only declared, admitted file slots, never opaque tool arguments or text.
pub fn file_references_for_event(kind: &str, data: &Value) -> Vec<crate::FileAttachmentRef> {
    let message = match kind {
        "user/message" => data,
        "tool/result" => &data["message"],
        _ => return vec![],
    };
    if !(message["role"] == "user" && message["source"]["kind"] == "user"
        || message["role"] == "tool" && message["source"]["kind"] == "tool")
    {
        return vec![];
    }
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "file")
        .filter_map(|block| serde_json::from_value(block["attachment"].clone()).ok())
        .collect()
}

/// Only admitted image content grants file access. Tool arguments, text,
/// assistant-authored references and nested metadata cannot grant permissions.
pub fn image_references_for_event(kind: &str, data: &Value) -> Vec<ImageAttachmentRef> {
    let message = match kind {
        "user/message" => data,
        "tool/result" => &data["message"],
        _ => return vec![],
    };
    if !(message["role"] == "user" && message["source"]["kind"] == "user"
        || message["role"] == "tool" && message["source"]["kind"] == "tool")
    {
        return vec![];
    }
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "image")
        .filter_map(|block| serde_json::from_value(block["attachment"].clone()).ok())
        .collect()
}

/// Resolve an image only from events supplied by the owning session. Opaque
/// IDs (including the bare digest) are not workspace filenames.
pub fn find_image_reference(value: &Value, id: &str) -> Option<ImageAttachmentRef> {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("image") {
                if let Some(attachment) = map.get("attachment") {
                    let stored = attachment["attachmentId"].as_str().unwrap_or("");
                    let matches = stored == id
                        || stored.strip_prefix("sha256:") == Some(id)
                        || (id.starts_with("generated-")
                            && attachment["name"].as_str() == Some(id));
                    if matches {
                        if let Ok(reference) = serde_json::from_value(attachment.clone()) {
                            return Some(reference);
                        }
                    }
                }
            }
            map.values()
                .find_map(|value| find_image_reference(value, id))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_image_reference(value, id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn image_permissions_require_admitted_content_slots() {
        let image = json!({"type":"image","attachment":{
            "attachmentId":format!("sha256:{}", "a".repeat(64)),
            "mediaType":"image/png","bytes":80,"width":1,"height":1
        }});
        let user = json!({"role":"user","source":{"kind":"user"},"content":[image.clone()]});
        assert_eq!(image_references_for_event("user/message", &user).len(), 1);
        let tool =
            json!({"message":{"role":"tool","source":{"kind":"tool"},"content":[image.clone()]}});
        assert_eq!(image_references_for_event("tool/result", &tool).len(), 1);
        for event in [
            json!({"role":"user","source":{"kind":"plugin"},"content":[image.clone()]}),
            json!({"role":"user","source":{"kind":"user"},"content":[{"type":"text","text":image.to_string()}]}),
            json!({"role":"user","source":{"kind":"user"},"content":[],"metadata":image.clone()}),
        ] {
            assert!(image_references_for_event("user/message", &event).is_empty());
        }
        assert!(
            image_references_for_event("assistant/message", &json!({"message":user})).is_empty()
        );
        assert!(image_references_for_event("tool/call", &json!({"arguments":image})).is_empty());
    }
    #[test]
    fn resolves_owned_ids_without_treating_paths_or_foreign_ids_as_attachments() {
        let digest = "a".repeat(64);
        let id = format!("sha256:{digest}");
        let event = json!({"content":[{"type":"image","attachment":{
            "attachmentId":id,"mediaType":"image/png","bytes":80,"width":1,"height":1
        }}]});
        assert!(find_image_reference(&event, &id).is_some());
        assert!(find_image_reference(&event, &digest).is_some());
        assert!(find_image_reference(&event, &format!("E:\\test\\{digest}")).is_none());
        assert!(find_image_reference(&event, &"b".repeat(64)).is_none());
        assert!(find_image_reference(&json!({"text":id}), &id).is_none());
    }
}
