//! Durable inbox acknowledgments, append-time retention and exclusive cursor pages.

use crate::calendar::{decode_record, parse_instant, trim_js, validate_record};
use crate::host_types::*;
use serde_json::Value;
use std::collections::HashSet;

fn validate_receipt(receipt: &DeliveryReceipt) -> Result<(), ScheduleError> {
    parse_instant(&receipt.scheduled_at)?;
    parse_instant(&receipt.delivered_at)?;
    if receipt.message_id.is_empty() || trim_js(&receipt.message_id) != receipt.message_id {
        return Err(ScheduleError::corrupt(
            "Delivery message identity must be non-empty and trimmed.",
        ));
    }
    Ok(())
}

/// Validate a typed task without changing its stored zone spelling or committed target.
pub fn validate_task_record(task: &HostScheduleTask) -> Result<(), ScheduleError> {
    if task.session_id.is_empty() {
        return Err(ScheduleError::corrupt("Task sessionId must not be empty."));
    }
    validate_record(&task.record)?;
    if let Some(receipt) = &task.last_delivery {
        validate_receipt(receipt)?;
    }
    if let Some(history) = &task.delivery_history {
        let mut identities = HashSet::new();
        for record in &history.records {
            validate_receipt(&record.receipt())?;
            if !identities.insert(record.message_id.as_str()) {
                return Err(ScheduleError::corrupt(
                    "Delivery message identities must be unique within a task.",
                ));
            }
        }
        if history.records.last().map(DeliveryRecord::receipt) != task.last_delivery {
            return Err(ScheduleError::corrupt(
                "Last delivery must match the latest saved receipt.",
            ));
        }
    }
    Ok(())
}

/// Decode the complete strict storage schema; absent status is active, absent history stays absent.
pub fn validate_task(value: &Value) -> Result<HostScheduleTask, ScheduleError> {
    let mut value = value.clone();
    let object = value
        .as_object_mut()
        .ok_or_else(|| ScheduleError::corrupt("Schedule task must be an object."))?;
    for key in ["lastDelivery", "deliveryHistory"] {
        if object.get(key).is_some_and(Value::is_null) {
            return Err(ScheduleError::corrupt(format!("{key} cannot be null.")));
        }
    }
    if let Some(history) = object.get("deliveryHistory").and_then(Value::as_object) {
        if history
            .get("earlierRecordsPruned")
            .is_some_and(Value::is_null)
        {
            return Err(ScheduleError::corrupt(
                "earlierRecordsPruned must be a boolean.",
            ));
        }
        if history
            .get("records")
            .and_then(Value::as_array)
            .is_some_and(|records| {
                records
                    .iter()
                    .any(|record| record.get("prompt").is_some_and(Value::is_null))
            })
        {
            return Err(ScheduleError::corrupt(
                "A saved delivery prompt must be a string when present.",
            ));
        }
    }
    let record = decode_record(
        object
            .get("record")
            .ok_or_else(|| ScheduleError::corrupt("Schedule task requires a record."))?,
    )?;
    object.insert(
        "record".into(),
        serde_json::to_value(record).expect("serializable reminder"),
    );
    let task: HostScheduleTask =
        serde_json::from_value(value).map_err(|e| ScheduleError::corrupt(e.to_string()))?;
    validate_task_record(&task)?;
    Ok(task)
}

fn history_of(task: &HostScheduleTask) -> DeliveryHistory {
    task.delivery_history
        .clone()
        .unwrap_or_else(|| DeliveryHistory {
            records: task
                .last_delivery
                .iter()
                .map(|receipt| DeliveryRecord::from_receipt(receipt, None))
                .collect(),
            earlier_records_unavailable: true,
            earlier_records_pruned: None,
        })
}

