use super::*;
use std::io::Write;

fn spec(format: &str, rows: Value) -> Value {
    if format == "docx" {
        json!({"format":format,"paragraphs":["联系电话名册"],"tables":[{"rows":rows}]})
    } else {
        json!({"format":format,"sheets":[{"name":"联系电话","rows":rows}]})
    }
}
fn selected(value: &Value) -> Vec<Vec<String>> {
    let docx = value["format"] == "docx";
    let rows = if docx {
        &value["tables"][0]["rows"]
    } else {
        &value["sheets"][0]["rows"]
    };
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|cell| {
                    cell[if docx { "text" } else { "rawValue" }]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect()
        })
        .collect()
}
fn replace_parts(bytes: &[u8], changes: &[(&str, String)]) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    let mut parts = Vec::new();
    for name in &names {
        let text = changes
            .iter()
            .find(|(target, _)| target == name)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| part(&mut archive, name).unwrap());
        parts.push((name.clone(), text));
    }
    for (name, text) in changes {
        if !names.iter().any(|stored| stored == name) {
            parts.push(((*name).into(), text.clone()));
        }
    }
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in parts {
        output
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(text.as_bytes()).unwrap();
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn seven_selected_rows_roundtrip_as_real_ooxml_with_literal_phone_values() {
    let rows = (0..12)
        .map(|index| {
            json!([
                format!("人员 {index}"),
                format!("001380000{index:04}"),
                format!("分机 +0086-010-00{index:02} & <原值>")
            ])
        })
        .collect::<Vec<_>>();
    let expected = rows[2..9]
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for input_format in ["docx", "xlsx"] {
        let (input, summary) = write(&spec(input_format, json!(rows)), &|| false).unwrap();
        assert!(input.starts_with(b"PK\x03\x04"));
        assert_eq!(summary["structureChecked"], true);
        let selection = read(&input, &json!({"start_row":3,"row_limit":7}), &|| false).unwrap();
        assert_eq!(selected(&selection), expected);
        for output_format in ["docx", "xlsx"] {
            let (output, count) =
                write(&spec(output_format, json!(selected(&selection))), &|| false).unwrap();
            assert_eq!(
                selected(&read(&output, &json!({}), &|| false).unwrap()),
                expected
            );
            assert_eq!(
                if output_format == "docx" {
                    &count["tableRowCounts"][0]
                } else {
                    &count["sheets"][0]["rowCount"]
                },
                7
            );
            crate::office_input_validation::validate_office_for_automation(&output, output_format)
                .unwrap();
        }
    }
}

#[test]
fn string_cells_are_never_formula_or_numeric_inputs() {
    let values = json!([[
        "000123",
        "+8613812345678",
        "=1+1",
        "12345678901234567890",
        "电话\n第二行",
        null,
        true,
        12.5,
        "_x0041_",
        "_x005F_",
        "电话🙂"
    ]]);
    let (bytes, _) = write(&spec("xlsx", values), &|| false).unwrap();
    let value = read(&bytes, &json!({}), &|| false).unwrap();
    let cells = &value["sheets"][0]["rows"][0]["cells"];
    assert_eq!(cells[0]["rawValue"], "000123");
    assert_eq!(cells[2]["rawValue"], "=1+1");
    assert!(cells[2]["formula"].is_null());
    assert_eq!(cells[3]["type"], "text");
    assert_eq!(cells[4]["rawValue"], "电话\n第二行");
    assert_eq!(cells[5]["type"], "blank");
    assert_eq!(cells[6]["type"], "boolean");
    assert_eq!(cells[7]["rawValue"], "12.5");
    assert_eq!(cells[8]["rawValue"], "_x0041_");
    assert_eq!(cells[9]["rawValue"], "_x005F_");
    assert_eq!(cells[10]["rawValue"], "电话🙂");
    let mut package = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert!(
        part(&mut package, "xl/worksheets/sheet1.xml")
            .unwrap()
            .contains("_x005F_x0041_")
    );
    assert!(write(&spec("xlsx", json!([[12345678901234567890u64]])), &|| false).is_err());
}

#[test]
fn numeric_raw_value_and_zero_padding_display_remain_distinct() {
    let (bytes, _) = write(&spec("xlsx", json!([[123]])), &|| false).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let sheet = part(&mut archive, "xl/worksheets/sheet1.xml")
        .unwrap()
        .replace("r=\"A1\" t=\"n\"", "r=\"A1\" t=\"n\" s=\"2\"")
        .replace("<v>123</v>", "<f>1+122</f><v>123</v>");
    let styles = part(&mut archive, "xl/styles.xml")
        .unwrap()
        .replace(
            "<fonts",
            "<numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"000000\"/></numFmts><fonts",
        )
        .replace("<cellXfs count=\"2\">", "<cellXfs count=\"3\">")
        .replace(
            "</cellXfs>",
            "<xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellXfs>",
        );
    let changed = replace_parts(
        &bytes,
        &[
            ("xl/worksheets/sheet1.xml", sheet),
            ("xl/styles.xml", styles),
        ],
    );
    let value = read(&changed, &json!({}), &|| false).unwrap();
    let cell = &value["sheets"][0]["rows"][0]["cells"][0];
    assert_eq!(cell["rawValue"], "123");
    assert_eq!(cell["displayValue"], "000123");
    assert_eq!(cell["numberFormat"], "000000");
    assert_eq!(cell["formula"], "1+122");
    assert_eq!(cell["valueSource"], "cached");
    assert_eq!(value["formulasEvaluated"], false);
}

#[test]
fn rejects_unsupported_corrupt_active_and_cancelled_documents() {
    let (empty, summary) = write(&json!({"format":"docx"}), &|| false).unwrap();
    assert_eq!(
        summary["paragraphCount"],
        read(&empty, &json!({}), &|| false).unwrap()["paragraphCount"]
    );
    assert!(
        read(b"<html><table/></html>", &json!({}), &|| false)
            .unwrap_err()
            .contains("UNSUPPORTED")
    );
    assert!(read(b"PK broken", &json!({}), &|| false).is_err());
    let (bytes, _) = write(&spec("docx", json!([["input"]])), &|| false).unwrap();
    let active = replace_parts(&bytes, &[("word/vbaProject.bin", "synthetic".into())]);
    assert!(
        read(&active, &json!({}), &|| false)
            .unwrap_err()
            .to_lowercase()
            .contains("active")
    );
    assert!(
        read(&bytes, &json!({}), &|| true)
            .unwrap_err()
            .contains("ABORTED")
    );
    assert!(
        write(&spec("docx", json!([["input"]])), &|| true)
            .unwrap_err()
            .contains("ABORTED")
    );
    assert!(write(&spec("docx", json!([["one"], ["two", "three"]])), &|| false).is_err());
    assert!(write(&json!({"format":"docx","tables":[{"rows":[]}]}), &|| false).is_err());
    assert!(read(&bytes, &json!({"start_row":0}), &|| false).is_err());
}

#[test]
fn existing_shared_strings_and_sparse_coordinates_are_preserved() {
    let (bytes, _) = write(&spec("xlsx", json!([["placeholder"]])), &|| false).unwrap();
    let mut package = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let sheet = part(&mut package, "xl/worksheets/sheet1.xml")
        .unwrap()
        .replace("<row r=\"1\">", "<row r=\"3\">")
        .replace("r=\"A1\" t=\"inlineStr\" s=\"1\"", "r=\"B3\" t=\"s\"")
        .replace(
            "<is><t xml:space=\"preserve\">placeholder</t></is>",
            "<v>0</v>",
        );
    let shared = r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><r><t>00138</t></r><r><t>12345678 _x005F_x0041_</t></r><rPh sb="0" eb="1"><t>annotation</t></rPh></si></sst>"#;
    let types=part(&mut package,"[Content_Types].xml").unwrap().replace("</Types>",r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#);
    let rels=part(&mut package,"xl/_rels/workbook.xml.rels").unwrap().replace("</Relationships>",r#"<Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#);
    let input = replace_parts(
        &bytes,
        &[
            ("xl/worksheets/sheet1.xml", sheet),
            ("xl/sharedStrings.xml", shared.into()),
            ("[Content_Types].xml", types),
            ("xl/_rels/workbook.xml.rels", rels),
        ],
    );
    let read = read(&input, &json!({"start_row":3,"row_limit":1}), &|| false).unwrap();
    let cell = &read["sheets"][0]["rows"][0]["cells"][0];
    assert_eq!(cell["address"], "B3");
    assert_eq!(cell["column"], 2);
    assert_eq!(cell["storedValue"], "0");
    assert_eq!(cell["rawValue"], "0013812345678 _x0041_");
}

#[test]
fn word_content_controls_and_nested_table_positions_are_explicit() {
    let (bytes, _) = write(&spec("docx", json!([["outer"]])), &|| false).unwrap();
    let mut package = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let text=part(&mut package,"word/document.xml").unwrap()
        .replacen("<w:p>","<w:sdt><w:sdtContent><w:p>",1)
        .replacen("</w:p>","</w:p></w:sdtContent></w:sdt>",1)
        .replacen("</w:tc>","<w:tbl><w:tr><w:tc><w:p><w:r><w:t>00123</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc>",1);
    let input = replace_parts(&bytes, &[("word/document.xml", text)]);
    let data = read(&input, &json!({}), &|| false).unwrap();
    assert_eq!(data["paragraphs"][0]["text"], "联系电话名册");
    assert_eq!(
        data["bodyOrder"],
        json!([{"type":"paragraph","index":1},{"type":"table","index":1}])
    );
    assert_eq!(
        data["tables"][0]["rows"][0]["cells"][0]["nestedTables"],
        json!([2])
    );
    assert_eq!(data["tables"][1]["parentTable"], 1);
    assert_eq!(data["tables"][1]["rows"][0]["cells"][0]["text"], "00123");
}

#[test]
fn ordinary_external_hyperlinks_remain_readable_without_weakening_automation() {
    let relation = r#"<Relationship Id="contact-link" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="mailto:contact@example.invalid" TargetMode="External"/>"#;
    let expected = vec![vec!["张三".to_string(), "0013812345678".to_string()]];
    for format in ["docx", "xlsx"] {
        let (bytes, _) = write(&spec(format, json!(expected)), &|| false).unwrap();
        let mut source = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let (part_name, relations_name, xml, relationships) = if format == "docx" {
            let document=part(&mut source,"word/document.xml").unwrap()
                .replacen("<w:document ","<w:document xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",1)
                .replace("<w:r><w:t xml:space=\"preserve\">0013812345678</w:t></w:r>","<w:hyperlink r:id=\"contact-link\"><w:r><w:t xml:space=\"preserve\">0013812345678</w:t></w:r></w:hyperlink>");
            let relationships = part(&mut source, "word/_rels/document.xml.rels")
                .unwrap()
                .replace("</Relationships>", &format!("{relation}</Relationships>"));
            (
                "word/document.xml",
                "word/_rels/document.xml.rels",
                document,
                relationships,
            )
        } else {
            let document=part(&mut source,"xl/worksheets/sheet1.xml").unwrap()
                .replacen("<worksheet ","<worksheet xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",1)
                .replace("</sheetData>","</sheetData><hyperlinks><hyperlink ref=\"B1\" r:id=\"contact-link\"/></hyperlinks>");
            let relationships = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relation}</Relationships>"#
            );
            (
                "xl/worksheets/sheet1.xml",
                "xl/worksheets/_rels/sheet1.xml.rels",
                document,
                relationships,
            )
        };
        let input = replace_parts(
            &bytes,
            &[(part_name, xml), (relations_name, relationships.clone())],
        );
        assert_eq!(
            selected(&read(&input, &json!({}), &|| false).unwrap()),
            expected
        );
        assert!(
            crate::office_input_validation::validate_office_for_automation(&input, format)
                .unwrap_err()
                .contains("External Office relationships")
        );
        let dtd=relationships.replacen("<Relationships ","<!DOCTYPE Relationships [<!ENTITY unused SYSTEM 'file:///unused-synthetic'>]><Relationships ",1);
        let unsafe_input = replace_parts(&input, &[(relations_name, dtd)]);
        assert!(
            read(&unsafe_input, &json!({}), &|| false)
                .unwrap_err()
                .to_lowercase()
                .contains("dtd")
        );
    }
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires the explicitly selected local WPS automation installation"]
async fn generated_ooxml_opens_in_wps_and_exports_actual_pdf() {
    let directory = std::env::var_os("DSH_OFFICE_SYNTHETIC_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("dsh-office-open-{}", uuid::Uuid::new_v4()))
        });
    std::fs::create_dir_all(&directory).unwrap();
    let rows = (0..7)
        .map(|index| {
            json!([
                format!("人员 {}", index + 1),
                format!("001380000{index:04}"),
                "010-00000000"
            ])
        })
        .collect::<Vec<_>>();
    let office = crate::office_preview::shared();
    for format in ["docx", "xlsx"] {
        let (bytes, _) = write(&spec(format, json!(rows)), &|| false).unwrap();
        let input = directory.join(format!("seven-rows.{format}"));
        std::fs::write(&input, &bytes).unwrap();
        let (_, pdf) = office.export(&input).await.unwrap();
        assert!(pdf.bytes > 16);
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
        std::fs::copy(
            &pdf.path,
            directory.join(format!("seven-rows-{format}.pdf")),
        )
        .unwrap();
        assert_eq!(
            selected(&read(&std::fs::read(&input).unwrap(), &json!({}), &|| false).unwrap()).len(),
            7
        );
    }
}
