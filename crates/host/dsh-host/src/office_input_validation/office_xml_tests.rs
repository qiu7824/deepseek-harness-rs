use super::*;
use std::io::Cursor;

#[test]
fn markup_mutations_never_relax_the_previous_parser_checks() {
    for original in [
        "<document x='1' y=\"2\"><p>text &amp; more</p></document>",
        "<w:document xmlns:w='urn:w' xmlns:p='urn:p' p:x='v'><w:p/></w:document>",
        "<?xml version='1.0'?><!--ok--><document><![CDATA[text]]><?note ok?></document>",
    ] {
        let bytes = original.as_bytes();
        for index in 0..bytes.len() {
            let mut deleted = bytes.to_vec();
            deleted.remove(index);
            let old = roxmltree::Document::parse(std::str::from_utf8(&deleted).unwrap()).is_ok();
            // roxmltree accepts some namespace-invalid names such as ':x'.
            // Keep stricter QName checks; every newly accepted mutation must
            // nevertheless satisfy the previous parser's syntax protection.
            assert!(
                !validate(deleted.as_slice(), false).is_ok() || old,
                "delete at {index}: {}",
                String::from_utf8_lossy(&deleted)
            );
            for byte in b"<>/=\"'&;?: \t" {
                let mut changed = bytes.to_vec();
                changed[index] = *byte;
                let old =
                    roxmltree::Document::parse(std::str::from_utf8(&changed).unwrap()).is_ok();
                assert!(
                    !validate(changed.as_slice(), false).is_ok() || old,
                    "replace at {index}: {}",
                    String::from_utf8_lossy(&changed)
                );
            }
        }
    }
}

#[test]
fn streaming_validation_preserves_namespace_and_xml_wellformedness_checks() {
    let valid = [
        "<document/>",
        "<?xml version='1.0' encoding='UTF-8'?><document/>",
        "\u{feff}<document/>",
        "<!-- before --><w:document xmlns:w='urn:word'><w:p xml:space='preserve'>文本 &amp; &lt; &#x1F600;</w:p></w:document><!--after-->",
        "<document xmlns='urn:root' xmlns:a='urn:same' xmlns:b='urn:other' a:x='1' b:x='2'/>",
        "<document xmlns:a='urn:outer'><a:p xmlns:a='urn:inner'/><a:p/></document>",
        "<document xmlns='urn:outer'><p xmlns=''/></document>",
        "<document xmlns:a='urn:s&#97;me' xmlns:b='urn:same' a:x='1' b:y='2'/>",
        "<document xmlns:xml='http://www.w3.org/XML/1998/namespac&#101;' xml:space='preserve'/>",
        "<document value='&#10; &#13; &#9; &quot; &apos; &amp;'>a<![CDATA[<&raw;>]]><?note a:b?><!-- note --></document>",
    ];
    for xml in valid {
        assert!(
            roxmltree::Document::parse(xml).is_ok(),
            "invalid reference fixture: {xml}"
        );
        assert_eq!(
            validate(xml.as_bytes(), false).unwrap(),
            "document",
            "{xml}"
        );
    }
    let invalid = [
        "",
        " ",
        "<document>",
        "<document/></extra>",
        "<document/><other/>",
        "prefix<document/>",
        "<document/>suffix",
        "<document><![CDATA[unterminated</document>",
        "<![CDATA[text]]><document/>",
        "<document>bad]]></document>",
        "<document>&unknown;</document>",
        "<document>&#0;</document>",
        "<document>&nbsp;</document>",
        "<document x='&copy;'/>",
        "<document>&#xD800;</document>",
        "<document>&#x110000;</document>",
        "<document>& missing;</document>",
        "<document><a></b></document>",
        "<w:document/>",
        "<document w:val='1'/>",
        "<document xmlns:w=''><w:p/></document>",
        "<document xmlns:xml='urn:not-xml'/>",
        "<document xmlns:xmlns='urn:invalid'/>",
        "<document xmlns='http://www.w3.org/2000/xmlns/'/>",
        "<document xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
        "<document xmlns:a='urn:same' xmlns:b='urn:same' a:x='1' b:x='2'/>",
        "<document xmlns:a='urn:s&#97;me' xmlns:b='urn:same' a:x='1' b:x='2'/>",
        "<document xmlns:a='urn:x' xmlns:a='urn:y'/>",
        "<document x='1' x='2'/>",
        "<document bad=unquoted/>",
        "<document bad='<value'/>",
        "<document x='&missing;'/>",
        "<document x='&#0;'/>",
        "<document x='1'y='2'/>",
        "<1document/>",
        "<a::document/>",
        "<:document/>",
        "<document:/>",
        "<document :x='1'/>",
        "<document>\u{1}</document>",
        "<!--broken--comment--><document/>",
        "<!--broken---><document/>",
        " <?xml version='1.0'?><document/>",
        "<document><?xml version='1.0'?></document>",
        "<?xml encoding='UTF-8'?><document/>",
        "<?XML version='1.0'?><document/>",
    ];
    for xml in invalid {
        assert!(
            validate(xml.as_bytes(), false).is_err(),
            "accepted malformed XML: {xml}"
        );
    }
    assert!(validate(Cursor::new(b"<document>\xff</document>"), false).is_err());
}

