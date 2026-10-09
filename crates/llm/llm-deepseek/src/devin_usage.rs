use super::Message;
use serde_json::{Value, json};

fn child<'a>(parent: &Message<'a>, tag: u32) -> Result<Option<Message<'a>>, String> {
    parent.bytes(tag)?.map(Message::parse).transpose()
}

fn percent(status: &Message<'_>, tag: u32) -> Result<Option<u64>, String> {
    match status.optional_number(tag)? {
        Some(value) if value <= 100 => Ok(Some(value)),
        Some(_) => Err("Devin 额度百分比超出有效范围".into()),
        None => Ok(None),
    }
}

fn reset(status: &Message<'_>, tag: u32) -> Result<Option<u64>, String> {
    Ok(status
        .optional_number(tag)?
        .filter(|value| *value > 0 && *value < 32_503_680_000))
}

fn plan_name(plan: Option<&Message<'_>>) -> Result<Option<String>, String> {
    let Some(plan) = plan else { return Ok(None) };
    let name = plan.text(2)?.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.len() > 256 || name.chars().any(char::is_control) {
        return Err("Devin 套餐名称无效".into());
    }
    Ok(Some(name.to_owned()))
}

pub(super) fn decode(bytes: &[u8]) -> Result<Value, String> {
    let root = Message::parse(bytes)?;
    let user = child(&root, 1)?.ok_or("Devin 额度响应缺少账号状态")?;
    let status = child(&user, 13)?;
    let top_plan = child(&root, 2)?;
    let nested_plan = status.as_ref().map(|s| child(s, 1)).transpose()?.flatten();
    let top_name = plan_name(top_plan.as_ref())?;
    let nested_name = plan_name(nested_plan.as_ref())?;
    if top_name.is_some() && nested_name.is_some() && top_name != nested_name {
        return Err("Devin 套餐状态不一致".into());
    }
    let name = top_name.or(nested_name);
    let top_strategy = top_plan
        .as_ref()
        .map(|p| p.optional_number(35))
        .transpose()?
        .flatten()
        .filter(|n| *n != 0);
    let nested_strategy = nested_plan
        .as_ref()
        .map(|p| p.optional_number(35))
        .transpose()?
        .flatten()
        .filter(|n| *n != 0);
    if top_strategy.is_some() && nested_strategy.is_some() && top_strategy != nested_strategy {
        return Err("Devin 套餐计费状态不一致".into());
    }
    let strategy = top_strategy.or(nested_strategy);
    let mut windows = Vec::new();
    if let Some(status) = status {
        for (id, label, quota_tag, reset_tag, hide_tag, mins) in [
            ("daily", "日额度", 14, 17, 36, 1440),
            ("weekly", "周额度", 15, 18, 37, 10080),
        ] {
            // An explicit hide flag suppresses the window. Scalar omission
            // remains unknown; proto defaults cannot prove quota exhaustion.
            let hidden = [top_plan.as_ref(), nested_plan.as_ref()]
                .into_iter()
                .flatten()
                .map(|p| p.optional_number(hide_tag))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|n| n == Some(1));
            if hidden {
                continue;
            }
            let remaining = percent(&status, quota_tag)?;
            let resets_at = reset(&status, reset_tag)?;
            if resets_at.is_none() && strategy != Some(2) {
                continue;
            }
            windows.push(json!({"id":id,"label":label,"unit":"percent",
                "usedPercent":remaining.map(|n|100-n),"remainingPercent":remaining,
                "resetsAt":resets_at,"windowDurationMins":mins,
                "used":null,"remaining":null,"limit":null}));
        }
    }
    // Legacy credit sentinels and the extra balance are deliberately not
    // converted to quota or money without a verified unit and contract.
    Ok(json!({"plan":name,"windows":windows}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devin_wire::Encoder;

    fn response(status: Vec<u8>, strategy: u64, hidden: bool) -> Vec<u8> {
        let mut plan = Encoder::default();
        plan.text(2, "Pro");
        plan.number(35, strategy);
        plan.number(36, u64::from(hidden));
        let mut user = Encoder::default();
        if status.is_empty() {
            user.0.extend_from_slice(&[0x6a, 0]);
        } else {
            user.bytes(13, &status);
        }
        let mut root = Encoder::default();
        root.bytes(1, &user.0);
        root.bytes(2, &plan.0);
        root.0
    }

    #[test]
    fn native_status_reports_remaining_and_separate_resets() {
        let mut status = Encoder::default();
        status.number(14, 63);
        status.number(15, 81);
        status.number(17, 1791273600);
        status.number(18, 1791705600);
        status.number(8, u64::MAX);
        status.number(16, u64::MAX);
        let value = decode(&response(status.0, 2, false)).unwrap();
        assert_eq!(value["plan"], "Pro");
        assert_eq!(value["windows"][0]["remainingPercent"], 63);
        assert_eq!(value["windows"][0]["usedPercent"], 37);
        assert_eq!(value["windows"][1]["resetsAt"], 1791705600u64);
        assert!(value["windows"][0]["limit"].is_null());
        assert!(value.get("overageBalance").is_none());
    }

    #[test]
    fn absent_percent_is_unknown_and_explicit_zero_is_exhausted() {
        let value = decode(&response(Vec::new(), 2, false)).unwrap();
        assert!(value["windows"][0]["usedPercent"].is_null());
        let value = decode(&response(vec![0x70, 0], 2, false)).unwrap();
        assert_eq!(value["windows"][0]["remainingPercent"], 0);
        assert_eq!(value["windows"][0]["usedPercent"], 100);
    }

    #[test]
    fn hide_daily_and_credit_plans_do_not_create_fake_zero_windows() {
        let value = decode(&response(Vec::new(), 2, true)).unwrap();
        assert_eq!(value["windows"].as_array().unwrap().len(), 1);
        assert_eq!(value["windows"][0]["id"], "weekly");
        assert!(
            decode(&response(Vec::new(), 1, false)).unwrap()["windows"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_percentage_is_rejected() {
        let mut status = Encoder::default();
        status.number(14, 101);
        assert!(decode(&response(status.0, 2, false)).is_err());
    }

    #[test]
    fn conflicting_billing_strategies_are_rejected() {
        let mut nested = Encoder::default();
        nested.text(2, "Pro");
        nested.number(35, 1);
        let mut status = Encoder::default();
        status.bytes(1, &nested.0);
        status.number(14, 63);
        assert!(decode(&response(status.0, 2, false)).is_err());
    }

    #[test]
    fn undated_credit_defaults_do_not_create_quota_progress() {
        let mut status = Encoder::default();
        status.number(14, 63);
        assert!(
            decode(&response(status.0, 1, false)).unwrap()["windows"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let mut status = Encoder::default();
        status.number(14, 63);
        status.number(17, 1791273600);
        assert_eq!(
            decode(&response(status.0, 1, false)).unwrap()["windows"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
