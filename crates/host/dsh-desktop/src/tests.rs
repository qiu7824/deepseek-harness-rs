use super::*;
#[test]
fn all_web_vectors_render_at_two_themes_and_three_scales() {
    let mut cache = assets::Assets::default();
    for v in assets::VECTORS {
        for dark in [false, true] {
            for scale in [1., 1.25, 2.] {
                let f = cache
                    .vector(v.id, v.height, scale, theme::Theme::new(dark).ink, dark)
                    .unwrap();
                assert!(
                    f.premultiplied_bgra8().chunks_exact(4).any(|p| p[3] > 0),
                    "{} is empty",
                    v.id
                );
                assert_eq!(f.height(), (v.height * scale).round() as u32);
            }
        }
    }
    assert!(cache.bytes <= 8 * 1024 * 1024);
}
#[test]
fn source_preserves_clip_rect_dimensions_and_brand_xml() {
    let settings = assets::VECTORS
        .iter()
        .find(|v| v.id == "IconSettingsOutline16")
        .unwrap();
    assert!(settings.svg.contains("width=\"16\" height=\"16\""));
    let brand = assets::VECTORS
        .iter()
        .find(|v| v.id == "BrandWordmark")
        .unwrap();
    assert!(!brand.svg.contains("=\"true\"=\"true\""));
}
#[test]
fn markdown_retains_rich_semantics() {
    use markdown::Block;
    let blocks = markdown::parse(
        "# 标题\n\n正文 **加粗** 和 [链接](https://example.com) 与 `code`。\n\n> 引用\n\n1. 项目\n2. 第二项\n\n| A | B |\n|---|---|\n| x | y |\n\n```rust\nfn main() {}\n```\n\n![图](pic.png)\n\n---",
    );
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b,Block::Table{rows}if rows.len()==2&&rows[0].len()==2))
    );
    assert!(blocks.iter().any(
        |b| matches!(b,Block::Code{language,text}if language=="rust"&&text.contains("fn main"))
    ));
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b,Block::Image{url,..}if url=="pic.png"))
    );
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, Block::Text { quote: true, .. }))
    );
    assert!(blocks.iter().any(|b|matches!(b,Block::Text{spans,..}if spans.iter().any(|s|s.bold)&&spans.iter().any(|s|s.link.is_some()))));
}
#[test]
fn history_hides_injected_instructions() {
    let h = serde_json::json!({"events":[
    {"event":{"seq":1,"type":"user/message","data":{"source":{"kind":"plugin"},"content":[{"type":"text","text":"hidden"}]}}},
    {"event":{"seq":2,"type":"user/message","data":{"source":{"kind":"user"},"content":[{"type":"text","text":"你好"}]}}},
    {"event":{"seq":3,"type":"assistant/message","data":{"message":{"content":[{"type":"text","text":"回复"}]}}}}
    ]});
    let ms = model::messages(&h);
    assert_eq!(ms.len(), 2);
    assert_eq!(ms[0].text, "你好");
    assert_eq!(ms[1].text, "回复");
}

#[test]
fn tool_result_uses_the_durable_call_identity_and_error() {
    let h = serde_json::json!({"events":[
        {"event":{"seq":1,"type":"tool/call","data":{"callId":"c1","name":"read","arguments":"{}"}}},
        {"event":{"seq":2,"type":"tool/result","data":{"message":{"source":{"kind":"tool","callId":"c1"},"content":[{"type":"text","text":"file missing"}]},"error":{"name":"NotFound"}}}}
    ]});
    let ms = model::messages(&h);
    assert_eq!(ms.len(), 1);
    assert!(ms[0].error);
    assert!(ms[0].text.contains("file missing"));
}
