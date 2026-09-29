use super::*;

fn word_blocks<'a, 'input>(node: Node<'a, 'input>) -> Vec<Node<'a, 'input>> {
    let mut output = Vec::new();
    for child in node.children().filter(Node::is_element) {
        if named(child, "p") || named(child, "tbl") {
            output.push(child);
        } else {
            output.extend(word_blocks(child));
        }
    }
    output
}

pub(super) fn docx(
    archive: &mut Archive<'_>,
    options: &Value,
    start: usize,
    limit: usize,
    signal: &dyn Fn() -> bool,
) -> Result<Value> {
    let text = part(archive, "word/document.xml")?;
    let document = xml(&text)?;
    let body = document
        .descendants()
        .find(|node| named(*node, "body"))
        .ok_or("DOCX body is missing")?;
    let paragraph_limit = integer(options, "paragraph_limit", 200, 1000)? as usize;
    let blocks = word_blocks(body);
    let paragraphs = blocks
        .iter()
        .copied()
        .filter(|node| named(*node, "p"))
        .collect::<Vec<_>>();
    let all_tables = body
        .descendants()
        .filter(|node| named(*node, "tbl"))
        .collect::<Vec<_>>();
    let selected = if options.get("table_index").is_some() {
        Some(integer(options, "table_index", 1, 10000)? as usize)
    } else {
        None
    };
    if selected.is_some_and(|index| index > all_tables.len()) {
        return Err("DOCX table_index is outside the available tables".into());
    }
    if selected.is_none() && all_tables.len() > 32 {
        return Err("Use table_index to select one of this document's tables".into());
    }
    let mut cells = 0usize;
    let mut tables = Vec::new();
    for (index, table) in all_tables.iter().enumerate() {
        if selected.is_some_and(|selected| selected != index + 1) {
            continue;
        }
        cancelled(signal)?;
        let source_rows = table
            .descendants()
            .filter(|node| {
                named(*node, "tr")
                    && node
                        .ancestors()
                        .skip(1)
                        .find(|parent| named(*parent, "tbl"))
                        == Some(*table)
            })
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for (row_index, row) in source_rows.iter().enumerate().skip(start - 1).take(limit) {
            cancelled(signal)?;
            let mut values = Vec::new();
            let mut column = row
                .children()
                .find(|node| named(*node, "trPr"))
                .and_then(|properties| {
                    properties
                        .children()
                        .find(|node| named(*node, "gridBefore"))
                })
                .and_then(|node| attr(node, "val"))
                .map(str::parse::<usize>)
                .transpose()
                .map_err(|_| "Invalid DOCX gridBefore")?
                .unwrap_or(0);
            if column >= 16384 {
                return Err("Invalid DOCX gridBefore".into());
            }
            column += 1;
            for cell in row.descendants().filter(|node| {
                named(*node, "tc")
                    && node.ancestors().skip(1).find(|parent| named(*parent, "tr")) == Some(*row)
            }) {
                cells += 1;
                if cells > MAX_CELLS {
                    return Err(
                        "Office result exceeds 20000 cells; select a smaller row range".into(),
                    );
                }
                let cell_blocks = word_blocks(cell);
                let paragraphs = cell_blocks
                    .iter()
                    .copied()
                    .filter(|node| named(*node, "p"))
                    .map(|paragraph| visible_text(paragraph, true))
                    .collect::<Vec<_>>();
                let span = cell
                    .children()
                    .find(|node| named(*node, "tcPr"))
                    .and_then(|props| props.children().find(|node| named(*node, "gridSpan")))
                    .and_then(|node| attr(node, "val"))
                    .unwrap_or("1")
                    .parse::<usize>()
                    .map_err(|_| "Invalid DOCX gridSpan")?;
                if span == 0 || span > 16384 {
                    return Err("Invalid DOCX gridSpan".into());
                }
                let merge = cell
                    .children()
                    .find(|node| named(*node, "tcPr"))
                    .and_then(|props| props.children().find(|node| named(*node, "vMerge")))
                    .map(|node| attr(node, "val").unwrap_or("continue"));
                if column + span > 16385 {
                    return Err("DOCX table column span exceeds the column limit".into());
                }
                let nested = cell_blocks
                    .iter()
                    .filter(|node| named(**node, "tbl"))
                    .filter_map(|node| {
                        all_tables
                            .iter()
                            .position(|table| table == node)
                            .map(|index| index + 1)
                    })
                    .collect::<Vec<_>>();
                values.push(json!({"column":column,"text":paragraphs.join("\n"),"paragraphs":paragraphs,"gridSpan":span,"verticalMerge":merge,"nestedTables":nested}));
                column += span;
            }
            rows.push(json!({"row":row_index + 1,"cells":values}));
        }
        let parent = table
            .ancestors()
            .skip(1)
            .find(|parent| named(*parent, "tbl"))
            .and_then(|parent| {
                all_tables
                    .iter()
                    .position(|table| *table == parent)
                    .map(|index| index + 1)
            });
        tables.push(json!({"index":index+1,"parentTable":parent,"rowCount":source_rows.len(),"startRow":start,"returnedRows":rows.len(),"hasMoreRows":start.saturating_sub(1)+rows.len()<source_rows.len(),"rows":rows}));
    }
    let mut paragraph_index = 0;
    let order = blocks
        .iter()
        .map(|node| {
            if named(*node, "p") {
                paragraph_index += 1;
                json!({"type":"paragraph","index":paragraph_index})
            } else {
                json!({"type":"table","index":all_tables.iter().position(|table|table==node).unwrap()+1})
            }
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"bodyOrder":order,"paragraphCount":paragraphs.len(),"paragraphs":paragraphs.iter().take(paragraph_limit).enumerate().map(|(index,node)|json!({"index":index+1,"text":visible_text(*node,true)})).collect::<Vec<_>>(),"hasMoreParagraphs":paragraphs.len()>paragraph_limit,"tableCount":all_tables.len(),"tables":tables}),
    )
}

