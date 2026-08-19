//! The one definition of a well-formed provider API key. Rust port of
//! `packages/llm/llm/src/api-key.ts`.

/// Why a supplied API key cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyRejection {
    Empty,
    IllegalCharacters,
}

/// The verdict on one supplied API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyCheck {
    Ok { value: String },
    Rejected { reason: ApiKeyRejection },
}

/// Judge one *supplied* API key, trimming surrounding whitespace first (TS
/// `normalizeApiKey`).
pub fn normalize_api_key(raw: &str) -> ApiKeyCheck {
    let value = raw.trim();
    if value.is_empty() {
        return ApiKeyCheck::Rejected {
            reason: ApiKeyRejection::Empty,
        };
    }
    // Printable ASCII, space excluded (TS `LEGAL_API_KEY`).
    if value.chars().any(|ch| !('\u{21}'..='\u{7E}').contains(&ch)) {
        return ApiKeyCheck::Rejected {
            reason: ApiKeyRejection::IllegalCharacters,
        };
    }
    ApiKeyCheck::Ok {
        value: value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_judges_keys() {
        assert_eq!(
            normalize_api_key("  sk-abc123  "),
            ApiKeyCheck::Ok {
                value: "sk-abc123".to_string()
            }
        );
        assert_eq!(
            normalize_api_key("   "),
            ApiKeyCheck::Rejected {
                reason: ApiKeyRejection::Empty
            }
        );
        assert_eq!(
            normalize_api_key("sk\u{00E9}abc"),
            ApiKeyCheck::Rejected {
                reason: ApiKeyRejection::IllegalCharacters
            }
        );
    }
}
