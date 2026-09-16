use crate::*;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

pub fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn outcome(check: &AcceptanceCheck, input: &[u8], result: Result<String>) -> AcceptanceResult {
    let (status, coverage, failure_reason) = match result {
        Ok(coverage) => (AcceptanceStatus::Passed, coverage, None),
        Err(error) => (
            AcceptanceStatus::Failed,
            "Only the declared assertions were checked.".into(),
            Some(error),
        ),
    };
    AcceptanceResult {
        check_id: check.id.clone(),
        checker_version: CHECKER_VERSION.into(),
        input_identity: digest(input),
        status,
        evidence_refs: vec![format!("sha256:{}", digest(input))],
        coverage,
        failure_reason,
    }
}

fn json_assertions(value: &Value, assertions: &BTreeMap<String, Value>) -> Result<String> {
    if assertions.is_empty() {
        return Err("At least one content assertion is required".into());
    }
    for (pointer, expected) in assertions {
        if !pointer.starts_with('/') || value.pointer(pointer) != Some(expected) {
            return Err(format!("Assertion at JSON pointer {pointer} did not match"));
        }
    }
    Ok(format!(
        "{} explicit JSON content assertions passed; unspecified fields were not checked.",
        assertions.len()
    ))
}

/// Bounded native checks. Office structural checks intentionally do not claim layout verification.
pub fn check_bytes(check: &AcceptanceCheck, bytes: &[u8]) -> AcceptanceResult {
    let result = (|| match &check.checker {
        Checker::Text {
            required,
            forbidden,
            ..
        } => {
            if required.is_empty() && forbidden.is_empty() {
                return Err("Text acceptance needs at least one content constraint".into());
            }
            let text = std::str::from_utf8(bytes).map_err(|_| "Not valid UTF-8 text")?;
            for needle in required {
                if !text.contains(needle) {
                    return Err(format!("Missing required text: {needle}"));
                }
            }
            for needle in forbidden {
                if text.contains(needle) {
                    return Err(format!("Forbidden text present: {needle}"));
                }
            }
            Ok("UTF-8 decoding and all declared text constraints passed.".into())
        }
        Checker::Json { assertions, .. } => json_assertions(
            &serde_json::from_slice::<Value>(bytes).map_err(|e| format!("Invalid JSON: {e}"))?,
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
            let mut reader = image::ImageReader::new(Cursor::new(bytes))
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
            let required = match format.as_str() {
                "docx" => "word/document.xml",
                "xlsx" => "xl/workbook.xml",
                "pptx" => "ppt/presentation.xml",
                _ => return Err("Unsupported Office package type".into()),
            };
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| format!("Cannot open Office package: {e}"))?;
            if archive.len() > 10_000 {
                return Err("Office package has too many entries".into());
            }
            let mut total = 0u64;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
                total = total.saturating_add(entry.size());
                if total > 64 * 1024 * 1024 {
                    return Err("Office expanded content exceeds validation budget".into());
                }
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| format!("Corrupt Office entry: {e}"))?;
            }
            for name in ["[Content_Types].xml", required] {
                let mut entry = archive
                    .by_name(name)
                    .map_err(|_| format!("Office package missing {name}"))?;
                let mut xml = String::new();
                entry.read_to_string(&mut xml).map_err(|e| e.to_string())?;
                roxmltree::Document::parse(&xml).map_err(|e|format!("Invalid XML content in {name}: {e}"))?;
            }
            Ok("Office ZIP and required XML parts are readable; page layout, formula correctness and application rendering require separate acceptance checks.".into())
        }
        _ => Err("This checker does not accept file bytes".into()),
    })();
    outcome(check, bytes, result)
}

pub fn check_tool_result(check: &AcceptanceCheck, step: Option<&Step>) -> AcceptanceResult {
    let bytes = step
        .and_then(|step| serde_json::to_vec(step).ok())
        .unwrap_or_default();
    let result = (|| {
        let Checker::ToolResult { assertions, .. } = &check.checker else {
            return Err("Not a tool result checker".into());
        };
        let step = step.ok_or("Required execution has not been recorded")?;
        let expected_readonly_failure = step.state == StepState::Failed
            && step.effect == EffectKind::ReadOnly
            && (assertions.get("/isError") == Some(&Value::Bool(true))
                || assertions
                    .get("/exitCode")
                    .and_then(Value::as_i64)
                    .is_some_and(|code| code != 0));
        if !matches!(step.state, StepState::Verified | StepState::Committed)
            && !expected_readonly_failure
        {
            return Err("Execution has not reached a verified terminal state".into());
        }
        json_assertions(step.result.as_ref().ok_or("Result too large or unavailable; inspect retained log and choose a bounded verification result")?,assertions)
    })();
    let mut result = outcome(check, &bytes, result);
    if let Some(step) = step {
        result
            .evidence_refs
            .push(format!("execution:{}", step.execution_id));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_flag_is_not_content_acceptance() {
        let check = AcceptanceCheck {
            id: "c".into(),
            description: "answer".into(),
            checker: Checker::Json {
                path: "out.json".into(),
                assertions: BTreeMap::from([("/answer".into(), Value::from(42))]),
            },
        };
        assert_eq!(
            check_bytes(&check, br#"{"exitCode":0,"answer":0}"#).status,
            AcceptanceStatus::Failed
        );
        assert_eq!(
            check_bytes(&check, br#"{"answer":42}"#).status,
            AcceptanceStatus::Passed
        );
    }
    #[test]
    fn corrupt_image_never_passes() {
        let check = AcceptanceCheck {
            id: "c".into(),
            description: "image".into(),
            checker: Checker::Image {
                path: "中文.png".into(),
                min_width: 1,
                min_height: 1,
                channels: None,
            },
        };
        assert_eq!(
            check_bytes(&check, b"\x89PNG\r\n\x1a\n").status,
            AcceptanceStatus::Failed
        );
    }
    #[test]
    fn office_garbage_fails_in_correct_stage() {
        let check = AcceptanceCheck {
            id: "c".into(),
            description: "document".into(),
            checker: Checker::OfficePackage {
                path: "a.docx".into(),
                format: "docx".into(),
            },
        };
        assert!(
            check_bytes(&check, b"not a zip")
                .failure_reason
                .unwrap()
                .contains("Cannot open Office")
        );
    }
}
