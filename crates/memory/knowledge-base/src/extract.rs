//! Plain text from uploaded documents.

use std::io::{Cursor, Read};

/// Largest accepted source document.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
/// Extracted text beyond this many characters is dropped.
pub const MAX_TEXT_CHARS: usize = 8 * 1024 * 1024;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml",
    "ini", "cfg", "conf", "xml", "sql", "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go",
    "java", "kt", "c", "h", "cc", "cpp", "hpp", "cs", "rb", "php", "swift", "dart", "sh", "bash",
    "ps1", "bat", "cmd", "lua", "r", "scala", "vue", "svelte", "css", "scss", "less", "tex",
];

/// Extensions the importer accepts.
pub fn supported(name: &str) -> bool {
    let ext = extension(name);
    TEXT_EXTENSIONS.contains(&ext.as_str())
        || matches!(
            ext.as_str(),
            "html" | "htm" | "docx" | "pptx" | "xlsx" | "pdf"
        )
}

/// Every accepted extension, documents first, for file pickers.
pub fn supported_extensions() -> Vec<&'static str> {
    let mut out = vec!["pdf", "docx", "pptx", "xlsx", "html", "htm"];
    out.extend_from_slice(TEXT_EXTENSIONS);
    out
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Decode text bytes: UTF-8 (with or without BOM), UTF-16 with BOM, then
/// GB18030 for legacy Chinese files when UTF-8 fails.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(bytes).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, had_errors) = encoding_rs::GB18030.decode(bytes);
            if had_errors {
                String::from_utf8_lossy(bytes).into_owned()
            } else {
                text.into_owned()
            }
        }
    }
}

/// Visible text of an HTML document: tags removed, block tags as line
/// breaks, scripts and styles dropped, common entities decoded.
pub fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        if rest.starts_with('<') {
            let tail = &lower[index..];
            let skip_until = if tail.starts_with("<script") {
                Some("</script>")
            } else if tail.starts_with("<style") {
                Some("</style>")
            } else {
                None
            };
            if let Some(close) = skip_until {
                index += tail
                    .find(close)
                    .map(|at| at + close.len())
                    .unwrap_or(rest.len());
                continue;
            }
            let end = rest.find('>').map(|at| at + 1).unwrap_or(rest.len());
            let tag = &tail[..end.min(tail.len())];
            if [
                "<br", "<p", "</p", "<div", "</div", "<li", "<tr", "<h1", "<h2", "<h3", "<h4",
                "</h",
            ]
            .iter()
            .any(|prefix| tag.starts_with(prefix))
            {
                out.push('\n');
            }
            index += end;
        } else {
            let next = rest.find('<').unwrap_or(rest.len());
            out.push_str(&rest[..next]);
            index += next;
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn zip_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("文档缺少 {name}"))?;
    if file.size() > MAX_DOCUMENT_BYTES as u64 * 4 {
        return Err("文档内容过大".into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("无法读取 {name}：{error}"))?;
    Ok(text)
}

/// Text of the `text_tag` elements in an OOXML part; `break_tags` end a line.
fn ooxml_text(xml: &str, text_tag: &[u8], break_tags: &[&[u8]]) -> Result<String, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut inside = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) if tag.local_name().as_ref() == text_tag => inside = true,
            Ok(Event::End(tag)) => {
                let name = tag.local_name();
                if name.as_ref() == text_tag {
                    inside = false;
                } else if break_tags.contains(&name.as_ref()) {
                    out.push('\n');
                }
            }
            Ok(Event::Empty(tag)) if matches!(tag.local_name().as_ref(), b"br" | b"tab") => {
                out.push(if tag.local_name().as_ref() == b"tab" {
                    '\t'
                } else {
                    '\n'
                });
            }
            Ok(Event::Text(text)) if inside => {
                out.push_str(&text.decode().map_err(|error| error.to_string())?);
            }
            // Entity references arrive as their own events.
            Ok(Event::GeneralRef(reference)) if inside => {
                if let Ok(Some(ch)) = reference.resolve_char_ref() {
                    out.push(ch);
                } else if let Ok(name) = reference.decode() {
                    out.push_str(match name.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("文档 XML 无效：{error}")),
            _ => {}
        }
    }
    Ok(out)
}

fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "不是有效的 DOCX 文件".to_string())?;
    ooxml_text(&zip_part(&mut archive, "word/document.xml")?, b"t", &[b"p"])
}

fn pptx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "不是有效的 PPTX 文件".to_string())?;
    let mut slides: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|name| {
            let number = name
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse()
                .ok()?;
            Some((number, name.to_string()))
        })
        .collect();
    slides.sort();
    let mut out = String::new();
    for (number, name) in slides {
        out.push_str(&format!("[幻灯片 {number}]\n"));
        out.push_str(&ooxml_text(&zip_part(&mut archive, &name)?, b"t", &[b"p"])?);
        out.push_str("\n\n");
    }
    Ok(out)
}

