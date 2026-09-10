//! Validated screen-review messages with durable image and coordinate evidence.
use super::{WebResponse, error};
use base64::Engine;
use dsh_attachment::{AttachmentStore, ImageMediaType, SaveImageAttachment};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Note {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Annotations {
    #[serde(default)]
    strokes: Vec<Vec<Point>>,
    #[serde(default)]
    notes: Vec<Note>,
}

fn invalid(message: &str) -> WebResponse {
    error(StatusCode::BAD_REQUEST, "annotation-invalid", message)
}

fn annotations(value: &Value) -> Result<Annotations, WebResponse> {
    let raw = serde_json::to_string(value).map_err(|_| invalid("批注格式无效"))?;
    if raw.len() > 48 * 1024 {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "annotation-too-large",
            "批注内容超过大小限制",
        ));
    }
    let value: Annotations =
        serde_json::from_str(&raw).map_err(|_| invalid("批注坐标或文字格式无效"))?;
    if value.strokes.is_empty() && value.notes.is_empty() {
        return Err(invalid("批注内容为空"));
    }
    let coordinate = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
    if value.strokes.len() > 64
        || value.notes.len() > 64
        || value.strokes.iter().any(|stroke| {
            stroke.is_empty()
                || stroke.len() > 1024
                || stroke.iter().any(|p| !coordinate(p.x) || !coordinate(p.y))
        })
    {
        return Err(invalid(
            "批注线条需使用 0 至 1 的归一化坐标，最多 64 条且每条不超过 1024 个点",
        ));
    }
    if value.notes.iter().any(|note| {
        note.text.trim().is_empty()
            || note.text.chars().count() > 500
            || match (note.x, note.y) {
                (None, None) => false,
                (Some(x), Some(y)) => !coordinate(x) || !coordinate(y),
                _ => true,
            }
    }) {
        return Err(invalid(
            "批注文字需要 1 至 500 个字符，位置需使用完整的归一化坐标",
        ));
    }
    Ok(value)
}

