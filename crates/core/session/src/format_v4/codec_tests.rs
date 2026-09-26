use super::*;
use serde_json::{Value, json};

fn header(seeded: bool) -> Value {
    let mut value =
        json!({"version":4,"id":"codec","createdAt":0,"isSeeded":seeded,"delegationDepth":0});
    if seeded {
        value["parentSession"] = json!("parent");
    }
    value
}
fn physical(seeded: bool) -> Value {
    encode_v4_header(header(seeded), 0).unwrap()
}
fn row(seq: u64) -> Value {
    json!({"type":"feedback/record","seq":seq,"time":seq,"data":{"retained":seq}})
}
fn references(seq: u64, refs: Value) -> Value {
    json!({"type":"plugin:opaque","seq":seq,"time":seq,"ignorable":true,"data":{"untouched":{"type":"tool-result"}},"sourceEventSeqs":refs})
}
fn decoder(prefix: u64) -> V4Decoder {
    let mut reader = V4Decoder::new(physical(false), V4Recovery::Strict).unwrap();
    for seq in 0..prefix {
        assert!(reader.decode_row(row(seq)).unwrap().is_some());
    }
    reader
}

#[test]
fn native_physical_headers_round_trip_and_do_not_accept_legacy_or_extra_fields() {
    assert_eq!(decode_v4_header(physical(false)).unwrap(), header(false));
    for (field, value) in [
        ("version", json!(3)),
        ("createdAt", json!(-1)),
        ("isSeeded", json!("false")),
        ("cwd", json!("relative")),
        ("seedLength", json!(0)),
        ("unknown", json!(true)),
    ] {
        let mut bad = physical(false);
        bad[field] = value;
        assert!(decode_v4_header(bad).is_err(), "{field}");
    }
    let mut bad = physical(false);
    bad.as_object_mut().unwrap().remove("delegationDepth");
    assert!(decode_v4_header(bad).is_err());
    assert!(encode_v4_header(header(false), 1).is_err());
}

#[test]
fn reference_ranges_preserve_plain_order_and_compress_only_monotonic_runs() {
    let vocabulary = V4Vocabulary::default();
    let logical = references(10, json!([0, 1, 2, 3, 5, 6, 9]));
    let encoded = encode_v4_event(logical.clone(), &vocabulary).unwrap();
    assert_eq!(encoded["sourceEventSeqs"], json!([[0, 3], 5, 6, 9]));
    assert_eq!(decoder(10).decode_row(encoded).unwrap(), Some(logical));
    let logical = references(10, json!([7, 1, 2, 3]));
    let encoded = encode_v4_event(logical.clone(), &vocabulary).unwrap();
    assert_eq!(encoded["sourceEventSeqs"], logical["sourceEventSeqs"]);
    assert_eq!(decoder(10).decode_row(encoded).unwrap(), Some(logical));
}

#[test]
fn physical_reference_scan_preserves_ranges_and_still_validates_them() {
    fn scan(refs: Value) -> Result<Vec<Value>, String> {
        let header = format!("{}\n", physical(false));
        let mut scanner = V4LogScanner::new(
            header.as_bytes(),
            V4Recovery::Strict,
            V4Vocabulary::default(),
        )?;
        let mut output = Vec::new();
        for seq in 0..10 {
            scanner.write_with_physical_references(
                format!("{}\n", row(seq)).as_bytes(),
                |value| {
                    output.push(value);
                    Ok(())
                },
            )?;
        }
        let line = format!("{}\n", references(10, refs));
        for chunk in line.as_bytes().chunks(7) {
            scanner.write_with_physical_references(chunk, |value| {
                output.push(value);
                Ok(())
            })?;
        }
        assert_eq!(scanner.finish()?.decoded.event_count, 11);
        Ok(output)
    }
    let rows = scan(json!([[0.0, 7.0], 9.0])).unwrap();
    assert_eq!(rows[10]["sourceEventSeqs"], json!([[0, 7], 9]));
    assert_eq!(
        scan(json!([7, 1, 2, 3])).unwrap()[10]["sourceEventSeqs"],
        json!([7, 1, 2, 3])
    );
    for refs in [
        json!([[0, 10]]),
        json!([[0, 1], [1, 2]]),
        json!([4, [0, 2]]),
        json!([0, 0]),
        json!([[-1, 2]]),
        json!([1.5]),
    ] {
        assert!(scan(refs).is_err());
    }
}

