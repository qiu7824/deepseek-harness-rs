//! Bounded protobuf and Connect framing for the Devin subscription transport.
use std::io::Read;

pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

pub(crate) struct Message<'a> {
    fields: Vec<(u32, Value<'a>)>,
}

fn varint(bytes: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut result = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset).ok_or("truncated protobuf integer")?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err("protobuf integer overflow".into());
        }
        result |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(result);
        }
    }
    Err("protobuf integer overflow".into())
}

impl<'a> Message<'a> {
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FRAME {
            return Err("protobuf message exceeds 16 MiB".into());
        }
        let mut offset = 0;
        let mut fields = Vec::new();
        while offset < bytes.len() {
            let key = varint(bytes, &mut offset)?;
            let field = key >> 3;
            if field == 0 || field >= 1 << 29 || fields.len() >= 100_000 {
                return Err("invalid protobuf field or excessive field count".into());
            }
            let value = match key & 7 {
                0 => Value::Varint(varint(bytes, &mut offset)?),
                1 | 2 | 5 => {
                    let length = match key & 7 {
                        1 => 8,
                        5 => 4,
                        _ => usize::try_from(varint(bytes, &mut offset)?)
                            .map_err(|_| "protobuf length overflow")?,
                    };
                    let end = offset
                        .checked_add(length)
                        .ok_or("protobuf length overflow")?;
                    let payload = bytes.get(offset..end).ok_or("truncated protobuf field")?;
                    offset = end;
                    if key & 7 == 2 {
                        Value::Bytes(payload)
                    } else {
                        Value::Fixed
                    }
                }
                _ => return Err("unsupported protobuf wire type".into()),
            };
            fields.push((field as u32, value));
        }
        Ok(Self { fields })
    }

    pub(crate) fn number(&self, field: u32) -> Result<u64, String> {
        match self.fields.iter().rev().find(|(id, _)| *id == field) {
            None => Ok(0),
            Some((_, Value::Varint(value))) => Ok(*value),
            _ => Err(format!("invalid numeric protobuf field {field}")),
        }
    }

    pub(crate) fn bytes(&self, field: u32) -> Result<Option<&'a [u8]>, String> {
        match self.fields.iter().rev().find(|(id, _)| *id == field) {
            None => Ok(None),
            Some((_, Value::Bytes(value))) => Ok(Some(*value)),
            _ => Err(format!("invalid protobuf message field {field}")),
        }
    }

    pub(crate) fn text(&self, field: u32) -> Result<&'a str, String> {
        std::str::from_utf8(self.bytes(field)?.unwrap_or_default())
            .map_err(|_| format!("invalid UTF-8 in protobuf field {field}"))
    }

    pub(crate) fn repeated(&self, field: u32) -> Result<Vec<&'a [u8]>, String> {
        self.fields
            .iter()
            .filter(|(id, _)| *id == field)
            .map(|(_, value)| {
                if let Value::Bytes(bytes) = value {
                    Ok(*bytes)
                } else {
                    Err(format!("invalid repeated protobuf field {field}"))
                }
            })
            .collect()
    }
}

fn put_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        output.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    output.push(value as u8);
}

#[derive(Default)]
pub(crate) struct Encoder(pub(crate) Vec<u8>);
impl Encoder {
    pub(crate) fn number(&mut self, field: u32, value: u64) {
        if value != 0 {
            put_varint(&mut self.0, u64::from(field) << 3);
            put_varint(&mut self.0, value);
        }
    }
    pub(crate) fn bytes(&mut self, field: u32, value: &[u8]) {
        if !value.is_empty() {
            put_varint(&mut self.0, (u64::from(field) << 3) | 2);
            put_varint(&mut self.0, value.len() as u64);
            self.0.extend_from_slice(value);
        }
    }
    pub(crate) fn text(&mut self, field: u32, value: &str) {
        self.bytes(field, value.as_bytes());
    }
    pub(crate) fn double(&mut self, field: u32, value: f64) {
        put_varint(&mut self.0, (u64::from(field) << 3) | 1);
        self.0.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn uncompress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take((MAX_FRAME + 1) as u64)
        .read_to_end(&mut output)
        .map_err(|_| "invalid gzip Connect frame")?;
    if output.len() > MAX_FRAME {
        return Err("expanded Connect frame exceeds 16 MiB".into());
    }
    Ok(output)
}

pub(crate) fn unary_payload(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_FRAME {
        return Err("Devin response exceeds 16 MiB".into());
    }
    if bytes.first().is_some_and(|flag| *flag <= 3) {
        if bytes.len() < 5 {
            return Err("truncated unary Connect envelope".into());
        }
        let length = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
        if length != bytes.len() - 5 || bytes[0] & 2 != 0 {
            return Err("invalid unary Connect envelope".into());
        }
        if bytes[0] & 1 != 0 {
            uncompress(&bytes[5..])
        } else {
            Ok(bytes[5..].to_vec())
        }
    } else {
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_fields_and_duplicate_scalars_preserve_known_values() {
        let mut encoded = Encoder::default();
        encoded.text(1, "SWE-2");
        encoded.number(2, 7);
        encoded.number(2, 9);
        encoded.double(400, 1.5);
        let parsed = Message::parse(&encoded.0).unwrap();
        assert_eq!(parsed.text(1).unwrap(), "SWE-2");
        assert_eq!(parsed.number(2).unwrap(), 9);
    }
    #[test]
    fn malformed_lengths_and_integer_overflows_are_rejected() {
        for value in [
            &[10, 255][..],
            &[0][..],
            &[8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 2][..],
            &[10, 5, 1][..],
        ] {
            assert!(Message::parse(value).is_err());
        }
        assert!(unary_payload(&[0, 0, 0, 0, 5, 10]).is_err());
    }
}
