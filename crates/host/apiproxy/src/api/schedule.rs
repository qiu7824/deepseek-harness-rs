//! Cold Host reminder management; records remain bound to their original session.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleDeleteRequest {
    pub session_id: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleCreateRequest {
    pub session_id: String,
    #[serde(flatten)]
    pub reminder: dsh_schedule::host_types::ScheduleCreateRequest,
}

pub use dsh_schedule::host_types::{HistoryRequest, ScheduleUpdateRequest};

pub fn decode_create_request(
    value: &serde_json::Value,
) -> Result<ScheduleCreateRequest, dsh_schedule::host_types::ScheduleError> {
    use dsh_schedule::host_types::ScheduleError;
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| ScheduleError::invalid("Schedule creation requires an object."))?;
    let session_id = object
        .remove("sessionId")
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.is_empty() && value.trim() == value)
        .ok_or_else(|| ScheduleError::invalid("sessionId must be a non-empty, trimmed string."))?;
    let reminder =
        dsh_schedule::calendar::decode_create_request(&serde_json::Value::Object(object))?;
    Ok(ScheduleCreateRequest {
        session_id,
        reminder,
    })
}

pub fn decode_history_request(
    value: &serde_json::Value,
) -> Result<HistoryRequest, dsh_schedule::host_types::ScheduleError> {
    use dsh_schedule::host_types::ScheduleError;
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| ScheduleError::invalid("Schedule history requires an object."))?;
    let limit = object
        .get("limit")
        .and_then(serde_json::Value::as_f64)
        .filter(|limit| limit.is_finite() && limit.fract() == 0.0 && (1.0..=100.0).contains(limit))
        .ok_or_else(|| {
            ScheduleError::invalid("History limit must be an integer between 1 and 100.")
        })?;
    if object
        .get("before")
        .is_some_and(|before| !before.is_string())
    {
        return Err(ScheduleError::invalid(
            "before must be a message identity when present.",
        ));
    }
    object.insert("limit".into(), serde_json::Value::from(limit as u64));
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ScheduleError::invalid(error.to_string()))
}
