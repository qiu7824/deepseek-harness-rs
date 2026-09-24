use super::*;
use crate::StorageRecord;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};

struct RecordedWriter {
    file: BufWriter<std::fs::File>,
    hash: Sha256,
}
impl Write for RecordedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.file.write(bytes)?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[test]
#[ignore = "requires an explicitly prepared private corpus manifest"]
fn private_corpus_streaming_conversion() {
    let manifest = std::env::var_os("DSH_V4_CORPUS_MANIFEST").expect("explicit corpus manifest");
    let manifest: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let output = std::path::PathBuf::from(
        std::env::var_os("DSH_V4_CORPUS_REPORT").expect("explicit corpus report path"),
    );
    let staged = output.parent().unwrap().join(format!(
        "v4-staged-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&staged).unwrap();
    let mut reports = Vec::new();
    let mut all_valid = true;
    for (index, fixture) in manifest["fixtures"].as_array().unwrap().iter().enumerate() {
        let file = std::fs::File::open(fixture["path"].as_str().unwrap()).unwrap();
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let physical: Value = serde_json::from_str(&line).unwrap();
        let (header, inherited) = decode_v3_header(physical, V3Dialect::Rust).unwrap();
        let mut stage = V3ToV4Transform::new(
            header,
            Some(fixture["children"].as_array().unwrap().clone()),
            inherited,
            V3Dialect::Rust,
        )
        .unwrap();
        let rows_path = staged.join(format!("logical-rows-{index}.jsonl"));
        let mut writer = RecordedWriter {
            file: BufWriter::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&rows_path)
                    .unwrap(),
            ),
            hash: Sha256::new(),
        };
        let mut count = 0;
        let mut tool_results = 0;
        let mut inspect = |row: Value| -> Result<(), String> {
            serde_json::to_writer(&mut writer, &row).map_err(|e| e.to_string())?;
            writer.write_all(b"\n").map_err(|e| e.to_string())?;
            count += 1;
            if row["type"] == "tool/result" {
                tool_results += 1;
            }
            Ok(())
        };
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            let value: Value = serde_json::from_str(&line).unwrap();
            let source_seq = value
                .get("seq")
                .or_else(|| value.get("seq0"))
                .cloned()
                .unwrap_or(Value::Null);
            let result = match StorageRecord::from_json(value).unwrap() {
                StorageRecord::Event(value) => stage.push(value).and_then(|rows| {
                    for row in rows {
                        inspect(row)?;
                    }
                    Ok(())
                }),
                packed => crate::visit_decoded_storage_record_events(packed, |event| {
                    for row in stage.push(serde_json::to_value(event).unwrap())? {
                        inspect(row)?;
                    }
                    Ok(true)
                })
                .map(|_| ()),
            };
            assert!(
                result.is_ok(),
                "fixture {} source seq {source_seq}: {}",
                fixture["id"],
                result.unwrap_err()
            );
        }
        let (summary, catalog) = stage.finish().unwrap();
        for row in catalog {
            inspect(row).unwrap();
        }
        drop(inspect);
        writer.flush().unwrap();
        let RecordedWriter { file, hash } = writer;
        drop(file);
        assert_eq!(summary.event_count, count);
        let mut validator =
            V4Validator::new(summary.header.clone(), summary.inherited_event_count).unwrap();
        let mut native_error = None;
        for line in BufReader::new(std::fs::File::open(&rows_path).unwrap()).lines() {
            let row: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if let Err(error) = validator.push(&row) {
                native_error = Some(json!({"seq":row["seq"],"type":row["type"],"error":error}));
                break;
            }
        }
        let native = if let Some(error) = native_error {
            all_valid = false;
            json!({"ok":false,"failure":error})
        } else {
            match validator.finish() {
                Ok(value) => {
                    json!({"ok":true,"validatedEvents":value.event_count,"openTurn":value.open_turn,"openStep":value.open_step,"pendingTools":value.pending_tools,"openCompaction":value.open_compaction})
                }
                Err(error) => {
                    all_valid = false;
                    json!({"ok":false,"failure":{"phase":"finish","error":error}})
                }
            }
        };
        reports.push(json!({"id":fixture["id"],"sourceEvents":summary.source_offsets.len(),"targetEvents":count,"toolResults":tool_results,"inheritedEventCount":summary.inherited_event_count,"sha256":format!("{:x}",hash.finalize()),"logicalRows":rows_path,"nativeValidation":native}));
    }
    std::fs::write(&output,serde_json::to_vec_pretty(&json!({"fixtures":reports,"rowConversionPassed":true,"fullLifecycleValidation":all_valid,"published":false})).unwrap()).unwrap();
    assert!(
        all_valid,
        "native V4 corpus validation failed; see {}",
        output.display()
    );
}