pub(super) async fn message(
    body: &Value,
    store: &dyn AttachmentStore,
) -> Result<dsh_llm::UserMessage, WebResponse> {
    let annotations = annotations(body.get("annotations").unwrap_or(&Value::Null))?;
    let control_session = match body.get("browserSessionId") {
        Some(value) => value.as_str().ok_or_else(|| invalid("控制会话标识无效"))?,
        None => "default",
    };
    if control_session.is_empty()
        || control_session.len() > 200
        || control_session.chars().any(char::is_control)
    {
        return Err(invalid("控制会话标识无效"));
    }
    let screenshot = body
        .get("screenshot")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("批注需要提交时的当前截图"))?;
    let (media_type, extension) = match screenshot.get("mediaType").and_then(Value::as_str) {
        Some("image/png") => (ImageMediaType::Png, "png"),
        Some("image/jpeg") => (ImageMediaType::Jpeg, "jpg"),
        Some("image/webp") => (ImageMediaType::Webp, "webp"),
        Some("image/gif") => (ImageMediaType::Gif, "gif"),
        _ => return Err(invalid("批注截图媒体类型无效")),
    };
    let encoded = screenshot
        .get("base64")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("批注截图编码无效"))?;
    if encoded.len() > 4 * 1024 * 1024 {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "annotation-image-too-large",
            "批注截图超过大小限制",
        ));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid("批注截图编码无效"))?;
    if data.is_empty() || data.len() > 3 * 1024 * 1024 {
        return Err(invalid("批注截图为空或超过大小限制"));
    }
    let reference = store
        .save_image(&SaveImageAttachment {
            data,
            media_type,
            name: Some(format!("screen-annotation.{extension}")),
        })
        .await
        .map_err(|failure| invalid(&failure.to_string()))?;
    let viewport = json!({"width":reference.width,"height":reference.height});
    if let Some(declared) = body.get("viewport") {
        if declared.get("width").and_then(Value::as_u64) != Some(reference.width)
            || declared.get("height").and_then(Value::as_u64) != Some(reference.height)
        {
            return Err(invalid("批注画面尺寸与截图不一致，请重新观察画面后提交"));
        }
    }
    let evidence = json!({"browserSessionId":control_session,"viewport":viewport,"coordinateSystem":"normalized-0-to-1","strokes":annotations.strokes,"notes":annotations.notes});
    let text = format!(
        "画面批注\n坐标以附带截图为基准：左上角为 (0,0)，右下角为 (1,1)。\n{}",
        serde_json::to_string(&evidence).expect("validated annotation")
    );
    Ok(dsh_llm::create_user_message(
        vec![
            dsh_llm::ContentBlock::Text { text },
            dsh_llm::ContentBlock::Image {
                attachment: dsh_llm::ImageAttachmentRef {
                    attachment_id: reference.attachment_id.to_string(),
                    media_type: Some(reference.media_type.as_str().into()),
                    bytes: Some(reference.bytes),
                    width: Some(reference.width),
                    height: Some(reference.height),
                    name: reference.name,
                },
            },
        ],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> Value {
        json!({"browserSessionId":"uu-control-one","annotations":{"strokes":[[{"x":0.1,"y":0.2},{"x":0.9,"y":0.8}]],"notes":[{"text":"请检查红线区域","at":123,"x":0.4,"y":0.5}]},"viewport":{"width":1,"height":1},"screenshot":{"mediaType":"image/png","base64":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}})
    }
    fn store() -> (
        std::path::PathBuf,
        std::sync::Arc<dsh_attachment_local::LocalAttachmentStore>,
    ) {
        let root =
            std::env::temp_dir().join(format!("dsh-annotation-test-{}", uuid::Uuid::new_v4()));
        let context = cordis::Context::root();
        let store = dsh_attachment_local::LocalAttachmentStore::install(
            &context,
            dsh_attachment_local::Config {
                dsh_home: Some(root.to_string_lossy().into()),
                ..Default::default()
            },
        );
        (root, store)
    }
    #[tokio::test]
    async fn submitted_user_message_contains_coordinates_and_a_durable_image() {
        let (root, store) = store();
        let value = message(&body(), store.as_ref()).await.unwrap();
        let serialized = serde_json::to_value(&value).unwrap();
        let text = serialized["content"][0]["text"].as_str().unwrap();
        let data: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(data["browserSessionId"], "uu-control-one");
        assert_eq!(data["strokes"], body()["annotations"]["strokes"]);
        assert_eq!(data["notes"], body()["annotations"]["notes"]);
        assert_eq!(data["viewport"], json!({"width":1,"height":1}));
        assert_eq!(serialized["role"], "user");
        assert!(serialized["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(serialized["content"][1]["type"], "image");
        assert!(
            serialized["content"][1]["attachment"]["attachmentId"]
                .as_str()
                .is_some_and(|id| id.starts_with("sha256:"))
        );
        let reference =
            serde_json::from_value(serialized["content"][1]["attachment"].clone()).unwrap();
        store
            .read_image(&reference, None)
            .await
            .expect("submitted screenshot remains readable from durable storage");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn missing_stale_and_invalid_images_cannot_become_annotation_messages() {
        let (root, store) = store();
        for scenario in [
            "missing",
            "wrong-dimensions",
            "wrong-format",
            "invalid-bytes",
        ] {
            let mut value = body();
            match scenario {
                "missing" => {
                    value.as_object_mut().unwrap().remove("screenshot");
                }
                "wrong-dimensions" => value["viewport"]["width"] = json!(2),
                "wrong-format" => value["screenshot"]["mediaType"] = json!("image/jpeg"),
                _ => value["screenshot"]["base64"] = json!("bm90IGFuIGltYWdl"),
            }
            assert!(message(&value, store.as_ref()).await.is_err(), "{scenario}");
        }
        drop(store);
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn invalid_coordinates_empty_content_and_excessive_notes_are_rejected() {
        for value in [
            json!({}),
            json!({"notes":[{"text":" "}]}),
            json!({"notes":[{"text":"ok","x":0.5}]}),
            json!({"strokes":[[{"x":1.1,"y":0.2}]]}),
            json!({"strokes":[[]]}),
            json!({"notes":[{"text":"x".repeat(501)}]}),
        ] {
            assert!(annotations(&value).is_err());
        }
        assert!(annotations(&json!({"notes":vec![json!({"text":"ok"});65]})).is_err());
        assert!(annotations(&json!({"notes":[{"text":"x".repeat(49*1024)}]})).is_err());
    }
}
