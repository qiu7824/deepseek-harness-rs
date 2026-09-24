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