/// Prepare the same task write that commits its next target/status after a real inbox flush.
pub fn append_delivery(
    task: &HostScheduleTask,
    receipt: &DeliveryReceipt,
    bounds: &RetentionBounds,
) -> Result<HostScheduleTask, ScheduleError> {
    bounds.validate()?;
    validate_task_record(task)?;
    validate_receipt(receipt)?;
    let history = history_of(task);
    if history
        .records
        .iter()
        .any(|record| record.message_id == receipt.message_id)
    {
        return Err(ScheduleError::corrupt(
            "Delivery message identity is already recorded for this task.",
        ));
    }
    let mut records = history.records;
    records.push(DeliveryRecord::from_receipt(
        receipt,
        Some(task.record.prompt().to_owned()),
    ));
    let appended_count = records.len();
    let floor = parse_instant(&receipt.delivered_at)? - i64::from(bounds.days) * 86_400_000;
    records.retain(|record| parse_instant(&record.delivered_at).is_ok_and(|time| time >= floor));
    if records.len() > bounds.records {
        records.drain(..records.len() - bounds.records);
    }
    let pruned = records.len() != appended_count;
    let mut result = task.clone();
    result.last_delivery = Some(receipt.clone());
    result.delivery_history = Some(DeliveryHistory {
        records,
        earlier_records_unavailable: history.earlier_records_unavailable || pruned,
        earlier_records_pruned: Some(history.earlier_records_pruned == Some(true) || pruned),
    });
    validate_task_record(&result)?;
    Ok(result)
}

