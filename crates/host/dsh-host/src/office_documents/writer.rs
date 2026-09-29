use super::*;
use std::{collections::BTreeSet, io::Write};

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

fn escape(value: &str) -> Result<String> {
    if value.chars().any(|ch| {
        !matches!(ch, '\t' | '\n' | '\r')
            && ((ch as u32) < 0x20 || matches!(ch, '\u{fffe}' | '\u{ffff}'))
    }) {
        return Err("Office text contains an invalid XML character".into());
    }
    Ok(quick_xml::escape::escape(value)
        .into_owned()
        .replace('\r', "&#13;"))
}
fn scalar(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok(String::new()), Value::String(text) => {
            if text.chars().count() > 32767 { return Err("An Office cell exceeds 32767 characters".into()); }
            Ok(text.clone())
        }, Value::Number(_) | Value::Bool(_) => Ok(value.to_string()),
        _ => Err("Office cells must be strings, numbers, booleans or null; formulas and scripts are not accepted".into()),
    }
}
fn rows(value: &Value) -> Result<&Vec<Value>> {
    let rows = value.as_array().ok_or("Office rows must be an array")?;
    if rows.len() > 10000 {
        return Err("Office output exceeds 10000 rows".into());
    }
    for row in rows {
        let cells = row
            .as_array()
            .ok_or("Each Office row must be an array of scalar values")?;
        if cells.len() > 128 {
            return Err("Office output exceeds 128 columns".into());
        }
        for cell in cells {
            scalar(cell)?;
        }
    }
    Ok(rows)
}
fn paragraph(text: &str) -> Result<String> {
    let pieces = text
        .split('\n')
        .map(|part| escape(part).map(|part| format!("<w:t xml:space=\"preserve\">{part}</w:t>")))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("<w:p><w:r>{}</w:r></w:p>", pieces.join("<w:br/>")))
}
fn package(parts: Vec<(String, String)>, signal: &dyn Fn() -> bool) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in parts {
        cancelled(signal)?;
        if content.len() as u64 > MAX_PART {
            return Err("Generated Office XML part exceeds 8 MiB".into());
        }
        writer
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        writer
            .write_all(content.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    let bytes = writer
        .finish()
        .map_err(|error| error.to_string())?
        .into_inner();
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Generated Office document exceeds 32 MiB".into());
    }
    cancelled(signal)?;
    Ok(bytes)
}

