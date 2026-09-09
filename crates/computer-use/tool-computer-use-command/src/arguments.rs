//! Shared input normalization for the built-in controllers.
use std::borrow::Cow;

use serde_json::Value;

use crate::AdapterError;

pub(super) fn normalize<'a>(
    adapter: &str,
    arguments: &'a Value,
) -> Result<Cow<'a, Value>, AdapterError> {
    // External commands define their own argument protocol.
    if !matches!(adapter, "native-browser" | "native-desktop" | "uu-desktop")
        || arguments
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            != Some("scroll")
    {
        return Ok(Cow::Borrowed(arguments));
    }
    let invalid = |message: String| AdapterError::new("COMPUTER_USE_INVALID_ARGUMENT", message);
    let limit = if adapter == "native-browser" {
        100_000.0
    } else {
        10_000.0
    };
    let mut result = Cow::Borrowed(arguments);
    let mut supplied = false;
    for (canonical, alias) in [("deltaX", "scrollX"), ("deltaY", "scrollY")] {
        let read = |key: &str| -> Result<Option<f64>, AdapterError> {
            arguments
                .get(key)
                .map(|value| {
                    value
                        .as_f64()
                        .filter(|value| value.is_finite())
                        .ok_or_else(|| invalid(format!("{key} must be a finite number")))
                })
                .transpose()
        };
        let primary = read(canonical)?;
        let alternate = read(alias)?;
        for (key, value) in [(canonical, primary), (alias, alternate)] {
            if value.is_some_and(|value| value.abs() > limit) {
                return Err(invalid(format!(
                    "{key} must be between -{limit} and {limit}"
                )));
            }
        }
        supplied |= primary.is_some() || alternate.is_some();
        if let (Some(primary), Some(alternate)) = (primary, alternate)
            && primary != alternate
        {
            return Err(invalid(format!(
                "{canonical} and {alias} must not conflict"
            )));
        }
        if alternate.is_some() {
            let object = result
                .to_mut()
                .as_object_mut()
                .expect("scroll arguments object");
            let value = object.remove(alias).expect("validated scroll alias");
            object.entry(canonical.to_string()).or_insert(value);
        }
    }
    if !supplied {
        return Err(invalid(
            "scroll requires deltaX or deltaY (scrollX/scrollY are accepted aliases)".into(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cordis::Context;
    use parking_lot::Mutex;
    use serde_json::json;

    use super::*;
    use crate::{
        AbortPredicate, AdapterOutput, AdapterRequest, ComputerUseAdapter, ComputerUseRuntime,
    };

    struct RecordingAdapter {
        id: &'static str,
        calls: Arc<Mutex<Vec<Value>>>,
    }

    #[async_trait::async_trait]
    impl ComputerUseAdapter for RecordingAdapter {
        fn adapter_id(&self) -> &'static str {
            self.id
        }

        async fn execute(
            &self,
            request: AdapterRequest,
            _signal: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            self.calls.lock().push(request.arguments);
            Ok(AdapterOutput::json(json!({"ok": true})))
        }
    }

    fn runtime(id: &'static str, calls: Arc<Mutex<Vec<Value>>>) -> ComputerUseRuntime {
        ComputerUseRuntime {
            ctx: Context::root(),
            adapter: Arc::new(RecordingAdapter { id, calls }),
            timeout: std::time::Duration::from_secs(1),
            owner_agents: Mutex::default(),
        }
    }

    #[tokio::test]
    async fn real_runtime_forwards_signed_scroll_aliases_for_agent_and_gui() {
        for id in ["native-browser", "native-desktop", "uu-desktop"] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let runtime = runtime(id, calls.clone());
            runtime
                .execute(
                    "test",
                    &json!({"action":"scroll", "x":12, "y":24, "scrollY":-600}),
                    Arc::new(|| false),
                )
                .await
                .unwrap();
            runtime
                .execute_for_human_session(
                    "test".into(),
                    &json!({"action":"scroll", "deltaY":600, "scrollX":-120}),
                    Arc::new(|| false),
                )
                .await
                .unwrap();
            let calls = calls.lock();
            assert_eq!(
                calls[0],
                json!({"action":"scroll", "x":12, "y":24, "deltaY":-600})
            );
            assert_eq!(
                calls[1],
                json!({"action":"scroll", "deltaY":600, "deltaX":-120})
            );
        }
    }

    #[tokio::test]
    async fn invalid_scroll_never_reaches_a_builtin_controller() {
        for (id, limit) in [
            ("native-desktop", 10_000),
            ("uu-desktop", 10_000),
            ("native-browser", 100_000),
        ] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let runtime = runtime(id, calls.clone());
            for args in [
                json!({"action":"scroll", "scrollAmount":600}),
                json!({"action":"scroll", "deltaY":0, "scrollY":600}),
                json!({"action":"scroll", "scrollY":"-600"}),
                json!({"action":"scroll", "deltaY":null}),
                json!({"action":"scroll", "x":12, "y":24, "deltaY":limit+1}),
                json!({"action":"scroll", "x":12, "y":24, "scrollX":-limit-1}),
            ] {
                let result = runtime.execute("test", &args, Arc::new(|| false)).await;
                assert_eq!(result.unwrap_err().code, "COMPUTER_USE_INVALID_ARGUMENT");
            }
            assert!(calls.lock().is_empty());
        }
    }

    #[test]
    fn matching_aliases_and_external_protocols_are_preserved() {
        assert_eq!(
            normalize(
                "uu-desktop",
                &json!({"action":"scroll", "deltaY":-120, "scrollY":-120})
            )
            .unwrap()
            .as_ref(),
            &json!({"action":"scroll", "deltaY":-120})
        );
        let custom = json!({"action":"scroll", "scrollAmount":3});
        assert_eq!(normalize("command", &custom).unwrap().as_ref(), &custom);
        assert_eq!(
            normalize("native-browser", &json!({"action":"scroll", "deltaY":0}))
                .unwrap()
                .as_ref(),
            &json!({"action":"scroll", "deltaY":0})
        );
    }
}
