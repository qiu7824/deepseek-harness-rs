//! Native V4 framing used by the normal persistence writer.
use crate::format::JsonlCompression;
use dsh_session::format_v4::{V4Vocabulary, decode_v4_header, encode_v4_event};
use dsh_session::{SessionEvent, SessionHeader, SessionLogOffset};
use std::io::Write;

fn rows(writer: &mut impl Write, events: &[SessionEvent]) -> Result<(), String> {
    let vocabulary = V4Vocabulary::default();
    for event in events {
        let row = encode_v4_event(
            serde_json::to_value(event).map_err(|e| e.to_string())?,
            &vocabulary,
        )?;
        serde_json::to_writer(&mut *writer, &row).map_err(|e| e.to_string())?;
        writer.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn event_batch(
    events: &[SessionEvent],
    compression: JsonlCompression,
) -> Result<Vec<u8>, String> {
    if events.is_empty() {
        return Ok(vec![]);
    }
    match compression {
        JsonlCompression::None => {
            let mut output = vec![];
            rows(&mut output, events)?;
            Ok(output)
        }
        JsonlCompression::Zstd => {
            let mut encoder =
                zstd::stream::Encoder::new(Vec::new(), 0).map_err(|e| e.to_string())?;
            encoder.include_checksum(true).map_err(|e| e.to_string())?;
            rows(&mut encoder, events)?;
            encoder.finish().map_err(|e| e.to_string())
        }
    }
}

pub(crate) fn materialization(
    meta: &SessionHeader,
    inherited: SessionLogOffset,
    events: &[SessionEvent],
    compression: JsonlCompression,
) -> Result<Vec<u8>, String> {
    let physical = serde_json::to_value(crate::format::to_header_line(meta, Some(inherited))?)
        .map_err(|e| e.to_string())?;
    decode_v4_header(physical.clone())?;
    if meta.is_seeded
        && !events.get(inherited.get() as usize).is_some_and(|event| {
            event.type_ == "session/end-seed" && event.data["inherited"] == true
        })
    {
        return Err("V4 materialization requires its exact inherited marker".into());
    }
    let mut header = serde_json::to_vec(&physical).map_err(|e| e.to_string())?;
    header.push(b'\n');
    let mut output = match compression {
        JsonlCompression::None => header,
        JsonlCompression::Zstd => crate::zstd::compress_zstd_frame(&header)?,
    };
    output.extend(event_batch(events, compression)?);
    Ok(output)
}
