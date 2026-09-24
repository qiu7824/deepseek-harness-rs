use serde::Deserialize;

#[derive(Debug, Default, PartialEq)]
enum Reason {
    #[default]
    Missing,
    Text(String),
}
impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::Text)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecision {
    risk: String,
    decision: String,
    #[serde(default)]
    reason: Reason,
}
#[derive(Debug, PartialEq)]
pub(crate) enum Decision {
    Allow,
    Deny(Option<String>),
}
pub(crate) fn parse(text: &str) -> Result<Decision, String> {
    let value: RawDecision = serde_json::from_str(text).map_err(|_| "invalid review JSON")?;
    match (value.risk.as_str(), value.decision.as_str(), value.reason) {
        ("low" | "medium", "allow", Reason::Missing) => Ok(Decision::Allow),
        ("medium" | "high", "deny", Reason::Missing) => Ok(Decision::Deny(None)),
        ("medium" | "high", "deny", Reason::Text(reason)) => Ok(Decision::Deny(Some(reason))),
        _ => Err("invalid risk/decision protocol".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_decision_protocol_rejects_duplicates_unknowns_and_invalid_combinations() {
        for text in [
            r#"{"risk":"low","decision":"allow"}"#,
            r#"{"risk":"medium","decision":"allow"}"#,
        ] {
            assert_eq!(parse(text).unwrap(), Decision::Allow);
        }
        assert_eq!(
            parse(r#"{"risk":"high","decision":"deny","reason":"secret transfer"}"#).unwrap(),
            Decision::Deny(Some("secret transfer".into()))
        );
        for text in [
            r#"{"risk":"high","decision":"allow"}"#,
            r#"{"risk":"low","decision":"deny"}"#,
            r#"{"risk":"low","decision":"allow","reason":null}"#,
            r#"{"risk":"low","decision":"allow","reason":"fine"}"#,
            r#"{"risk":"high","risk":"low","decision":"allow"}"#,
            r#"{"risk":"low","decision":"allow","extra":1}"#,
            r#"{"risk":"low","decision":"allow"} explanation"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
