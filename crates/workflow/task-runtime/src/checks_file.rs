//! File-handle acceptance; input identity always describes the bytes checked.
use super::*;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

struct CheckedInput<'a> {
    file: &'a mut std::fs::File,
    aborted: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}
impl Read for CheckedInput<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.aborted.is_some_and(|signal| signal()) {
            return Err(io::Error::other("Task validation cancelled"));
        }
        self.file.read(bytes)
    }
}
impl Seek for CheckedInput<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

pub fn check_file(
    check: &AcceptanceCheck,
    file: &mut std::fs::File,
    aborted: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> AcceptanceResult {
    let mut input = CheckedInput { file, aborted };
    let mut identity = String::new();
    let result = (|| {
        input.rewind().map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        identity = format!("{:x}", hash.finalize());
        input.rewind().map_err(|e| e.to_string())?;
        match &check.checker {
            Checker::Text {
                required,
                forbidden,
                ..
            } => check_text(&mut input, required, forbidden),
            Checker::Json { assertions, .. } => json_assertions(
                &serde_json::from_reader(BufReader::new(&mut input))
                    .map_err(|e| format!("Invalid JSON: {e}"))?,
                assertions,
            ),
            Checker::Image {
                min_width,
                min_height,
                channels,
                ..
            } => {
                if *min_width == 0 || *min_height == 0 {
                    return Err("Image dimensions must be positive".into());
                }
                let mut reader = image::ImageReader::new(BufReader::new(&mut input))
                    .with_guessed_format()
                    .map_err(|e| e.to_string())?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(20_000);
                limits.max_image_height = Some(20_000);
                limits.max_alloc = Some(256 * 1024 * 1024);
                reader.limits(limits);
                let image = reader
                    .decode()
                    .map_err(|e| format!("Image decode failed: {e}"))?;
                if image.width() < *min_width
                    || image.height() < *min_height
                    || channels.is_some_and(|n| image.color().channel_count() != n)
                {
                    return Err(format!(
                        "Unexpected image dimensions/channels: {}x{} / {}",
                        image.width(),
                        image.height(),
                        image.color().channel_count()
                    ));
                }
                Ok(format!(
                    "Image decoded: {}x{}, {} channels; visual content was not judged.",
                    image.width(),
                    image.height(),
                    image.color().channel_count()
                ))
            }
            Checker::OfficePackage { format, .. } => {
                crate::office::validate_structure(&mut input, format)?;
                Ok("Office ZIP and required XML parts are readable; page layout, formula correctness and application rendering require separate acceptance checks.".into())
            }
            _ => Err("This checker does not accept file bytes".into()),
        }
    })();
    outcome_with_identity(check, identity, result)
}

fn check_text<R: Read>(mut input: R, required: &[String], forbidden: &[String]) -> Result<String> {
    if required.is_empty() && forbidden.is_empty() {
        return Err("Text acceptance needs at least one content constraint".into());
    }
    if forbidden.iter().any(String::is_empty) {
        return Err("Forbidden text present: ".into());
    }
    let retain = required
        .iter()
        .chain(forbidden)
        .map(String::len)
        .max()
        .unwrap_or(0)
        .saturating_sub(1);
    let mut found = required.iter().map(String::is_empty).collect::<Vec<_>>();
    let mut tail = Vec::new();
    let mut utf8_tail = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        utf8_tail.extend_from_slice(&buffer[..count]);
        match std::str::from_utf8(&utf8_tail) {
            Ok(_) => utf8_tail.clear(),
            Err(error) if error.error_len().is_none() => {
                utf8_tail.drain(..error.valid_up_to());
            }
            Err(_) => return Err("Not valid UTF-8 text".into()),
        }
        tail.extend_from_slice(&buffer[..count]);
        for (index, needle) in required.iter().enumerate() {
            if !found[index] {
                found[index] = memchr::memmem::find(&tail, needle.as_bytes()).is_some();
            }
        }
        for needle in forbidden {
            if memchr::memmem::find(&tail, needle.as_bytes()).is_some() {
                return Err(format!("Forbidden text present: {needle}"));
            }
        }
        if tail.len() > retain {
            tail.drain(..tail.len() - retain);
        }
    }
    if !utf8_tail.is_empty() {
        return Err("Not valid UTF-8 text".into());
    }
    for (needle, found) in required.iter().zip(found) {
        if !found {
            return Err(format!("Missing required text: {needle}"));
        }
    }
    Ok("UTF-8 decoding and all declared text constraints passed.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn text_checks_match_whole_utf8_input_across_read_boundaries() {
        let mut bytes = vec![b'x'; 65535];
        bytes.extend_from_slice("文字 must-match".as_bytes());
        assert!(check_text(Cursor::new(&bytes), &["文字 must-match".into()], &[]).is_ok());
        assert!(check_text(Cursor::new(&bytes), &[], &["文字 must-match".into()]).is_err());
        bytes.push(0xe4);
        assert!(
            check_text(Cursor::new(&bytes), &["must-match".into()], &[])
                .unwrap_err()
                .contains("UTF-8")
        );
        assert!(check_text(Cursor::new(b""), &[String::new()], &[]).is_ok());
        assert!(check_text(Cursor::new(b""), &[], &[String::new()]).is_err());
    }
    #[test]
    fn file_checks_preserve_content_hash_and_assertions() {
        let path = std::env::temp_dir().join(format!("dsh-check-file-{}", uuid::Uuid::new_v4()));
        for (data, checker) in [
            (
                b"correct output".to_vec(),
                Checker::Text {
                    path: "out.txt".into(),
                    required: vec!["correct".into()],
                    forbidden: vec!["wrong".into()],
                },
            ),
            (
                br#"{"answer":42}"#.to_vec(),
                Checker::Json {
                    path: "out.json".into(),
                    assertions: BTreeMap::from([("/answer".into(), json!(42))]),
                },
            ),
            (
                crate::office::feature_docx().unwrap(),
                Checker::OfficePackage {
                    path: "out.docx".into(),
                    format: "docx".into(),
                },
            ),
        ] {
            std::fs::write(&path, &data).unwrap();
            let check = AcceptanceCheck {
                id: "check".into(),
                description: "content".into(),
                checker,
            };
            let mut file = std::fs::File::open(&path).unwrap();
            let streamed = check_file(&check, &mut file, None);
            let whole = check_bytes(&check, &data);
            assert_eq!(streamed, whole);
            assert_eq!(
                check_file(&check, &mut file, Some(&|| true)).status,
                AcceptanceStatus::Failed
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn obsolete_office_evidence_blocks_reuse_without_rewriting_history_or_manual_approval() {
        let mut task: TaskContract = serde_json::from_value(json!({
            "version":1,"taskId":"old","owner":"owner","revision":9,
            "spec":{"objective":"Deliver","acceptanceChecks":[
                {"id":"office","description":"package","checker":{"kind":"office_package","path":"out.docx","format":"docx"}},
                {"id":"manual","description":"appearance","checker":{"kind":"manual","reason":"Visual inspection"}}
            ]},"state":"completed","steps":[],"acceptanceResults":[],
            "validationIdentity":"previous","outputIdentities":{"out.docx":"hash"},"createdAt":1,"updatedAt":2
        })).unwrap();
        for id in ["office", "manual"] {
            task.acceptance_results.push(AcceptanceResult {
                check_id: id.into(),
                checker_version: CHECKER_VERSION.into(),
                input_identity: "hash".into(),
                status: AcceptanceStatus::Passed,
                evidence_refs: vec![format!("user-confirmation:{id}")],
                coverage: "verified".into(),
                failure_reason: None,
            });
        }
        let historical = serde_json::to_value(&task).unwrap();
        let blockers = task.completion_blockers();
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("obsolete checker"));
        assert_eq!(serde_json::to_value(&task).unwrap(), historical);
        task.acceptance_results[0].checker_version =
            task.spec.acceptance_checks[0].checker.version().into();
        assert!(task.completion_blockers().is_empty());
        assert_eq!(task.acceptance_results[1].checker_version, CHECKER_VERSION);
        assert_eq!(task.state, TaskState::Completed);
    }
}