#[test]
fn range_bounds_duplicates_and_mixed_order_are_refused() {
    for refs in [
        json!([[3, 1]]),
        json!([[0, 10]]),
        json!([[0, 1], [1, 2]]),
        json!([0, 0]),
        json!([[0, 1, 2]]),
        json!([4, [0, 2]]),
        json!([-1]),
        json!([1.5]),
        json!(["1"]),
    ] {
        let mut reader = decoder(10);
        assert!(reader.decode_row(references(10, refs)).is_err());
        assert!(reader.decode_row(row(10)).is_err());
        assert!(reader.finish().is_err());
    }
    assert!(encode_v4_event(references(10, json!([[0, 3]])), &V4Vocabulary::default()).is_err());
}

#[test]
fn density_is_checked_before_range_expansion_and_discarded_suffix_is_never_expanded() {
    const MAX: u64 = 9_007_199_254_740_991;
    let huge = references(MAX, json!([[0, MAX - 1]]));
    let mut reader = decoder(0);
    assert!(reader.decode_row(huge.clone()).is_err());
    let mut reader = V4Decoder::new(physical(false), V4Recovery::RecoverableTail).unwrap();
    reader.decode_row(row(0)).unwrap();
    assert!(reader.decode_row(Value::Null).unwrap().is_none());
    assert!(reader.decode_row(huge).unwrap().is_none());
    let summary = reader.finish().unwrap();
    assert_eq!(summary.event_count, 1);
    assert!(summary.recovered_tail.is_some());
}

#[test]
fn ordinary_torn_tail_retains_prefix_but_a_later_closed_turn_refuses_recovery() {
    let mut reader = V4Decoder::new(physical(false), V4Recovery::RecoverableTail).unwrap();
    assert!(reader.decode_row(row(0)).unwrap().is_some());
    assert!(reader.decode_json_line(b"{incomplete").unwrap().is_none());
    assert!(reader.decode_row(row(2)).unwrap().is_none());
    let summary = reader.finish().unwrap();
    assert_eq!(summary.event_count, 1);
    assert_eq!(summary.recovered_tail.unwrap().row, 1);
    let mut reader = V4Decoder::new(physical(false), V4Recovery::RecoverableTail).unwrap();
    reader.decode_row(row(0)).unwrap();
    reader.decode_row(Value::Null).unwrap();
    assert!(
        reader
            .decode_row(json!({"type":"turn/end","seq":2,"time":2,"data":{"turn":1}}))
            .is_err()
    );
    assert!(reader.finish().is_err());
}

#[test]
fn hard_native_admission_is_not_hidden_after_an_ordinary_suffix_error() {
    let bad_rows = [
        json!({"type":"tool/result","seq":1,"time":1,"data":{"message":{"id":"old","role":"user","source":{"kind":"tool","callId":"call"},"content":[]}}}),
        json!({"type":"request/header","seq":1,"time":1,"data":{"header":{"system":"retired"}}}),
        json!({"type":"unknown/required","seq":1,"time":1,"data":null}),
        json!({"type":"developer/message","seq":1,"time":1,"ignorable":true,"data":null}),
    ];
    for bad in bad_rows {
        let mut reader = V4Decoder::new(physical(false), V4Recovery::RecoverableTail).unwrap();
        reader.decode_row(Value::Null).unwrap();
        assert!(reader.decode_row(bad).is_err());
        assert!(reader.finish().is_err());
    }
}

#[test]
fn only_accepted_inherited_markers_establish_the_cut() {
    let mut reader = V4Decoder::new(physical(true), V4Recovery::RecoverableTail).unwrap();
    reader.decode_row(row(0)).unwrap();
    reader
        .decode_row(json!({"type":"session/end-seed","seq":1,"time":1,"data":{"inherited":true}}))
        .unwrap();
    reader.decode_row(Value::Null).unwrap();
    reader
        .decode_row(json!({"type":"session/end-seed","seq":3,"time":3,"data":{"inherited":true}}))
        .unwrap();
    assert_eq!(reader.finish().unwrap().inherited_event_count, 1);
    let mut reader = V4Decoder::new(physical(true), V4Recovery::RecoverableTail).unwrap();
    reader.decode_row(Value::Null).unwrap();
    reader
        .decode_row(json!({"type":"session/end-seed","seq":1,"time":1,"data":{"inherited":true}}))
        .unwrap();
    assert!(reader.finish().is_err());
    let mut reader = decoder(0);
    reader
        .decode_row(json!({"type":"session/end-seed","seq":0,"time":1,"data":{"inherited":true}}))
        .unwrap();
    assert!(reader.finish().is_err());
}

