use super::*;
use crate::compress_zstd_frame;
use serde_json::json;
use std::io::Write;

fn header() -> Vec<u8> {
    format!("{}\n", json!({"type":"session","version":4,"id":"fixture","createdAt":0,"isSeeded":false,"delegationDepth":0})).into_bytes()
}
fn body() -> Vec<u8> {
    format!(
        "{}\n{}\n",
        json!({"type":"session/end-seed","seq":0,"time":0,"data":{}}),
        json!({"type":"feedback/record","seq":1,"time":1,"data":{"text":"中文保存"}})
    )
    .into_bytes()
}
fn path(bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("v4-artifact-{}.data", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(bytes).unwrap();
    path
}
fn packed(parts: &[&[u8]]) -> Vec<u8> {
    parts
        .iter()
        .flat_map(|part| compress_zstd_frame(part).unwrap())
        .collect()
}

#[test]
fn plain_and_concatenated_frames_validate_the_same_lifecycle() {
    let header = header();
    let body = body();
    for (compression, bytes, frames) in [
        (
            JsonlCompression::None,
            [header.as_slice(), body.as_slice()].concat(),
            0,
        ),
        (JsonlCompression::Zstd, packed(&[&header, &body]), 2),
    ] {
        let file = path(&bytes);
        let result = validate_v4_artifact(&file, compression, "fixture", &|| false).unwrap();
        assert_eq!(result.lifecycle.event_count, 2);
        assert_eq!(result.framing.physical_bytes, bytes.len() as u64);
        assert_eq!(
            result.framing.plaintext_bytes,
            (header.len() + body.len()) as u64
        );
        assert_eq!(result.framing.frames, frames);
        assert_eq!(result.scan.accepted_bytes, result.scan.input_bytes);
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }
}

#[test]
fn torn_frames_checksum_corruption_and_split_records_are_not_published_as_valid() {
    let header = header();
    let body = body();
    let good = packed(&[&header, &body]);
    let mut corrupt = good.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    let split = packed(&[&header, &body[..12], &body[12..]]);
    for bytes in [
        good[..good.len() - 1].to_vec(),
        corrupt,
        split,
        packed(&[&[header.as_slice(), body.as_slice()].concat()]),
        [good.as_slice(), b"junk"].concat(),
    ] {
        let file = path(&bytes);
        assert!(validate_v4_artifact(&file, JsonlCompression::Zstd, "fixture", &|| false).is_err());
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }
    let bytes = [header.as_slice(), &body[..body.len() - 1]].concat();
    assert!(
        validate_v4_artifact(&path(&bytes), JsonlCompression::None, "fixture", &|| false).is_err()
    );
}

#[test]
fn semantic_failures_identity_and_cancellation_reject_complete_physical_files() {
    let header = header();
    let body = body();
    let bytes = [header.as_slice(), body.as_slice()].concat();
    let file = path(&bytes);
    assert!(validate_v4_artifact(&file, JsonlCompression::None, "other", &|| false).is_err());
    assert!(
        validate_v4_artifact(&file, JsonlCompression::None, "fixture", &|| true)
            .unwrap_err()
            .contains("cancelled")
    );
    let bad = format!(
        "{}\n",
        json!({"type":"step/end","seq":2,"time":2,"data":{"turn":1,"step":1}})
    );
    let bad_file = path(&[bytes, bad.into_bytes()].concat());
    assert!(validate_v4_artifact(&bad_file, JsonlCompression::None, "fixture", &|| false).is_err());
}

#[test]
fn body_callbacks_are_bounded_and_sink_failures_stop_the_stream() {
    let header = header();
    let body = vec![b'x'; 4 * 1024 * 1024];
    let mut bytes = body.clone();
    bytes.push(b'\n');
    let file = path(&packed(&[&header, &bytes]));
    let mut handle = File::open(&file).unwrap();
    let mut chunks = 0;
    visit_artifact(&mut handle, JsonlCompression::Zstd, &|| false, |part| {
        if let ArtifactPart::Body(bytes) = part {
            assert!(bytes.len() <= BUFFER);
            chunks += 1;
        }
        Ok(())
    })
    .unwrap();
    assert!(chunks > 100);
    let mut failed_calls = 0;
    let error = visit_artifact(&mut handle, JsonlCompression::Zstd, &|| false, |part| {
        if let ArtifactPart::Body(_) = part {
            failed_calls += 1;
            return Err("sink failed".into());
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error, "sink failed");
    assert_eq!(failed_calls, 1);
}