/// Read by append order, without pruning, reconstructing receipts, or consulting Session logs.
pub fn history_page(
    task: &HostScheduleTask,
    request: &HistoryRequest,
    retention: &RetentionBounds,
) -> Result<HistoryResult, ScheduleError> {
    if !(1..=100).contains(&request.limit) {
        return Err(ScheduleError::invalid(
            "Delivery history limit must be a safe integer from 1 through 100.",
        ));
    }
    retention.validate()?;
    if task.session_id != request.session_id || task.record.id() != request.id {
        return Ok(HistoryResult::Miss {
            id: request.id.clone(),
            code: "schedule_not_found".into(),
        });
    }
    validate_task_record(task)?;
    let history = history_of(task);
    let end = match &request.before {
        None => history.records.len(),
        Some(cursor) => match history
            .records
            .iter()
            .position(|record| record.message_id == *cursor)
        {
            Some(index) => index,
            None => {
                return Ok(HistoryResult::Miss {
                    id: request.id.clone(),
                    code: "delivery_cursor_not_found".into(),
                });
            }
        },
    };
    let start = end.saturating_sub(request.limit);
    let records: Vec<_> = history.records[start..end].iter().rev().cloned().collect();
    let next_before = if start > 0 {
        records.last().map(|record| record.message_id.clone())
    } else {
        None
    };
    Ok(HistoryResult::Page {
        id: request.id.clone(),
        records,
        earlier_records_unavailable: history.earlier_records_unavailable,
        earlier_records_pruned: history.earlier_records_pruned == Some(true),
        retention: *retention,
        next_before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::{create_record, decode_create_request};
    use serde_json::json;

    fn task() -> HostScheduleTask {
        HostScheduleTask {
            session_id: "s".into(),
            record: create_record(
                "a",
                &decode_create_request(
                    &json!({"title":"task","prompt":"original","every_seconds":60}),
                )
                .unwrap(),
                0,
            )
            .unwrap(),
            status: TaskStatus::Active,
            last_delivery: None,
            delivery_history: Some(DeliveryHistory::empty()),
        }
    }
    fn receipt(id: &str, at: &str) -> DeliveryReceipt {
        DeliveryReceipt {
            scheduled_at: "2026-01-01T00:00:00.000Z".into(),
            delivered_at: at.into(),
            message_id: id.into(),
        }
    }
    fn request(limit: usize, before: Option<&str>) -> HistoryRequest {
        HistoryRequest {
            session_id: "s".into(),
            id: "a".into(),
            limit,
            before: before.map(str::to_owned),
        }
    }

    #[test]
    fn pages_follow_append_order_even_if_clock_rolls_back() {
        let mut task = task();
        let bounds = RetentionBounds::default();
        for (id, at) in [
            ("first", "2026-01-03T00:00:00.000Z"),
            ("second", "2026-01-02T00:00:00.000Z"),
            ("third", "2026-01-01T00:00:00.000Z"),
        ] {
            task = append_delivery(&task, &receipt(id, at), &bounds).unwrap();
        }
        let HistoryResult::Page {
            records,
            next_before,
            earlier_records_unavailable,
            ..
        } = history_page(&task, &request(2, None), &bounds).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            records
                .iter()
                .map(|r| r.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["third", "second"]
        );
        assert_eq!(next_before.as_deref(), Some("second"));
        assert!(!earlier_records_unavailable);
        let HistoryResult::Page {
            records,
            next_before,
            ..
        } = history_page(&task, &request(2, Some("second")), &bounds).unwrap()
        else {
            panic!()
        };
        assert_eq!(records[0].message_id, "first");
        assert_eq!(next_before, None);
        assert!(
            matches!(history_page(&task,&request(2,Some("missing")),&bounds).unwrap(),HistoryResult::Miss { code,.. } if code=="delivery_cursor_not_found")
        );
    }

    #[test]
    fn append_prunes_window_then_count_and_keeps_immutable_sent_prompt() {
        let bounds = RetentionBounds {
            days: 2,
            records: 2,
        };
        let mut task = task();
        for (id, at) in [
            ("a", "2026-01-01T00:00:00.000Z"),
            ("b", "2026-01-02T00:00:00.000Z"),
            ("c", "2026-01-03T00:00:00.000Z"),
        ] {
            task = append_delivery(&task, &receipt(id, at), &bounds).unwrap();
        }
        task.record = task
            .record
            .with_content("new title".into(), "changed".into());
        task = append_delivery(&task, &receipt("d", "2026-01-05T00:00:00.000Z"), &bounds).unwrap();
        let history = task.delivery_history.as_ref().unwrap();
        assert_eq!(history.records.len(), 2);
        assert_eq!(history.records[0].message_id, "c");
        assert_eq!(history.records[0].prompt.as_deref(), Some("original"));
        assert_eq!(history.records[1].prompt.as_deref(), Some("changed"));
        assert!(history.earlier_records_unavailable);
        assert_eq!(history.earlier_records_pruned, Some(true));
    }

    #[test]
    fn legacy_receipt_is_read_only_without_invented_history() {
        let mut task = task();
        task.delivery_history = None;
        task.last_delivery = Some(receipt("last", "2026-01-01T00:00:00.000Z"));
        let original = task.clone();
        let HistoryResult::Page {
            records,
            earlier_records_unavailable,
            earlier_records_pruned,
            ..
        } = history_page(&task, &request(1, None), &RetentionBounds::default()).unwrap()
        else {
            panic!()
        };
        assert_eq!(task, original);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, None);
        assert!(earlier_records_unavailable);
        assert!(!earlier_records_pruned);
    }

    #[test]
    fn schema_refuses_unknown_null_duplicate_and_inconsistent_receipts() {
        let value = serde_json::to_value(task()).unwrap();
        for (field, bad) in [
            ("other", json!(true)),
            ("lastDelivery", Value::Null),
            ("deliveryHistory", Value::Null),
            ("status", Value::Null),
        ] {
            let mut candidate = value.clone();
            candidate[field] = bad;
            assert!(validate_task(&candidate).is_err(), "{field}");
        }
        let mut task = append_delivery(
            &task(),
            &receipt("one", "2026-01-01T00:00:00.000Z"),
            &RetentionBounds::default(),
        )
        .unwrap();
        assert!(
            append_delivery(
                &task,
                &receipt("one", "2026-01-02T00:00:00.000Z"),
                &RetentionBounds::default()
            )
            .is_err()
        );
        let row = task.delivery_history.as_ref().unwrap().records[0].clone();
        task.delivery_history.as_mut().unwrap().records.push(row);
        assert!(validate_task_record(&task).is_err());
        task.delivery_history.as_mut().unwrap().records.pop();
        task.last_delivery = None;
        assert!(validate_task_record(&task).is_err());
    }

    #[test]
    fn schema_defaults_status_without_rewriting_legacy_history() {
        let mut value = serde_json::to_value(task()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("status");
        object.remove("deliveryHistory");
        let decoded = validate_task(&value).unwrap();
        assert_eq!(decoded.status, TaskStatus::Active);
        assert_eq!(decoded.delivery_history, None);
        assert_eq!(
            history_page(&decoded, &request(0, None), &RetentionBounds::default())
                .unwrap_err()
                .code,
            "invalid_rule"
        );
    }
}