fn target(base: &str, relative: &str) -> Result<String> {
    if relative.contains(['\\', ':', '\0']) {
        return Err("Invalid internal worksheet relationship".into());
    }
    let combined = if relative.starts_with('/') {
        relative.trim_start_matches('/').to_owned()
    } else {
        format!("{base}/{relative}")
    };
    let mut parts = Vec::new();
    for part in combined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("Worksheet relationship escapes the package".into());
                }
            }
            _ => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}
fn formats(archive: &mut Archive<'_>) -> Result<Vec<String>> {
    let mut names: BTreeMap<u64, String> = [
        (0, "General"),
        (1, "0"),
        (2, "0.00"),
        (3, "#,##0"),
        (4, "#,##0.00"),
        (9, "0%"),
        (10, "0.00%"),
        (14, "mm-dd-yy"),
        (49, "@"),
    ]
    .into_iter()
    .map(|(id, name)| (id, name.into()))
    .collect();
    if !archive.file_names().any(|name| name == "xl/styles.xml") {
        return Ok(vec!["General".into()]);
    }
    let source = part(archive, "xl/styles.xml")?;
    let document = xml(&source)?;
    for node in document.descendants().filter(|node| named(*node, "numFmt")) {
        if let (Some(id), Some(code)) = (
            attr(node, "numFmtId").and_then(|id| id.parse().ok()),
            attr(node, "formatCode"),
        ) {
            names.insert(id, code.into());
        }
    }
    let formats = document
        .descendants()
        .find(|node| named(*node, "cellXfs"))
        .map(|node| {
            node.children()
                .filter(|child| named(*child, "xf"))
                .map(|xf| {
                    let id = attr(xf, "numFmtId")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0);
                    names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| format!("builtin:{id}"))
                })
                .collect()
        })
        .unwrap_or_else(|| vec!["General".into()]);
    Ok(formats)
}
fn numeric_display(raw: &str, format: &str) -> (String, &'static str) {
    let trimmed = raw.trim();
    if !format.is_empty()
        && format.len() <= 128
        && format.bytes().all(|byte| byte == b'0')
        && !trimmed.is_empty()
        && trimmed.bytes().all(|byte| byte.is_ascii_digit())
    {
        (
            format!("{trimmed:0>width$}", width = format.len()),
            "zero_padding",
        )
    } else {
        (raw.into(), "raw_not_rendered")
    }
}