fn xlsx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "不是有效的 XLSX 文件".to_string())?;
    // Shared strings carry nearly all cell text; inline and numeric cells in
    // sheets are appended so numbers and formulas' cached values survive.
    let shared = zip_part(&mut archive, "xl/sharedStrings.xml")
        .ok()
        .map(|xml| ooxml_text(&xml, b"t", &[b"si"]))
        .transpose()?
        .unwrap_or_default();
    let mut sheets: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    sheets.sort();
    let mut out = shared;
    for sheet in sheets {
        let values = ooxml_text(&zip_part(&mut archive, &sheet)?, b"v", &[b"row"])?;
        if !values.trim().is_empty() {
            out.push('\n');
            out.push_str(&values);
        }
    }
    Ok(out)
}

fn pdf_text(bytes: &[u8]) -> Result<String, String> {
    let document = lopdf::Document::load_mem(bytes)
        .map_err(|error| format!("不是有效的 PDF 文件：{error}"))?;
    if document.is_encrypted() {
        return Err("PDF 已加密，无法读取文字".into());
    }
    let pages: Vec<u32> = document.get_pages().keys().copied().collect();
    let mut out = String::new();
    for page in pages {
        if let Ok(text) = document.extract_text(&[page]) {
            out.push_str(&text);
            out.push_str("\n\n");
        }
    }
    if out.trim().is_empty() {
        return Err("PDF 中没有可提取的文字（可能是扫描件）".into());
    }
    Ok(out)
}

/// Plain text of one document named `name`.
pub fn extract(name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("文件超过 64 MiB".into());
    }
    let ext = extension(name);
    let text = match ext.as_str() {
        "docx" => docx_text(bytes)?,
        "pptx" => pptx_text(bytes)?,
        "xlsx" => xlsx_text(bytes)?,
        "pdf" => pdf_text(bytes)?,
        "html" | "htm" => html_text(&decode_text(bytes)),
        _ if TEXT_EXTENSIONS.contains(&ext.as_str()) => {
            if bytes.iter().take(8192).any(|byte| *byte == 0)
                && !bytes.starts_with(&[0xFF, 0xFE])
                && !bytes.starts_with(&[0xFE, 0xFF])
            {
                return Err("文件看起来是二进制内容".into());
            }
            decode_text(bytes)
        }
        _ => return Err(format!("暂不支持 .{ext} 文件")),
    };
    let text: String = text
        .chars()
        .filter(|ch| *ch != '\0')
        .take(MAX_TEXT_CHARS)
        .collect();
    if text.trim().is_empty() {
        return Err("没有提取到文字".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ooxml(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            for (name, body) in parts {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn text_files_decode_utf8_utf16_and_gbk() {
        assert_eq!(
            extract("a.md", "# 标题\n内容".as_bytes()).unwrap(),
            "# 标题\n内容"
        );
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "中文".encode_utf16() {
            utf16.extend(unit.to_le_bytes());
        }
        assert_eq!(extract("a.txt", &utf16).unwrap(), "中文");
        let (gbk, _, _) = encoding_rs::GBK.encode("知识库");
        assert_eq!(extract("a.txt", &gbk).unwrap(), "知识库");
        assert!(extract("a.txt", &[0, 1, 2, 3]).is_err());
        assert!(extract("a.exe", b"MZ").unwrap_err().contains(".exe"));
        assert!(!supported("photo.png") && supported("Report.DOCX"));
    }

    #[test]
    fn html_drops_markup_scripts_and_decodes_entities() {
        let text = html_text(
            "<html><style>p{}</style><p>一&amp;二</p><script>x()</script><div>三 &lt;四&gt;</div></html>",
        );
        assert!(text.contains("一&二") && text.contains("三 <四>"));
        assert!(!text.contains("x()") && !text.contains("p{}"));
    }

    #[test]
    fn office_documents_yield_paragraph_text() {
        let docx = ooxml(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>第一段</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">第二 &amp; 段</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        assert_eq!(extract("a.docx", &docx).unwrap(), "第一段\n第二 & 段\n");
        let pptx = ooxml(&[
            (
                "ppt/slides/slide2.xml",
                r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>结论</a:t></a:r></a:p></p:sld>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>开场</a:t></a:r></a:p></p:sld>"#,
            ),
        ]);
        let text = extract("a.pptx", &pptx).unwrap();
        assert!(text.find("开场").unwrap() < text.find("结论").unwrap());
        let xlsx = ooxml(&[
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>姓名</t></si><si><t>分数</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c><v>98</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        let text = extract("a.xlsx", &xlsx).unwrap();
        assert!(text.contains("姓名") && text.contains("98"));
        assert!(extract("a.docx", b"not a zip").is_err());
    }

    #[test]
    fn invalid_pdfs_report_a_readable_error() {
        assert!(
            extract("a.pdf", b"%PDF-1.4 broken")
                .unwrap_err()
                .contains("PDF")
        );
    }

    #[test]
    fn pdf_page_text_is_extracted() {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal("Rollback guide")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
                "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert!(
            extract("guide.pdf", &bytes)
                .unwrap()
                .contains("Rollback guide")
        );
    }
}
