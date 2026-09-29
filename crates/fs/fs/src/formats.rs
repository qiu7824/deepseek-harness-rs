//! Binary container formats that text publication must not claim. Renaming
//! HTML or CSV to `.xlsx` does not convert it; every path that can publish a
//! file under one of these extensions checks the bytes it actually carries.
use std::path::Path;

/// Leading bytes needed by [`content_matches_extension`].
pub const SIGNATURE_BYTES: usize = 12;

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}

/// `None` when the extension is not a binary format this module knows.
fn signature_matches(extension: &str, head: &[u8]) -> Option<bool> {
    const ZIP: &[u8] = b"PK\x03\x04";
    const OLE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
    Some(match extension {
        "docx" | "docm" | "dotx" | "xlsx" | "xlsm" | "xlsb" | "pptx" | "pptm" => {
            head.starts_with(ZIP)
        }
        "zip" => head.starts_with(ZIP) || head.starts_with(b"PK\x05\x06"),
        "doc" | "xls" | "ppt" => head.starts_with(OLE),
        "pdf" => head.starts_with(b"%PDF-"),
        "png" => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => head.starts_with(b"\xFF\xD8\xFF"),
        "gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "webp" => head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP",
        _ => return None,
    })
}

/// Whether the extension names a binary document, archive or image format.
pub fn is_binary_format_path(path: &Path) -> bool {
    extension(path).is_some_and(|extension| signature_matches(&extension, &[]).is_some())
}

/// Whether `head` (at least [`SIGNATURE_BYTES`] leading bytes when the file
/// is that long) carries the container its extension names. Paths without a
/// binary format extension always match.
pub fn content_matches_extension(path: &Path, head: &[u8]) -> bool {
    extension(path)
        .and_then(|extension| signature_matches(&extension, head))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_text_never_matches_a_binary_container() {
        let html = b"<html><table><tr><td>17603339142</td></tr></table></html>";
        for name in [
            "通讯录.xlsx",
            "值班表.DOCX",
            "表.xls",
            "a.pdf",
            "b.png",
            "c.webp",
        ] {
            assert!(is_binary_format_path(Path::new(name)), "{name}");
            assert!(!content_matches_extension(Path::new(name), html), "{name}");
        }
        assert!(content_matches_extension(
            Path::new("通讯录.xlsx"),
            b"PK\x03\x04\x14\x00"
        ));
        assert!(content_matches_extension(
            Path::new("old.xls"),
            b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1"
        ));
        assert!(content_matches_extension(
            Path::new("page.webp"),
            b"RIFF\x10\x00\x00\x00WEBPVP8 "
        ));
        assert!(!content_matches_extension(Path::new("page.webp"), b"RIFF"));
        for name in ["preview.html", "data.csv", "notes", "script.py"] {
            assert!(!is_binary_format_path(Path::new(name)), "{name}");
            assert!(content_matches_extension(Path::new(name), html), "{name}");
        }
    }
}
