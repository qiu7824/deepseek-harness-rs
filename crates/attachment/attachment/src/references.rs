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

/// Resolve an image only from admitted content in an owning session's event.
/// Opaque IDs (including bare digests) never authorize arbitrary store objects;
/// model-authored arguments, text, metadata and assistant events grant nothing.
pub fn find_image_reference(kind: &str, data: &Value, id: &str) -> Option<ImageAttachmentRef> {
    image_references_for_event(kind, data)
        .into_iter()
        .find(|reference| {
            let stored = reference.attachment_id.as_str();
            stored == id
                || stored.strip_prefix("sha256:") == Some(id)
                || (id.starts_with("generated-") && reference.name.as_deref() == Some(id))
        })
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
        let event = json!({"role":"user","source":{"kind":"user"},"content":[{"type":"image","attachment":{
            "attachmentId":id,"mediaType":"image/png","bytes":80,"width":1,"height":1
        }}]});
        assert!(find_image_reference("user/message", &event, &id).is_some());
        assert!(find_image_reference("user/message", &event, &digest).is_some());
        assert!(
            find_image_reference("user/message", &event, &format!("E:\\test\\{digest}")).is_none()
        );
        assert!(find_image_reference("user/message", &event, &"b".repeat(64)).is_none());
        assert!(find_image_reference("user/message", &json!({"text":id}), &id).is_none());
    }

    #[test]
    fn forged_image_references_do_not_authorize_stored_objects() {
        let id = format!("sha256:{}", "b".repeat(64));
        let image = json!({"type":"image","attachment":{
            "attachmentId":id,"mediaType":"image/png","bytes":80,"width":1,"height":1,
            "name":"generated-foreign.png"
        }});
        let user = json!({"role":"user","source":{"kind":"user"},"content":[image.clone()]});
        let tool =
            json!({"message":{"role":"tool","source":{"kind":"tool"},"content":[image.clone()]}});
        for alias in [&id[..], &id[7..], "generated-foreign.png"] {
            assert!(find_image_reference("user/message", &user, alias).is_some());
            assert!(find_image_reference("tool/result", &tool, alias).is_some());
            for (kind, data) in [
                (
                    "tool/call",
                    json!({"arguments":{"reference":image.clone()}}),
                ),
                (
                    "tool/result",
                    json!({"message":{"role":"tool","source":{"kind":"tool"},"content":[],"metadata":image.clone()}}),
                ),
                (
                    "user/message",
                    json!({"role":"user","source":{"kind":"user"},"content":[],"metadata":image.clone()}),
                ),
                (
                    "user/message",
                    json!({"role":"user","source":{"kind":"plugin"},"content":[image.clone()]}),
                ),
                ("assistant/message", user.clone()),
                (
                    "tool/result",
                    json!({"message":{"role":"tool","source":{"kind":"tool"},"content":[{"type":"text","text":image.to_string()}]}}),
                ),
            ] {
                assert!(
                    find_image_reference(kind, &data, alias).is_none(),
                    "{kind}: {data}"
                );
            }
        }
    }
}