pub(super) fn xlsx(
    archive: &mut Archive<'_>,
    options: &Value,
    start: usize,
    limit: usize,
    signal: &dyn Fn() -> bool,
) -> Result<Value> {
    let workbook = part(archive, "xl/workbook.xml")?;
    let relationships = part(archive, "xl/_rels/workbook.xml.rels")?;
    let document = xml(&workbook)?;
    let rels = xml(&relationships)?;
    let mut targets = BTreeMap::new();
    for rel in rels
        .descendants()
        .filter(|node| named(*node, "Relationship"))
    {
        if attr(rel, "Type").is_some_and(|value| value.ends_with("/worksheet")) {
            if attr(rel, "TargetMode") == Some("External") {
                return Err("External worksheet relationships are unsupported".into());
            }
            targets.insert(
                attr(rel, "Id")
                    .ok_or("Worksheet relationship ID missing")?
                    .to_owned(),
                target(
                    "xl",
                    attr(rel, "Target").ok_or("Worksheet relationship target missing")?,
                )?,
            );
        }
    }
    let selected = options["sheet_name"].as_str();
    let mut shared = Vec::new();
    if archive
        .file_names()
        .any(|name| name == "xl/sharedStrings.xml")
    {
        let source = part(archive, "xl/sharedStrings.xml")?;
        let strings = xml(&source)?;
        shared = strings
            .descendants()
            .filter(|node| named(*node, "si"))
            .map(sheet_text)
            .collect::<Result<Vec<_>>>()?;
    }
    let formats = formats(archive)?;
    let definitions = document
        .descendants()
        .filter(|node| named(*node, "sheet"))
        .collect::<Vec<_>>();
    if definitions.len() > 32 && selected.is_none() {
        return Err("Use sheet_name to select one of this workbook's worksheets".into());
    }
    let mut sheets = Vec::new();
    let mut cells = 0usize;
    let mut output_bytes = 0usize;
    for (index, sheet) in definitions.iter().enumerate() {
        let name = attr(*sheet, "name").ok_or("Worksheet name is missing")?;
        if selected.is_some_and(|selected| selected != name) {
            continue;
        }
        cancelled(signal)?;
        let id = attr(*sheet, "id").ok_or("Worksheet relationship is missing")?;
        let source = part(
            archive,
            targets
                .get(id)
                .ok_or("Worksheet relationship is unresolved")?,
        )?;
        let sheet_doc = xml(&source)?;
        let mut rows = Vec::new();
        let mut last_row = 0usize;
        let mut total_rows = 0usize;
        let mut max_column = 0usize;
        for row in sheet_doc.descendants().filter(|node| named(*node, "row")) {
            cancelled(signal)?;
            let row_index = attr(row, "r")
                .map(str::parse::<usize>)
                .transpose()
                .map_err(|_| "Invalid worksheet row index")?
                .unwrap_or(last_row + 1);
            if row_index <= last_row || row_index > 1_048_576 {
                return Err("Invalid or unordered worksheet rows".into());
            }
            last_row = row_index;
            total_rows += 1;
            if row_index < start || row_index >= start.saturating_add(limit) {
                continue;
            }
            let mut values = Vec::new();
            let mut previous_column = 0;
            for cell in row.children().filter(|node| named(*node, "c")) {
                cells += 1;
                if cells > MAX_CELLS {
                    return Err(
                        "Office result exceeds 20000 cells; select a smaller row range".into(),
                    );
                }
                let column = attr(cell, "r")
                    .and_then(column_of)
                    .unwrap_or(previous_column + 1);
                if column <= previous_column || column > 16_384 {
                    return Err("Invalid or unordered worksheet cells".into());
                }
                let reference = address(column, row_index);
                if attr(cell, "r").is_some_and(|actual| actual != reference) {
                    return Err("Worksheet cell address does not match its row".into());
                }
                previous_column = column;
                max_column = max_column.max(column);
                let raw = cell
                    .children()
                    .find(|node| named(*node, "v"))
                    .and_then(|node| node.text())
                    .unwrap_or("");
                let kind = attr(cell, "t").unwrap_or("n");
                let style = attr(cell, "s")
                    .map(str::parse::<usize>)
                    .transpose()
                    .map_err(|_| "Invalid cell style")?
                    .unwrap_or(0);
                let format = formats.get(style).ok_or("Cell style does not exist")?;
                let (cell_type, value, display_kind) = match kind {
                    "s" => {
                        let at = raw
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| "Invalid shared string index")?;
                        (
                            "text",
                            shared.get(at).ok_or("Missing shared string")?.clone(),
                            "literal",
                        )
                    }
                    "inlineStr" => ("text", sheet_text(cell)?, "literal"),
                    "str" => ("text", decode_xstring(raw)?, "literal"),
                    "b" => (
                        "boolean",
                        if raw == "1" {
                            "TRUE"
                        } else if raw == "0" {
                            "FALSE"
                        } else {
                            return Err("Invalid boolean cell value".into());
                        }
                        .into(),
                        "logical",
                    ),
                    "e" => ("error", raw.into(), "literal"),
                    "d" => ("date", raw.into(), "literal"),
                    "n" if raw.is_empty() => ("blank", String::new(), "literal"),
                    "n" => {
                        let (value, source) = numeric_display(raw, format);
                        ("number", value, source)
                    }
                    _ => return Err(format!("Unsupported worksheet cell type: {kind}")),
                };
                let formula = cell
                    .children()
                    .find(|node| named(*node, "f"))
                    .map(|node| node.text().unwrap_or(""));
                output_bytes = output_bytes.saturating_add(
                    value.len() + raw.len() * 2 + format.len() + formula.map(str::len).unwrap_or(0),
                );
                if output_bytes > 4 * 1024 * 1024 {
                    return Err("Office result exceeds 4 MiB; select a smaller row range".into());
                }
                values.push(json!({"address":reference,"column":column,"type":cell_type,"storageType":kind,"rawValue":if cell_type=="text"{value.as_str()}else{raw},"storedValue":raw,"displayValue":value,"displayValueKind":display_kind,"numberFormat":format,"formula":formula,"valueSource":if formula.is_some(){"cached"}else{"stored"}}));
            }
            rows.push(json!({"row":row_index,"cells":values}));
        }
        sheets.push(json!({"name":name,"index":index+1,"storedRowCount":total_rows,"lastRow":last_row,"selectedMaxColumn":max_column,"startRow":start,"returnedRows":rows.len(),"hasMoreRows":last_row>=start.saturating_add(limit),"rows":rows}));
    }
    if selected.is_some() && sheets.is_empty() {
        return Err("sheet_name is not present in this workbook".into());
    }
    Ok(
        json!({"sheetNames":definitions.iter().map(|node|attr(*node,"name").unwrap_or("")).collect::<Vec<_>>(),"dateSystem":if document.descendants().find(|node|named(*node,"workbookPr")).and_then(|node|attr(node,"date1904"))==Some("1"){ "1904" }else{"1900"},"sheets":sheets}),
    )
}
