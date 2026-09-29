//! Bounded raw character-data scans through quick-xml's documented stream seam.
use super::*;

pub(super) struct Buffered<R> {
    pub input: R,
    bytes: [u8; 8192],
    start: usize,
    end: usize,
    eof: bool,
}
impl<R: Read> Buffered<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            bytes: [0; 8192],
            start: 0,
            end: 0,
            eof: false,
        }
    }
}
impl<R: Read> BufRead for Buffered<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        // Small lookahead identifies markup without consuming bytes that the
        // XML reader still owns, including prefixes crossing read boundaries.
        if self.end - self.start < 16 && !self.eof {
            self.bytes.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
            while self.end < 16 && !self.eof {
                let n = self.input.read(&mut self.bytes[self.end..])?;
                self.end += n;
                self.eof = n == 0;
            }
        }
        Ok(&self.bytes[self.start..self.end])
    }
    fn consume(&mut self, count: usize) {
        self.start = self.end.min(self.start + count);
    }
}
impl<R: Read> Read for Buffered<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let bytes = self.fill_buf()?;
        let count = bytes.len().min(output.len());
        output[..count].copy_from_slice(&bytes[..count]);
        self.consume(count);
        Ok(count)
    }
}

#[derive(Default)]
struct Characters {
    incomplete: Vec<u8>,
    last: [u8; 2],
}
impl Characters {
    fn push(&mut self, bytes: &[u8], forbidden: Option<&[u8]>, inside: bool) -> Result<()> {
        if !inside
            && !bytes
                .iter()
                .all(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        {
            return Err("Invalid Office XML text outside root".into());
        }
        if let Some(forbidden) = forbidden {
            let mut prefix = Vec::from(self.last);
            prefix.extend_from_slice(&bytes[..bytes.len().min(2)]);
            if prefix
                .windows(forbidden.len())
                .chain(bytes.windows(forbidden.len()))
                .any(|part| part == forbidden)
            {
                return Err("Invalid Office XML character-data delimiter".into());
            }
        }
        for &b in bytes {
            self.last = [self.last[1], b];
        }
        self.incomplete.extend_from_slice(bytes);
        match std::str::from_utf8(&self.incomplete) {
            Ok(value) => {
                if !value.chars().all(xml_char) {
                    return Err("Invalid Office XML character".into());
                }
                self.incomplete.clear();
            }
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                if !std::str::from_utf8(&self.incomplete[..valid])
                    .unwrap()
                    .chars()
                    .all(xml_char)
                {
                    return Err("Invalid Office XML character".into());
                }
                self.incomplete.drain(..valid);
            }
            Err(_) => return Err("Invalid Office XML UTF-8".into()),
        }
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        if self.incomplete.is_empty() {
            Ok(())
        } else {
            Err("Invalid Office XML UTF-8".into())
        }
    }
}

fn delimited<R: BufRead>(input: &mut R, end: &[u8], comment: bool) -> Result<()> {
    let mut pending = Vec::new();
    let mut characters = Characters::default();
    loop {
        let bytes = input.fill_buf().map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return Err("Invalid Office XML: unterminated character data".into());
        }
        let mut combined = Vec::with_capacity(pending.len() + bytes.len());
        combined.extend_from_slice(&pending);
        combined.extend_from_slice(bytes);
        if let Some(position) = combined.windows(end.len()).position(|part| part == end) {
            characters.push(
                &combined[..position],
                comment.then_some(b"--".as_slice()),
                true,
            )?;
            if comment && characters.last[1] == b'-' {
                return Err("Invalid Office XML comment".into());
            }
            let consumed = position + end.len() - pending.len();
            input.consume(consumed);
            return characters.finish();
        }
        let boundary = combined.len().saturating_sub(end.len() - 1);
        characters.push(
            &combined[..boundary],
            comment.then_some(b"--".as_slice()),
            true,
        )?;
        pending.clear();
        pending.extend_from_slice(&combined[boundary..]);
        let consumed = bytes.len();
        input.consume(consumed);
    }
}

pub(super) fn content<R: BufRead>(reader: &mut Reader<R>, inside: bool) -> Result<bool> {
    let mut input = reader.stream();
    let mut consumed = false;
    let mut characters = Characters::default();
    loop {
        let bytes = input.fill_buf().map_err(|e| e.to_string())?;
        if bytes.is_empty() || bytes[0] == b'&' {
            characters.finish()?;
            return Ok(consumed);
        }
        if bytes.starts_with(b"<![CDATA[") {
            characters.finish()?;
            if !inside {
                return Err("Invalid Office XML: CDATA outside root".into());
            }
            input.consume(9);
            delimited(&mut input, b"]]>", false)?;
            characters = Characters::default();
            consumed = true;
            continue;
        }
        if bytes.starts_with(b"<!--") {
            characters.finish()?;
            input.consume(4);
            delimited(&mut input, b"-->", true)?;
            characters = Characters::default();
            consumed = true;
            continue;
        }
        if bytes.starts_with(b"<?")
            && !(bytes.starts_with(b"<?xml")
                && bytes
                    .get(5)
                    .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'?')))
        {
            characters.finish()?;
            input.consume(2);
            let mut target = Vec::new();
            loop {
                let bytes = input.fill_buf().map_err(|e| e.to_string())?;
                if bytes.is_empty() {
                    return Err("Invalid Office XML processing instruction".into());
                }
                let count = bytes
                    .iter()
                    .position(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t' | b'?'))
                    .unwrap_or(bytes.len());
                target.extend_from_slice(&bytes[..count]);
                if target.len() > 4096 {
                    return Err("Office XML name budget exceeded".into());
                }
                let ended = count < bytes.len();
                input.consume(count);
                if ended {
                    break;
                }
            }
            let name = text(&target)?;
            let mut chars = name.chars();
            if name.eq_ignore_ascii_case("xml")
                || !chars.next().is_some_and(|c| name_start(c) || c == ':')
                || !chars.all(|c| name_char(c) || c == ':')
            {
                return Err("Invalid Office XML processing instruction".into());
            }
            if input
                .fill_buf()
                .map_err(|e| e.to_string())?
                .starts_with(b"?")
                && !input
                    .fill_buf()
                    .map_err(|e| e.to_string())?
                    .starts_with(b"?>")
            {
                return Err("Invalid Office XML processing-instruction separator".into());
            }
            delimited(&mut input, b"?>", false)?;
            characters = Characters::default();
            consumed = true;
            continue;
        }
        if bytes[0] == b'<' {
            characters.finish()?;
            return Ok(consumed);
        }
        let count = bytes
            .iter()
            .position(|b| matches!(b, b'<' | b'&'))
            .unwrap_or(bytes.len());
        characters.push(&bytes[..count], Some(b"]]>"), inside)?;
        input.consume(count);
        consumed = true;
    }
}
