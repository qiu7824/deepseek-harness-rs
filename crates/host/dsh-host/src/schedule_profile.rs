//! Hydrate optional reminder settings without loading its task database.
use dsh_schedule::host_service::ScheduleService;
use serde_json::Value;

pub(crate) fn configure(service: &ScheduleService, entries: &[Value]) -> Result<(), String> {
    fn collect<'a>(entries: &'a [Value], rows: &mut Vec<&'a Value>) {
        for row in entries {
            if matches!(
                row["name"].as_str(),
                Some("dsh-schedule" | "@deepseek-ai/dsh-schedule")
            ) {
                rows.push(row);
            }
            if row["group"] == true {
                if let Some(children) = row["config"].as_array() {
                    collect(children, rows);
                }
            }
        }
    }
    let mut rows = Vec::new();
    collect(entries, &mut rows);
    let row = match rows.as_slice() {
        [] => return Ok(()),
        [row] => row,
        _ => return Err("multiple reminder entries have ambiguous Host retention settings".into()),
    };
    let config = dsh_schedule::host_plugin::decode_host_schedule_config(
        &row.get("config")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|error| {
        format!(
            "entry {} has invalid reminder configuration: {error}",
            row["id"]
        )
    })?;
    service.configure(config).map_err(|error| {
        format!(
            "entry {} has invalid reminder configuration: {error}",
            row["id"]
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    struct NoSessions;
    #[async_trait::async_trait]
    impl dsh_schedule::host_service::ScheduleSessionController for NoSessions {
        async fn acquire(
            &self,
            _: &str,
            _: bool,
        ) -> Result<
            Box<dyn dsh_schedule::host_service::ScheduleSessionLease>,
            dsh_schedule::host_types::ScheduleError,
        > {
            panic!("reading disabled retention must not acquire a session")
        }
    }

    #[tokio::test]
    async fn disabled_profile_settings_do_not_open_storage_and_invalid_values_are_retained_as_errors()
     {
        let ctx = cordis::Context::root();
        // No storage or session services exist in this fixture.
        let service = ScheduleService::install(&ctx, Arc::new(NoSessions));
        let entries = serde_json::json!([{"id":"tools","group":true,"config":[{
            "id":"custom-reminders","name":"@deepseek-ai/dsh-schedule","disabled":true,
            "config":{"deliveryHistoryDays":7.0,"deliveryHistoryRecords":2.0}
        }]}]);
        configure(&service, entries.as_array().unwrap()).unwrap();
        assert!(!service.enabled());
        assert_eq!(service.config().delivery_history_days, 7);
        assert_eq!(service.config().delivery_history_records, 2);
        for config in [
            serde_json::json!([]),
            serde_json::json!(null),
            serde_json::json!({"deliveryHistoryRecords":0}),
        ] {
            let bad = serde_json::json!([{"id":"bad","name":"dsh-schedule","disabled":true,"config":config}]);
            let original = bad.clone();
            assert!(
                configure(&service, bad.as_array().unwrap())
                    .unwrap_err()
                    .contains("invalid reminder configuration")
            );
            assert_eq!(bad, original);
        }
        assert!(!service.enabled());
        assert_eq!(service.config().delivery_history_records, 2);
        service.shutdown().await;
        ctx.fiber.dispose().await;
    }
}
