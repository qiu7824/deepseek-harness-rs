//! Streaming namespace-aware OOXML validation; retains no document tree.
use crate::Result;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
    name::{Namespace, NamespaceResolver, ResolveResult},
};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    io::{self, BufRead, Read},
};

const MAX_PART: usize = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 256;
const MAX_ATTRIBUTES: usize = 4096;
const MAX_NAMESPACE_BYTES: usize = 1024 * 1024;
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

#[path = "office_xml_stream.rs"]
mod streaming;
#[cfg(test)]
#[path = "office_xml_tests.rs"]
mod tests;

struct Bounded<R> {
    input: R,
    remaining: usize,
    limit: usize,
    token_remaining: Option<usize>,
}
impl<R: Read> Read for Bounded<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return if self.input.read(&mut [0])? == 0 {
                Ok(0)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Office XML part exceeds {} MiB", self.limit / (1024 * 1024)),
                ))
            };
        }
        if self.token_remaining == Some(0) {
            return Err(io::Error::other(
                "Office XML markup metadata budget exceeded",
            ));
        }
        let limit = buffer
            .len()
            .min(self.remaining)
            .min(self.token_remaining.unwrap_or(usize::MAX));
        let read = self.input.read(&mut buffer[..limit])?;
        self.remaining -= read;
        if let Some(remaining) = &mut self.token_remaining {
            *remaining -= read;
        }
        Ok(read)
    }
}

fn xml_char(c: char) -> bool {
    matches!(c as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}
fn text(bytes: &[u8]) -> Result<&str> {
    let value = std::str::from_utf8(bytes).map_err(|e| format!("Invalid Office XML: {e}"))?;
    if !value.chars().all(xml_char) {
        return Err("Invalid Office XML character".into());
    }
    Ok(value)
}
fn name_start(c: char) -> bool {
    matches!(c, 'A'..='Z' | '_' | 'a'..='z')
        || matches!(c as u32,
        0xc0..=0xd6 | 0xd8..=0xf6 | 0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff |
        0x200c..=0x200d | 0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff |
        0xf900..=0xfdcf | 0xfdf0..=0xfffd | 0x10000..=0xeffff)
}
fn name_char(c: char) -> bool {
    name_start(c)
        || matches!(c, '-' | '.' | '0'..='9')
        || matches!(c as u32, 0xb7 | 0x300..=0x36f | 0x203f..=0x2040)
}
fn qualified_name(bytes: &[u8]) -> Result<&str> {
    if bytes.len() > 4096 {
        return Err("Office XML name budget exceeded".into());
    }
    let name = text(bytes)?;
    let mut parts = name.split(':');
    for part in parts.by_ref().take(2) {
        let mut chars = part.chars();
        if !chars.next().is_some_and(name_start) || !chars.all(name_char) {
            return Err("Invalid Office XML qualified name".into());
        }
    }
    if parts.next().is_some() {
        return Err("Invalid Office XML qualified name".into());
    }
    Ok(name)
}
fn attribute_value(bytes: &[u8]) -> Result<Cow<'_, str>> {
    let raw = text(bytes)?;
    if raw.contains('<') {
        return Err("Invalid Office XML attribute value".into());
    }
    // XML attribute normalization precedes reference expansion: &#10; stays
    // a newline, whereas literal line endings and tabs become spaces.
    let normalized = if raw.bytes().any(|b| matches!(b, b'\r' | b'\n' | b'\t')) {
        Cow::Owned(raw.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "))
    } else {
        Cow::Borrowed(raw)
    };
    let decoded = match normalized {
        Cow::Borrowed(raw) => {
            quick_xml::escape::unescape_with(raw, quick_xml::escape::resolve_xml_entity)
                .map_err(|e| format!("Invalid Office XML: {e}"))?
        }
        Cow::Owned(raw) => Cow::Owned(
            quick_xml::escape::unescape_with(&raw, quick_xml::escape::resolve_xml_entity)
                .map_err(|e| format!("Invalid Office XML: {e}"))?
                .into_owned(),
        ),
    };
    if !decoded.chars().all(xml_char) {
        return Err("Invalid Office XML character reference".into());
    }
    Ok(decoded)
}

fn attribute_separators(raw: &[u8]) -> Result<()> {
    // quick-xml validates quotes and '=', but intentionally accepts adjacent
    // quoted attributes. XML requires S between attributes; enforce it here.
    let mut quote = None;
    let mut closed = false;
    for &byte in raw {
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
                closed = true;
            }
        } else if closed {
            if !matches!(byte, b' ' | b'\r' | b'\n' | b'\t') {
                return Err("Invalid Office XML: attributes require whitespace separators".into());
            }
            closed = false;
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        }
    }
    Ok(())
}

