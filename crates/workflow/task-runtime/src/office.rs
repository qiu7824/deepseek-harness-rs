//! Bounded OOXML inspection for the explicit WPS automation boundary.
use crate::Result;
use std::io::{Cursor,Read,Write};

pub fn validate_docx_for_automation(bytes:&[u8])->Result<()> {
    if bytes.len()>32*1024*1024 {return Err("Office input exceeds 32 MiB".into());}
    let mut archive=zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e|format!("Invalid DOCX package: {e}"))?;
    if archive.len()>10_000 {return Err("Office entry budget exceeded".into());}
    let mut total=0u64;let mut has_document=false;
    for index in 0..archive.len() {
        let mut entry=archive.by_index(index).map_err(|e|e.to_string())?;
        let name=entry.name().to_ascii_lowercase();
        if name.contains("vbaproject")||name.contains("/activex/")||name.contains("/embeddings/") {return Err("Active or embedded Office content is not permitted in the automatic export channel".into());}
        total=total.saturating_add(entry.size());if total>64*1024*1024 {return Err("Office expanded input exceeds 64 MiB".into());}
        if name.ends_with(".xml")||name.ends_with(".rels") {
            let mut xml=String::new();entry.by_ref().take(8*1024*1024+1).read_to_string(&mut xml).map_err(|e|e.to_string())?;
            if xml.len()>8*1024*1024 {return Err("Office XML part exceeds 8 MiB".into());}
            let document=roxmltree::Document::parse(&xml).map_err(|e|format!("Invalid Office XML: {e}"))?;
            if name.ends_with(".rels")&&document.descendants().any(|node|node.is_element()&&node.attribute("TargetMode").is_some_and(|mode|mode.eq_ignore_ascii_case("external"))) {return Err("External Office relationships require separate review before automation".into());}
            if name=="word/document.xml" {has_document=document.root_element().tag_name().name()=="document";}
        }
    }
    if !has_document {return Err("DOCX has no valid word/document.xml".into());}
    Ok(())
}

/// Fixed one-page synthetic input for an explicit feature check, never user material.
pub fn feature_docx()->Result<Vec<u8>> {
    let mut writer=zip::ZipWriter::new(Cursor::new(Vec::new()));
    let parts=[
        ("[Content_Types].xml",r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#),
        ("_rels/.rels",r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="document" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#),
        ("word/document.xml",r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:rPr><w:color w:val="000000"/><w:sz w:val="24"/></w:rPr><w:t>Office conversion check</w:t></w:r></w:p><w:p><w:r><w:t>文档转换验证 Alpha 123</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#),
    ];
    for (name,content) in parts {writer.start_file(name,zip::write::SimpleFileOptions::default()).map_err(|e|e.to_string())?;writer.write_all(content.as_bytes()).map_err(|e|e.to_string())?;}
    Ok(writer.finish().map_err(|e|e.to_string())?.into_inner())
}
/// A framing check proves neither PDF rendering nor visual document correctness.
pub fn validate_export_framing(bytes:&[u8])->Result<()> {
    if bytes.len()<16||bytes.len()>64*1024*1024||!bytes.starts_with(b"%PDF-")||!bytes[bytes.len().saturating_sub(1024)..].windows(5).any(|part|part==b"%%EOF") {return Err("WPS did not produce a complete bounded PDF stream".into());}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn feature_input_is_readable_ooxml_without_external_content() {validate_docx_for_automation(&feature_docx().unwrap()).unwrap();}
    #[test] fn external_relationships_are_detected_after_xml_entity_decoding() {
        let mut writer=zip::ZipWriter::new(Cursor::new(Vec::new()));writer.start_file("word/_rels/document.xml.rels",zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(br#"<Relationships><Relationship TargetMode="Exter&#110;al" Target="https://example.invalid"/></Relationships>"#).unwrap();
        let bytes=writer.finish().unwrap().into_inner();assert!(validate_docx_for_automation(&bytes).unwrap_err().contains("External"));
    }
    #[test] fn pdf_signature_alone_is_not_export_completion() {assert!(validate_export_framing(b"%PDF-1.7 incomplete").is_err());}
}