#[test]
fn ignorable_developer_admission_depends_on_reader_ownership_but_writer_still_validates() {
    let vocabulary = V4Vocabulary::explicit(["feedback/record"]);
    let unknown = json!({"type":"developer/message","seq":0,"time":0,"data":null,"ignorable":true});
    let mut reader =
        V4Decoder::with_vocabulary(physical(false), V4Recovery::Strict, vocabulary.clone())
            .unwrap();
    assert_eq!(
        reader.decode_row(unknown.clone()).unwrap(),
        Some(unknown.clone())
    );
    reader.finish().unwrap();
    assert!(encode_v4_event(unknown.clone(), &vocabulary).is_err());
    assert!(decoder(0).decode_row(unknown).is_err());
}

#[test]
fn explicit_semantic_vocabulary_cannot_reinterpret_opaque_payloads_or_boundaries() {
    let vocabulary = V4Vocabulary::explicit(["turn/start", "turn/end"]);
    let mut validator = V4Validator::with_vocabulary(header(false), 0, vocabulary).unwrap();
    for (seq, kind) in [
        "tool/result",
        "system/message",
        "request/header",
        "session/end-seed",
    ]
    .into_iter()
    .enumerate()
    {
        validator
            .push(&json!({"type":kind,"seq":seq,"time":seq,"data":null,"ignorable":true}))
            .unwrap();
    }
    validator
        .push(&json!({"type":"turn/start","seq":4,"time":4,"data":{"turn":1}}))
        .unwrap();
    validator
        .push(&json!({"type":"turn/end","seq":5,"time":5,"data":{"turn":1}}))
        .unwrap();
    validator.finish().unwrap();
    let vocabulary = V4Vocabulary::explicit(["compaction/start", "turn/start"]);
    let mut validator = V4Validator::with_vocabulary(header(false), 0, vocabulary).unwrap();
    validator.push(&json!({"type":"compaction/start","seq":0,"time":0,"data":{"compactionId":"c","turn":null}})).unwrap();
    validator
        .push(&json!({"type":"session/end-seed","seq":1,"time":1,"data":{},"ignorable":true}))
        .unwrap();
    validator
        .push(&json!({"type":"turn/start","seq":2,"time":2,"data":{"turn":1}}))
        .unwrap();
    assert!(validator.finish().is_err());
}

#[test]
fn byte_scanner_handles_utf8_splits_and_records_exact_accepted_prefix() {
    let mut header_bytes = serde_json::to_vec(&physical(false)).unwrap();
    header_bytes.push(b'\n');
    let mut event = row(0);
    event["data"]["text"] = json!("中文跨块");
    let mut bytes = serde_json::to_vec(&event).unwrap();
    bytes.push(b'\n');
    let committed = header_bytes.len() + bytes.len();
    bytes.extend_from_slice(b"{torn");
    let mut scanner = V4LogScanner::new(
        &header_bytes,
        V4Recovery::RecoverableTail,
        V4Vocabulary::default(),
    )
    .unwrap();
    let mut output = vec![];
    for chunk in bytes.chunks(1) {
        scanner
            .write(chunk, |event| {
                output.push(event);
                Ok(())
            })
            .unwrap();
    }
    let scan = scanner.finish().unwrap();
    assert_eq!(output, vec![event]);
    assert_eq!(scan.accepted_bytes, committed as u64);
    assert_eq!(scan.torn_tail_bytes, 5);
    let mut strict =
        V4LogScanner::new(&header_bytes, V4Recovery::Strict, V4Vocabulary::default()).unwrap();
    strict.write(&bytes, |_| Ok(())).unwrap();
    assert!(strict.finish().is_err());
}

#[test]
fn scanner_does_not_recover_a_sink_failure_or_split_header() {
    assert!(V4LogScanner::new(b"", V4Recovery::RecoverableTail, V4Vocabulary::default()).is_err());
    let mut header_bytes = serde_json::to_vec(&physical(false)).unwrap();
    assert!(V4LogScanner::new(&header_bytes, V4Recovery::Strict, V4Vocabulary::default()).is_err());
    header_bytes.push(b'\n');
    let mut bytes = serde_json::to_vec(&row(0)).unwrap();
    bytes.push(b'\n');
    let mut scanner = V4LogScanner::new(
        &header_bytes,
        V4Recovery::RecoverableTail,
        V4Vocabulary::default(),
    )
    .unwrap();
    assert!(
        scanner
            .write(&bytes, |_| Err("fixture disk full".into()))
            .is_err()
    );
    assert!(scanner.finish().is_err());
}
