//! Explicit target selection: local computer by default, bound UU device only
//! when remote was requested. Selection does not connect or probe other targets.
use crate::{AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, ComputerUseAdapter};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct TargetRouter {
    local: Arc<dyn ComputerUseAdapter>,
    remote: Arc<dyn ComputerUseAdapter>,
    browser: Arc<dyn ComputerUseAdapter>,
}
impl TargetRouter {
    pub fn new(
        local: Arc<dyn ComputerUseAdapter>,
        remote: Arc<dyn ComputerUseAdapter>,
        browser: Arc<dyn ComputerUseAdapter>,
    ) -> Self {
        Self {
            local,
            remote,
            browser,
        }
    }
    fn selected(&self, arguments: &Value) -> Result<&Arc<dyn ComputerUseAdapter>, AdapterError> {
        match arguments.get("target") {
            None => Ok(&self.local),
            Some(Value::String(target)) => match target.as_str() {
                "local" => Ok(&self.local),
                "remote" => Ok(&self.remote),
                "browser" => Ok(&self.browser),
                _ => Err(AdapterError::new(
                    "COMPUTER_USE_TARGET",
                    "target must be local, remote or browser",
                )),
            },
            _ => Err(AdapterError::new(
                "COMPUTER_USE_TARGET",
                "target must be a string",
            )),
        }
    }
    fn all(&self) -> [&Arc<dyn ComputerUseAdapter>; 3] {
        [&self.local, &self.remote, &self.browser]
    }
}
#[async_trait]
impl ComputerUseAdapter for TargetRouter {
    fn adapter_id(&self) -> &'static str {
        self.local.adapter_id()
    }
    fn adapter_id_for(&self, arguments: &Value) -> Result<&'static str, AdapterError> {
        Ok(self.selected(arguments)?.adapter_id())
    }
    fn targets(&self) -> Value {
        json!([{"id":"local","adapter":self.local.adapter_id(),"default":true},{"id":"remote","adapter":self.remote.adapter_id(),"requiresBinding":true},{"id":"browser","adapter":self.browser.adapter_id()}])
    }
    fn availability(&self) -> Result<(), AdapterError> {
        self.local.availability()
    }
    fn availability_for(&self, arguments: &Value) -> Result<(), AdapterError> {
        self.selected(arguments)?.availability()
    }
    fn control_scope(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        self.selected(&request.arguments)?.control_scope(request)
    }
    async fn change_control(
        &self,
        request: &AdapterRequest,
        manual: bool,
    ) -> Result<(), AdapterError> {
        self.selected(&request.arguments)?
            .change_control(request, manual)
            .await
    }
    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        let adapter = self.selected(&request.arguments)?.clone();
        let mut output = adapter.execute(request, signal).await?;
        if let Some(object) = output.value.as_object_mut() {
            object.insert("adapter".into(), json!(adapter.adapter_id()));
        }
        Ok(output)
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        let mut error = None;
        for adapter in self.all() {
            if let Err(e) = adapter.shutdown().await {
                error.get_or_insert(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
    async fn close_owner(&self, owner: &str) -> Result<(), AdapterError> {
        let mut error = None;
        for adapter in self.all() {
            if let Err(e) = adapter.close_owner(owner).await {
                error.get_or_insert(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
    fn has_owner_activity(&self, owner: &str) -> bool {
        self.all()
            .iter()
            .any(|adapter| adapter.has_owner_activity(owner))
    }
    fn mark_owner_active(&self, owner: &str) {
        for adapter in self.all() {
            adapter.mark_owner_active(owner);
        }
    }
    async fn reap_inactive(&self) -> Vec<String> {
        let mut owners = Vec::new();
        for adapter in self.all() {
            owners.extend(adapter.reap_inactive().await);
        }
        owners.sort();
        owners.dedup();
        owners.retain(|owner| !self.has_owner_activity(owner));
        owners
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Recording {
        id: &'static str,
        calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }
    #[async_trait]
    impl ComputerUseAdapter for Recording {
        fn adapter_id(&self) -> &'static str {
            self.id
        }
        async fn execute(
            &self,
            _: AdapterRequest,
            _: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            self.calls.lock().unwrap().push(self.id);
            Ok(AdapterOutput::json(json!({})))
        }
    }
    #[tokio::test]
    async fn local_never_implicitly_contacts_remote() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let adapter = |id| {
            Arc::new(Recording {
                id,
                calls: calls.clone(),
            }) as Arc<dyn ComputerUseAdapter>
        };
        let router = TargetRouter::new(
            adapter("native-desktop"),
            adapter("uu-desktop"),
            adapter("native-browser"),
        );
        for args in [
            json!({"action":"start"}),
            json!({"action":"start","target":"browser"}),
            json!({"action":"start","target":"remote"}),
        ] {
            router
                .execute(
                    AdapterRequest::from_arguments(&args).unwrap(),
                    Arc::new(|| false),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["native-desktop", "native-browser", "uu-desktop"]
        );
        assert!(
            router
                .adapter_id_for(&json!({"target":"arbitrary-device"}))
                .is_err()
        );
        assert_eq!(calls.lock().unwrap().len(), 3);
    }
}
