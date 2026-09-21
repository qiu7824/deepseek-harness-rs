//! Bounded OOXML inspection for the explicit WPS automation boundary.
use crate::Result;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

#[path = "office_zip.rs"]
mod package;
#[path = "office_xml.rs"]
mod xml;

/// Structural acceptance permits normal relationships and embedded content;
/// it checks ZIP integrity and required XML, never authorizes WPS automation.
pub(crate) fn validate_structure<R: Read + Seek>(input: R, format: &str) -> Result<()> {
    let required = match format {
        "docx" => "word/document.xml",
        "xlsx" => "xl/workbook.xml",
        "pptx" => "ppt/presentation.xml",
        _ => return Err("Unsupported Office package type".into()),
    };
    let mut archive =
        package::open(input).map_err(|e| format!("Cannot open Office package: {e}"))?;
    if archive.len() > 10_000 {
        return Err("Office package has too many entries".into());
    }
    let mut total = 0u64;
    let mut actual = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        total = total.saturating_add(entry.size());
        if total > 64 * 1024 * 1024 {
            return Err("Office expanded content exceeds validation budget".into());
        }
        let expected = entry.size();
        let mut bounded = package::Expanded {
            input: &mut entry,
            total: &mut actual,
            count: 0,
            expected,
        };
        std::io::copy(&mut bounded, &mut std::io::sink())
            .map_err(|e| format!("Corrupt Office entry: {e}"))?;
    }
    for name in ["[Content_Types].xml", required] {
        let entry = archive
            .by_name(name)
            .map_err(|_| format!("Office package missing {name}"))?;
        xml::validate_with_limit(entry, false, 64 * 1024 * 1024)
            .map_err(|e| format!("Invalid XML content in {name}: {e}"))?;
    }
    Ok(())
}

pub fn validate_docx_for_automation(bytes: &[u8]) -> Result<()> {
    validate_office_for_automation(bytes, "docx")
}
pub fn validate_office_for_automation(bytes: &[u8], format: &str) -> Result<()> {
    validate_office_reader(Cursor::new(bytes), format)
}

/// Inspect the immutable input copy without loading the ZIP into memory.
pub fn validate_office_file_for_automation(path: &std::path::Path, format: &str) -> Result<()> {
    validate_office_reader(
        std::fs::File::open(path).map_err(|e| e.to_string())?,
        format,
    )
}

fn validate_office_reader<R: Read + Seek>(mut input: R, format: &str) -> Result<()> {
    let main = match format {
        "docx" => "word/document.xml",
        "xlsx" => "xl/workbook.xml",
        "pptx" => "ppt/presentation.xml",
        _ => return Err("Unsupported Office format".into()),
    };
    if input.seek(SeekFrom::End(0)).map_err(|e| e.to_string())? > 32 * 1024 * 1024 {
        return Err("Office input exceeds 32 MiB".into());
    }
    input.rewind().map_err(|e| e.to_string())?;
    let mut archive = package::open(input).map_err(|e| format!("Invalid DOCX package: {e}"))?;
    if archive.len() > 10_000 {
        return Err("Office entry budget exceeded".into());
    }
    let mut total = 0u64;
    let mut actual = 0u64;
    let mut has_document = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_ascii_lowercase();
        let security_name = name.replace('\\', "/");
        if security_name.contains("vbaproject")
            || security_name
                .split('/')
                .any(|part| matches!(part, "activex" | "embeddings"))
        {
            return Err("Active or embedded Office content is not permitted in the automatic export channel".into());
        }
        total = total.saturating_add(entry.size());
        if total > 64 * 1024 * 1024 {
            return Err("Office expanded input exceeds 64 MiB".into());
        }
        let expected = entry.size();
        let mut bounded = package::Expanded {
            input: &mut entry,
            total: &mut actual,
            count: 0,
            expected,
        };
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let root = xml::validate(&mut bounded, name.ends_with(".rels"))?;
            if name == main {
                has_document = matches!(root.as_str(), "document" | "workbook" | "presentation");
            }
        } else {
            std::io::copy(&mut bounded, &mut std::io::sink())
                .map_err(|e| format!("Invalid Office entry: {e}"))?;
        }
    }
    if !has_document {
        return Err(format!("Office package has no valid {main}"));
    }
    Ok(())
}

/// Fixed one-page synthetic input for an explicit feature check, never user material.
pub fn feature_docx() -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="document" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        (
            "word/document.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:rPr><w:color w:val="000000"/><w:sz w:val="24"/></w:rPr><w:t>Office conversion check</w:t></w:r></w:p><w:p><w:r><w:t>文档转换验证 Alpha 123</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#,
        ),
    ];
    for (name, content) in parts {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .map_err(|e| e.to_string())?;
        writer
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    Ok(writer.finish().map_err(|e| e.to_string())?.into_inner())
}
/// A framing check proves neither PDF rendering nor visual document correctness.
pub fn validate_export_framing(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 16
        || bytes.len() > 64 * 1024 * 1024
        || !bytes.starts_with(b"%PDF-")
        || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|part| part == b"%%EOF")
    {
        return Err("WPS did not produce a complete bounded PDF stream".into());
    }
    Ok(())
}

/// Same framing gate as the byte API, retaining only the PDF header and tail.
pub fn validate_export_file_framing(path: &std::path::Path) -> Result<u64> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if !(16..=64 * 1024 * 1024).contains(&len) {
        return Err("WPS did not produce a complete bounded PDF stream".into());
    }
    let mut header = [0; 5];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(len.saturating_sub(1024)))
        .map_err(|e| e.to_string())?;
    let mut tail = Vec::with_capacity(1024);
    file.take(1024)
        .read_to_end(&mut tail)
        .map_err(|e| e.to_string())?;
    if &header != b"%PDF-" || !tail.windows(5).any(|part| part == b"%%EOF") {
        return Err("WPS did not produce a complete bounded PDF stream".into());
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feature_input_is_readable_ooxml_without_external_content() {
        validate_docx_for_automation(&feature_docx().unwrap()).unwrap();
    }
    #[test]
    fn external_relationships_are_detected_after_xml_entity_decoding() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(br#"<Relationships><Relationship TargetMode="Exter&#110;al" Target="https://example.invalid"/></Relationships>"#).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(
            validate_docx_for_automation(&bytes)
                .unwrap_err()
                .contains("External")
        );
    }
    #[test]
    fn pdf_signature_alone_is_not_export_completion() {
        assert!(validate_export_framing(b"%PDF-1.7 incomplete").is_err());
    }
    #[test]
    fn file_validation_matches_buffer_validation() {
        let path = std::env::temp_dir().join(format!("dsh-office-check-{}", uuid::Uuid::new_v4()));
        let bytes = feature_docx().unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            validate_office_file_for_automation(&path, "docx"),
            validate_docx_for_automation(&bytes)
        );
        for bytes in [
            b"%PDF-1.7\ncontent\n%%EOF\n".as_slice(),
            b"%PDF-1.7 incomplete".as_slice(),
            b"invalid".as_slice(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                validate_export_file_framing(&path).is_ok(),
                validate_export_framing(bytes).is_ok()
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn active_content_path_aliases_are_rejected_without_aliasing_main_part_identity() {
        for name in [
            "activeX/control.bin",
            "word\\activeX\\control.bin",
            "embeddings/object.bin",
        ] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"data").unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            assert!(
                validate_docx_for_automation(&bytes)
                    .unwrap_err()
                    .contains("Active or embedded")
            );
        }
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word\\document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(
            validate_docx_for_automation(&bytes)
                .unwrap_err()
                .contains("no valid")
        );
    }
}