pub(super) fn validate<R: Read>(input: R, relationships: bool) -> Result<String> {
    validate_with_limit(input, relationships, MAX_PART)
}

pub(super) fn validate_with_limit<R: Read>(
    input: R,
    relationships: bool,
    limit: usize,
) -> Result<String> {
    let mut input = streaming::Buffered::new(Bounded {
        input,
        remaining: limit,
        limit,
        token_remaining: None,
    });
    if input
        .fill_buf()
        .map_err(|e| e.to_string())?
        .starts_with(b"\xef\xbb\xbf")
    {
        input.consume(3);
    }
    let mut reader = Reader::from_reader(input);
    reader.config_mut().enable_all_checks(true);
    let mut namespaces = NamespaceResolver::default();
    let mut namespace_sizes = Vec::new();
    let mut namespace_bytes = 0usize;
    let mut root = None;
    let mut declaration_allowed = true;
    let mut buffer = Vec::with_capacity(8192);
    let mut can_stream = true;
    loop {
        reader.get_mut().input.token_remaining = None;
        if can_stream && streaming::content(&mut reader, !namespace_sizes.is_empty())? {
            declaration_allowed = false;
        }
        // Only markup reaches the ordinary event buffer; text, CDATA, comments
        // and PI payloads are validated incrementally without materialization.
        reader.get_mut().input.token_remaining = Some(MAX_PART - 8192);
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|e| format!("Invalid Office XML: {e}"))?;
        text(event.as_ref())?;
        can_stream = !matches!(event, Event::Text(_));
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                declaration_allowed = false;
                if namespace_sizes.len() >= MAX_DEPTH {
                    return Err("Office XML nesting exceeds 256 levels".into());
                }
                let name = qualified_name(element.name().as_ref())?.to_string();
                attribute_separators(element.attributes_raw())?;
                if namespace_sizes.is_empty() {
                    if root.is_some() {
                        return Err("Invalid Office XML: multiple root elements".into());
                    }
                    root = Some(name.rsplit(':').next().unwrap().to_string());
                }
                // Let the library manage scope/pop and reserved-prefix rules,
                // but bind decoded values instead of NsReader's raw xmlns bytes.
                namespaces
                    .push(&BytesStart::new("scope"))
                    .map_err(|e| format!("Invalid Office XML namespace: {e}"))?;
                let mut added = 0usize;
                for (index, attribute) in element.attributes().enumerate() {
                    if index >= MAX_ATTRIBUTES {
                        return Err("Office XML attribute budget exceeded".into());
                    }
                    let attribute =
                        attribute.map_err(|e| format!("Invalid Office XML attribute: {e}"))?;
                    qualified_name(attribute.key.as_ref())?;
                    let value = attribute_value(&attribute.value)?;
                    if let Some(prefix) = attribute.key.as_namespace_binding() {
                        if value == XMLNS_NAMESPACE
                            || (value == XML_NAMESPACE && attribute.key.as_ref() != b"xmlns:xml")
                            || (value.is_empty() && attribute.key.as_ref() != b"xmlns")
                        {
                            return Err("Invalid Office XML namespace binding".into());
                        }
                        added = added.saturating_add(attribute.key.as_ref().len() + value.len());
                        if namespace_bytes.saturating_add(added) > MAX_NAMESPACE_BYTES {
                            return Err("Office XML namespace budget exceeded".into());
                        }
                        namespaces
                            .add(prefix, Namespace(value.as_bytes()))
                            .map_err(|e| format!("Invalid Office XML namespace: {e}"))?;
                    } else if relationships
                        && attribute.key.as_ref() == b"TargetMode"
                        && value.eq_ignore_ascii_case("external")
                    {
                        return Err("External Office relationships require separate review before automation".into());
                    }
                }
                if matches!(
                    namespaces.resolve_element(element.name()).0,
                    ResolveResult::Unknown(_)
                ) || name.starts_with("xmlns:")
                {
                    return Err("Invalid Office XML: unbound element namespace".into());
                }
                let mut expanded = BTreeSet::new();
                for attribute in element.attributes() {
                    let attribute =
                        attribute.map_err(|e| format!("Invalid Office XML attribute: {e}"))?;
                    if attribute.key.as_namespace_binding().is_some() {
                        continue;
                    }
                    let (namespace, local) = namespaces.resolve_attribute(attribute.key);
                    let namespace = match namespace {
                        ResolveResult::Bound(namespace) => namespace.0,
                        ResolveResult::Unbound => b"",
                        ResolveResult::Unknown(_) => {
                            return Err("Invalid Office XML: unbound attribute namespace".into());
                        }
                    };
                    if !expanded.insert((namespace, local.into_inner())) {
                        return Err("Invalid Office XML: duplicate expanded attribute name".into());
                    }
                }
                if matches!(event, Event::Empty(_)) {
                    namespaces.pop();
                } else {
                    namespace_sizes.push(added);
                    namespace_bytes += added;
                }
            }
            Event::End(_) => {
                declaration_allowed = false;
                namespace_bytes -= namespace_sizes
                    .pop()
                    .ok_or("Invalid Office XML end element")?;
                namespaces.pop();
            }
            Event::Text(value) => {
                declaration_allowed = false;
                let value = text(&value)?;
                if value.contains("]]>")
                    || (namespace_sizes.is_empty()
                        && !value
                            .bytes()
                            .all(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t')))
                {
                    return Err(
                        "Invalid Office XML text outside an element or CDATA terminator".into(),
                    );
                }
            }
            Event::CData(_) if namespace_sizes.is_empty() => {
                return Err("Invalid Office XML: CDATA outside root".into());
            }
            Event::GeneralRef(value) => {
                if namespace_sizes.is_empty() {
                    return Err("Invalid Office XML reference outside root".into());
                }
                match value
                    .resolve_char_ref()
                    .map_err(|e| format!("Invalid Office XML reference: {e}"))?
                {
                    Some(c) if xml_char(c) => {}
                    None if quick_xml::escape::resolve_xml_entity(text(&value)?).is_some() => {}
                    _ => return Err("Invalid Office XML entity reference".into()),
                }
            }
            Event::DocType(_) => {
                return Err("Office XML DTDs and custom entities are not permitted".into());
            }
            Event::Decl(value) => {
                if !declaration_allowed || value.len() > 4096 {
                    return Err("Invalid Office XML declaration position or size".into());
                }
                // The declaration is tiny metadata, not the document body.
                roxmltree::Document::parse(&format!("<?{}?><root/>", text(&value)?))
                    .map_err(|e| format!("Invalid Office XML declaration: {e}"))?;
                declaration_allowed = false;
            }
            Event::PI(value) => {
                declaration_allowed = false;
                let target = text(value.target())?;
                let mut chars = target.chars();
                if target.eq_ignore_ascii_case("xml")
                    || !chars.next().is_some_and(|c| name_start(c) || c == ':')
                    || !chars.all(|c| name_char(c) || c == ':')
                {
                    return Err("Invalid Office XML processing instruction".into());
                }
            }
            Event::Comment(value) => {
                declaration_allowed = false;
                if value.ends_with(b"-") {
                    return Err("Invalid Office XML comment".into());
                }
            }
            Event::Eof => {
                if !namespace_sizes.is_empty() {
                    return Err("Invalid Office XML: unclosed element".into());
                }
                return root.ok_or("Invalid Office XML: root element missing".into());
            }
            _ => {}
        }
        buffer.clear();
    }
}