fn docx(spec: &Value, signal: &dyn Fn() -> bool) -> Result<(Vec<u8>, Value)> {
    if spec.get("sheets").is_some() {
        return Err("DOCX output uses paragraphs and tables, not sheets".into());
    }
    let paragraphs = spec
        .get("paragraphs")
        .map(|value| {
            value
                .as_array()
                .ok_or("paragraphs must be an array of strings")
        })
        .transpose()?
        .cloned()
        .unwrap_or_default();
    let tables = spec
        .get("tables")
        .map(|value| value.as_array().ok_or("tables must be an array"))
        .transpose()?
        .cloned()
        .unwrap_or_default();
    if paragraphs.len() > 1000 || tables.len() > 32 {
        return Err("DOCX output exceeds its paragraph or table limit".into());
    }
    let mut body = String::new();
    let mut row_counts = Vec::new();
    let mut cell_count = 0;
    for value in &paragraphs {
        cancelled(signal)?;
        body.push_str(&paragraph(
            value.as_str().ok_or("DOCX paragraphs must be strings")?,
        )?);
    }
    for table in &tables {
        let table_rows = rows(&table["rows"])?;
        let columns = table_rows
            .iter()
            .filter_map(Value::as_array)
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .max(1);
        if table_rows.is_empty()
            || table_rows
                .iter()
                .any(|row| row.as_array().unwrap().len() != columns)
        {
            return Err("DOCX tables require at least one row and the same nonzero column count in each row".into());
        }
        row_counts.push(table_rows.len());
        body.push_str("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\"/><w:left w:val=\"single\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:sz=\"4\"/><w:right w:val=\"single\" w:sz=\"4\"/><w:insideH w:val=\"single\" w:sz=\"4\"/><w:insideV w:val=\"single\" w:sz=\"4\"/></w:tblBorders></w:tblPr><w:tblGrid>");
        for _ in 0..columns {
            body.push_str(&format!("<w:gridCol w:w=\"{}\"/>", 9360 / columns));
        }
        body.push_str("</w:tblGrid>");
        for row in table_rows {
            cancelled(signal)?;
            body.push_str("<w:tr>");
            for cell in row.as_array().unwrap() {
                cell_count += 1;
                if cell_count > MAX_CELLS {
                    return Err("Office output exceeds 20000 cells".into());
                }
                body.push_str(&format!(
                    "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
                    9360 / columns,
                    paragraph(&scalar(cell)?)?
                ));
            }
            body.push_str("</w:tr>");
        }
        body.push_str("</w:tbl>");
    }
    let empty_document = body.is_empty();
    if empty_document {
        body.push_str("<w:p/>");
    }
    body.push_str("<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\"/></w:sectPr>");
    let parts = vec![
        (
            "[Content_Types].xml".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="{TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#
            ),
        ),
        (
            "_rels/.rels".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{RELS}"><Relationship Id="office" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
            ),
        ),
        (
            "word/document.xml".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{WORD}"><w:body>{body}</w:body></w:document>"#
            ),
        ),
        (
            "word/_rels/document.xml.rels".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{RELS}"><Relationship Id="styles" Type="{REL}/styles" Target="styles.xml"/></Relationships>"#
            ),
        ),
        (
            "word/styles.xml".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:styles xmlns:w="{WORD}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="宋体"/><w:sz w:val="21"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#
            ),
        ),
    ];
    Ok((
        package(parts, signal)?,
        json!({"format":"docx","paragraphCount":paragraphs.len()+usize::from(empty_document),"tableCount":tables.len(),"tableRowCounts":row_counts,"cellCount":cell_count}),
    ))
}

