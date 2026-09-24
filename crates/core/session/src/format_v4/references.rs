use super::V3Dialect;
use super::wire::count;
use serde_json::{Value, json};

fn reference(value: &mut Value, source_seq: u64, mapping: &[u64]) -> Result<(), String> {
    let source = count(value, "V3 source event reference")?;
    if source >= source_seq {
        return Err("local reference must name an earlier source event".into());
    }
    let target = mapping
        .get(usize::try_from(source).map_err(|_| "reference index overflow")?)
        .ok_or("unmapped source event reference")?;
    *value = json!(target);
    Ok(())
}

fn references(value: &mut Value, seq: u64, mapping: &[u64]) -> Result<(), String> {
    for value in value
        .as_array_mut()
        .ok_or("local references must be an array")?
    {
        reference(value, seq, mapping)?;
    }
    Ok(())
}

pub(super) fn remap(
    row: &mut Value,
    seq: u64,
    mapping: &[u64],
    dialect: V3Dialect,
) -> Result<(), String> {
    let source = count(&row["seq"], "source seq")?;
    if let Some(value) = row.get_mut("sourceEventSeqs") {
        references(value, source, mapping)?;
    }
    if let Some(surface) = row.get_mut("surfaceOp") {
        if surface != "append" {
            let range = surface.as_object_mut().ok_or("invalid surface operation")?;
            if range.get("op").and_then(Value::as_str) != Some("replace") || range.len() != 3 {
                return Err("invalid replacement surface operation".into());
            }
            if dialect == V3Dialect::Rust
                && range.contains_key("start")
                && range.contains_key("end")
            {
                let start = range.remove("start").unwrap();
                let end = range.remove("end").unwrap();
                range.insert("startSeq".into(), start);
                range.insert("endSeq".into(), end);
            }
            for key in ["startSeq", "endSeq"] {
                reference(
                    range
                        .get_mut(key)
                        .ok_or("replacement requires startSeq and endSeq")?,
                    source,
                    mapping,
                )?;
            }
            // Endpoint order belongs to the live surface, not to log sequence:
            // an earlier replacement may be newer than its following node.
        }
    }
    match row["type"].as_str() {
        Some("command/done") => {
            if let Some(value) = row["data"].get_mut("sourceEventSeq") {
                reference(value, source, mapping)?;
            }
        }
        Some("compaction/summary" | "compaction/prune") => {
            let data = row["data"]
                .as_object_mut()
                .ok_or("compaction data must be an object")?;
            let range = data
                .get_mut("shadowedRange")
                .and_then(Value::as_object_mut)
                .ok_or("compaction requires shadowedRange")?;
            for key in ["start", "end"] {
                reference(
                    range.get_mut(key).ok_or("invalid shadowedRange")?,
                    source,
                    mapping,
                )?;
            }
            references(
                data.get_mut("shadowedSeqs")
                    .ok_or("compaction requires shadowedSeqs")?,
                source,
                mapping,
            )?;
        }
        Some("session/title" | "session/title-llm-request") => references(
            row["data"]
                .get_mut("messageSeqs")
                .ok_or("title requires messageSeqs")?,
            source,
            mapping,
        )?,
        Some("image/offload") => {
            for target in row["data"]
                .get_mut("targets")
                .and_then(Value::as_array_mut)
                .ok_or("image/offload requires targets")?
            {
                reference(
                    target.get_mut("seq").ok_or("offload target requires seq")?,
                    source,
                    mapping,
                )?;
            }
        }
        _ => {}
    }
    row["seq"] = json!(seq);
    Ok(())
}