#[test]
fn dtd_and_external_relationships_cannot_hide_behind_character_references() {
    for xml in [
        "<!DOCTYPE document><document/>",
        "<!DOCTYPE document SYSTEM 'https://example.invalid/entity'><document/>",
        "<!DOCTYPE document [<!ENTITY x 'expanded'>]><document>&x;</document>",
        "<!DOCTYPE document [<!ENTITY x SYSTEM 'file:///credentials'>]><document>&x;</document>",
        "<!DOCTYPE document [<!ENTITY a 'aaaa'><!ENTITY b '&a;&a;&a;'>]><document>&b;</document>",
    ] {
        assert!(validate(xml.as_bytes(), false).unwrap_err().contains("DTD"));
    }
    for value in [
        "External",
        "eXtErNaL",
        "Exter&#110;al",
        "&#69;xternal",
        "Ex&#x74;ernal",
    ] {
        let xml = format!(
            "<Relationships><Relationship TargetMode='{value}' Target='https://example.invalid'/></Relationships>"
        );
        assert!(
            validate(xml.as_bytes(), true)
                .unwrap_err()
                .contains("External")
        );
    }
    assert!(
        validate(
            b"<Relationships><Relationship TargetMode='Internal'/></Relationships>".as_slice(),
            true
        )
        .is_ok()
    );
    assert!(validate(b"<Relationships xmlns:r='urn:other'><Relationship r:TargetMode='External'/></Relationships>".as_slice(), true).is_ok(), "namespaced attribute does not acquire the unqualified TargetMode meaning");
}

#[test]
fn full_eight_mib_parts_remain_valid_and_stream_without_tree_retention() {
    let open = b"<w:document xmlns:w='urn:word'>";
    let close = b"</w:document>";
    let mut xml = Vec::with_capacity(MAX_PART + 1);
    xml.extend_from_slice(open);
    let paragraph = b"<w:p><w:r><w:t>content</w:t></w:r></w:p>";
    while xml.len() + paragraph.len() + close.len() <= MAX_PART {
        xml.extend_from_slice(paragraph);
    }
    xml.resize(MAX_PART - close.len(), b' ');
    xml.extend_from_slice(close);
    assert_eq!(xml.len(), MAX_PART);
    struct Chunked<'a> {
        bytes: &'a [u8],
        largest_request: usize,
    }
    impl Read for Chunked<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.largest_request = self.largest_request.max(buffer.len());
            let count = buffer.len().min(self.bytes.len()).min(997);
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }
    let mut chunks = Chunked {
        bytes: &xml,
        largest_request: 0,
    };
    assert_eq!(validate(&mut chunks, false).unwrap(), "document");
    assert!(chunks.largest_request <= 8192);
    xml.push(b' ');
    assert!(
        validate(xml.as_slice(), false)
            .unwrap_err()
            .contains("8 MiB")
    );
    let mut giant_text = Vec::from(b"<document>".as_slice());
    giant_text.resize(MAX_PART - b"</document>".len(), b'x');
    giant_text.extend_from_slice(b"</document>");
    assert_eq!(validate(giant_text.as_slice(), false).unwrap(), "document");
}

#[test]
fn structural_xml_keeps_sixty_four_mib_text_cdata_comment_and_pi_allowance() {
    let limit = 64 * 1024 * 1024;
    for (open, close) in [
        ("<document>", "</document>"),
        ("<document><![CDATA[", "]]></document>"),
        ("<document><!--", "--></document>"),
        ("<document><?note ", "?></document>"),
    ] {
        // The generator itself retains no large test document.
        let body = std::io::repeat(b'x').take((limit - open.len() - close.len()) as u64);
        let input = Cursor::new(open.as_bytes())
            .chain(body)
            .chain(Cursor::new(close.as_bytes()));
        assert_eq!(
            validate_with_limit(input, false, limit).unwrap(),
            "document"
        );
    }
}

#[test]
fn pathological_metadata_is_rejected_explicitly_without_lowering_part_size() {
    let deep = format!(
        "{}{}",
        "<a>".repeat(MAX_DEPTH + 1),
        "</a>".repeat(MAX_DEPTH + 1)
    );
    assert!(
        validate(deep.as_bytes(), false)
            .unwrap_err()
            .contains("nesting")
    );
    let attributes = (0..=MAX_ATTRIBUTES)
        .map(|i| format!(" a{i}='x'"))
        .collect::<String>();
    assert!(
        validate(format!("<document{attributes}/>").as_bytes(), false)
            .unwrap_err()
            .contains("attribute budget")
    );
    let namespace = format!("<document xmlns:p='{}'/>", "x".repeat(MAX_NAMESPACE_BYTES));
    assert!(
        validate(namespace.as_bytes(), false)
            .unwrap_err()
            .contains("namespace budget")
    );
    assert!(
        validate(format!("<{} />", "x".repeat(4097)).as_bytes(), false)
            .unwrap_err()
            .contains("name budget")
    );
}