#[test]
#[ignore = "requires explicitly prepared private logical generations"]
fn private_corpus_physical_roundtrip() {
    let input: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("DSH_V4_NATIVE_REPORT").expect("native report")).unwrap(),
    )
    .unwrap();
    assert_eq!(input["fullLifecycleValidation"], true);
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("DSH_V4_CORPUS_MANIFEST").expect("corpus manifest"))
            .unwrap(),
    )
    .unwrap();
    let output =
        std::path::PathBuf::from(std::env::var_os("DSH_V4_CODEC_REPORT").expect("codec report"));
    let directory = output.parent().unwrap().join(format!(
        "v4-physical-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let vocabulary = V4Vocabulary::default();
    let mut reports = vec![];
    for (index, fixture) in input["fixtures"].as_array().unwrap().iter().enumerate() {
        let original = manifest["fixtures"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == fixture["id"])
            .unwrap();
        let mut source =
            BufReader::new(std::fs::File::open(original["path"].as_str().unwrap()).unwrap());
        let mut line = String::new();
        source.read_line(&mut line).unwrap();
        let (mut header, _) =
            decode_v3_header(serde_json::from_str(&line).unwrap(), V3Dialect::Rust).unwrap();
        header["version"] = json!(4);
        let cut = fixture["inheritedEventCount"].as_u64().unwrap();
        let physical_path = directory.join(format!("session-{index}.v4.jsonl"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&physical_path)
            .unwrap();
        let mut writer = RecordedWriter {
            file: BufWriter::new(file),
            hash: Sha256::new(),
        };
        serde_json::to_writer(&mut writer, &encode_v4_header(header.clone(), cut).unwrap())
            .unwrap();
        writer.write_all(b"\n").unwrap();
        let logical_path = fixture["logicalRows"].as_str().unwrap();
        let mut compressed_refs = 0;
        for line in BufReader::new(std::fs::File::open(logical_path).unwrap()).lines() {
            let row: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let physical = encode_v4_event(row, &vocabulary).unwrap();
            if physical["sourceEventSeqs"]
                .as_array()
                .is_some_and(|r| r.iter().any(Value::is_array))
            {
                compressed_refs += 1;
            }
            serde_json::to_writer(&mut writer, &physical).unwrap();
            writer.write_all(b"\n").unwrap();
        }
        writer.flush().unwrap();
        let RecordedWriter { file, hash } = writer;
        drop(file);
        let digest = format!("{:x}", hash.finalize());
        let mut physical = BufReader::new(std::fs::File::open(&physical_path).unwrap());
        let mut header_line = vec![];
        physical.read_until(b'\n', &mut header_line).unwrap();
        let mut scanner =
            V4LogScanner::new(&header_line, V4Recovery::Strict, vocabulary.clone()).unwrap();
        let mut validator = V4Validator::new(header, cut).unwrap();
        let mut expected = BufReader::new(std::fs::File::open(logical_path).unwrap()).lines();
        let mut bytes = [0u8; 4093];
        loop {
            let count = physical.read(&mut bytes).unwrap();
            if count == 0 {
                break;
            }
            scanner
                .write(&bytes[..count], |row| {
                    let source: Value = serde_json::from_str(
                        &expected
                            .next()
                            .ok_or("unexpected decoded row")?
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    if source != row {
                        return Err(format!("roundtrip changed event {}", row["seq"]));
                    }
                    validator.push(&row)
                })
                .unwrap();
        }
        assert!(expected.next().is_none());
        let scan = scanner.finish().unwrap();
        let validated = validator.finish().unwrap();
        assert_eq!(scan.input_bytes, scan.accepted_bytes);
        assert_eq!(scan.torn_tail_bytes, 0);
        assert!(scan.decoded.recovered_tail.is_none());
        assert_eq!(scan.decoded.inherited_event_count, cut);
        assert_eq!(
            validated.event_count,
            fixture["targetEvents"].as_u64().unwrap()
        );
        reports.push(json!({"id":fixture["id"],"path":physical_path,"bytes":scan.input_bytes,"events":validated.event_count,"compressedReferenceRows":compressed_refs,"sha256":digest,"losslessLogicalRoundtrip":true,"nativeValidation":true}));
    }
    std::fs::write(output,serde_json::to_vec_pretty(&json!({"fixtures":reports,"physicalRoundtripPassed":true,"sourceMutations":0,"published":false})).unwrap()).unwrap();
}