fn xlsx(spec: &Value, signal: &dyn Fn() -> bool) -> Result<(Vec<u8>, Value)> {
    if spec.get("paragraphs").is_some() || spec.get("tables").is_some() {
        return Err("XLSX output uses sheets, not paragraphs or tables".into());
    }
    let sheets = spec["sheets"]
        .as_array()
        .filter(|sheets| !sheets.is_empty() && sheets.len() <= 32)
        .ok_or("XLSX sheets requires 1 to 32 worksheets")?;
    let mut names = BTreeSet::new();
    let mut workbook = String::new();
    let mut rels = String::new();
    let mut content_types = String::new();
    let mut parts = Vec::new();
    let mut summaries = Vec::new();
    let mut count = 0;
    for (index, sheet) in sheets.iter().enumerate() {
        cancelled(signal)?;
        let name=sheet["name"].as_str().filter(|name|!name.is_empty() && name.encode_utf16().count()<=31 && !name.starts_with('\'') && !name.ends_with('\'') && !name.contains(['[',']',':','*','?','/','\\'])).ok_or("Worksheet names require 1 to 31 characters without []:*?/\\ or surrounding apostrophes")?;
        if !names.insert(name.to_lowercase()) {
            return Err("Worksheet names must be unique".into());
        }
        let sheet_rows = rows(&sheet["rows"])?;
        let mut data = String::new();
        let mut max_columns = 0;
        let mut string_cells = 0;
        for (row_index, row) in sheet_rows.iter().enumerate() {
            cancelled(signal)?;
            data.push_str(&format!("<row r=\"{}\">", row_index + 1));
            for (column, value) in row.as_array().unwrap().iter().enumerate() {
                count += 1;
                if count > MAX_CELLS {
                    return Err("Office output exceeds 20000 cells".into());
                }
                max_columns = max_columns.max(column + 1);
                let reference = address(column + 1, row_index + 1);
                match value {
                    Value::Null => data.push_str(&format!("<c r=\"{reference}\"/>")),
                    Value::String(text) => {
                        string_cells += 1;
                        data.push_str(&format!("<c r=\"{reference}\" t=\"inlineStr\" s=\"1\"><is><t xml:space=\"preserve\">{}</t></is></c>",escape(&literal_xstring(text))?));
                    }
                    Value::Number(value) => {
                        let number = value.to_string();
                        if number.bytes().filter(u8::is_ascii_digit).count() > 15 {
                            return Err("Excel numbers exceed 15 significant decimal digits; supply identifiers and long literal numbers as strings".into());
                        }
                        data.push_str(&format!("<c r=\"{reference}\" t=\"n\"><v>{number}</v></c>"));
                    }
                    Value::Bool(value) => data.push_str(&format!(
                        "<c r=\"{reference}\" t=\"b\"><v>{}</v></c>",
                        if *value { 1 } else { 0 }
                    )),
                    _ => return Err("Unsupported Office cell value".into()),
                }
            }
            data.push_str("</row>");
        }
        let number = index + 1;
        let sheet_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{SHEET}"><sheetPr><pageSetUpPr fitToPage="1"/></sheetPr><dimension ref="A1:{}"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="20"/><cols><col min="1" max="{}" width="24" customWidth="1"/></cols><sheetData>{data}</sheetData><pageMargins left="0.3" right="0.3" top="0.5" bottom="0.5" header="0.2" footer="0.2"/><pageSetup paperSize="9" fitToWidth="1" fitToHeight="0" orientation="portrait"/></worksheet>"#,
            address(max_columns.max(1), sheet_rows.len().max(1)),
            max_columns.max(1)
        );
        parts.push((format!("xl/worksheets/sheet{number}.xml"), sheet_xml));
        workbook.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{number}\" r:id=\"sheet{number}\"/>",
            escape(name)?
        ));
        rels.push_str(&format!(r#"<Relationship Id="sheet{number}" Type="{REL}/worksheet" Target="worksheets/sheet{number}.xml"/>"#));
        content_types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{number}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        summaries.push(json!({"name":name,"rowCount":sheet_rows.len(),"columnCount":max_columns,"stringCells":string_cells}));
    }
    parts.extend([
        ("[Content_Types].xml".into(),format!(r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="{TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>{content_types}</Types>"#)),
        ("_rels/.rels".into(),format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{RELS}"><Relationship Id="office" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#)),
        ("xl/workbook.xml".into(),format!(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{SHEET}" xmlns:r="{REL}"><workbookPr date1904="0"/><bookViews><workbookView/></bookViews><sheets>{workbook}</sheets></workbook>"#)),
        ("xl/_rels/workbook.xml.rels".into(),format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{RELS}">{rels}<Relationship Id="styles" Type="{REL}/styles" Target="styles.xml"/></Relationships>"#)),
        ("xl/styles.xml".into(),format!(r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="{SHEET}"><fonts count="1"><font><sz val="11"/><name val="Arial"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="49" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"><alignment vertical="top" wrapText="1"/></xf></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#)),
    ]);
    Ok((
        package(parts, signal)?,
        json!({"format":"xlsx","sheetCount":sheets.len(),"sheets":summaries,"cellCount":count}),
    ))
}

pub(super) fn write(spec: &Value, signal: &dyn Fn() -> bool) -> Result<(Vec<u8>, Value)> {
    cancelled(signal)?;
    let (bytes, mut summary) = match spec["format"].as_str() {
        Some("docx") => docx(spec, signal)?,
        Some("xlsx") => xlsx(spec, signal)?,
        _ => {
            return Err(
                "OFFICE_FORMAT_UNSUPPORTED: office_write supports DOCX and XLSX only".into(),
            );
        }
    };
    let format = summary["format"].as_str().unwrap();
    crate::office_input_validation::validate_office_for_automation(&bytes, format)?;
    super::read(&bytes, &json!({"row_limit":1,"paragraph_limit":1}), signal)?;
    summary["structureChecked"] = json!(true);
    summary["formulasEvaluated"] = json!(false);
    summary["verificationScope"] = json!("file-structure");
    summary["sourceComparisonPerformed"] = json!(false);
    summary["visualInspectionPerformed"] = json!(false);
    Ok((bytes, summary))
}
