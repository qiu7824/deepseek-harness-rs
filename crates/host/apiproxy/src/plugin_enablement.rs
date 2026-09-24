use super::*;
use dsh_host_plugin_inventory::PluginSetEnabledResult;

pub type PluginProgress = Arc<dyn Fn(&str) + Send + Sync>;
pub type PluginClientReady = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub struct PluginOperationControl {
    pub run: Arc<
        dyn Fn(
                String,
                bool,
                AbortSignal,
            ) -> BoxFuture<'static, Result<PluginSetEnabledResult, String>>
            + Send
            + Sync,
    >,
}
impl cordis::Service for PluginOperationControl {
    fn service_name(&self) -> &'static str {
        "pluginOperationControl"
    }
}

impl ApiProxyService {
    /// The operation owner keeps this future alive through rollback/commit;
    /// transport cancellation requests cancellation without abandoning mutation.
    pub async fn apply_plugin_enablement(
        &self,
        entry_id: String,
        enabled: bool,
        signal: AbortSignal,
        operation_id: Option<String>,
        progress: PluginProgress,
        client_ready: Option<PluginClientReady>,
    ) -> Result<PluginSetEnabledResult, String> {
        let loader = self
            .ctx
            .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or("loader service is not composed")?;
        progress("等待插件配置锁");
        let _mutation = tokio::select! {guard=self.defaults.plugin_mutation_lock.lock()=>guard,_=signal.cancelled()=>return Err("插件启停已取消，尚未修改运行状态".into())};
        if signal.aborted() {
            return Err("插件启停已取消，尚未修改运行状态".into());
        }
        let entry = loader
            .tree
            .resolve(&entry_id)
            .map_err(|error| error.to_string())?;
        let transaction = if let Some(path) = &self.defaults.plugins_document {
            let directory = path
                .parent()
                .ok_or("plugin configuration has no profile directory")?
                .to_owned();
            let id = entry_id.clone();
            Some(
                tokio::task::spawn_blocking(move || -> Result<_, String> {
                    let mut profile = dsh_app_boot::plugin_profile::Profile::open(&directory)?;
                    if let Some(operation_id) = operation_id {
                        profile.set_operation(dsh_app_boot::plugin_profile::OperationTag {
                            operation_id,
                            action: if enabled { "enable" } else { "disable" }.into(),
                            spec: id.clone(),
                        })?;
                    }
                    let mut documents = profile.documents()?;
                    if !set_plugin_document_enabled(&mut documents.entries, &id, enabled) {
                        return Err(format!(
                            "plugin entry {id:?} is absent from the current config"
                        ));
                    }
                    Ok((profile, documents))
                })
                .await
                .map_err(|error| error.to_string())??,
            )
        } else {
            None
        };
        if signal.aborted() {
            return Err("插件启停已取消，尚未修改运行状态".into());
        }
        let previous = entry.options.lock().clone();
        let mut patch = indexmap::IndexMap::new();
        patch.insert("disabled".into(), serde_json::Value::Bool(!enabled));
        progress("切换插件运行状态");
        if let Err(error) = entry.update(patch, false).await {
            progress("启停失败，恢复原运行状态");
            if let Err(rollback) = entry.replace_options(previous).await {
                return Err(format!(
                    "PLUGIN_RUNTIME_RECOVERY_REQUIRED: {error}; runtime rollback failed: {rollback}"
                ));
            }
            return Err(format!("plugin enablement failed: {error}"));
        }
        let ready = if signal.aborted() {
            Err("插件启停已取消".into())
        } else if let Some(ready) = client_ready {
            ready().await
        } else {
            Ok(())
        };
        if let Err(error) = ready.and_then(|()| {
            if signal.aborted() {
                Err("插件启停已取消".into())
            } else {
                Ok(())
            }
        }) {
            progress("恢复原插件运行状态");
            entry.replace_options(previous).await.map_err(|rollback| {
                format!(
                    "PLUGIN_RUNTIME_RECOVERY_REQUIRED: {error}; runtime rollback failed: {rollback}"
                )
            })?;
            return Err(error);
        }
        if let Some((profile, documents)) = transaction {
            progress("提交插件配置");
            if let Err(error) =
                tokio::task::spawn_blocking(move || profile.replace(documents, None))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            {
                progress("配置提交失败，恢复运行状态");
                entry.replace_options(previous).await.map_err(|rollback|format!("PLUGIN_RUNTIME_RECOVERY_REQUIRED: {error}; runtime rollback failed: {rollback}"))?;
                return Err(format!("plugin config persist failed: {error}"));
            }
        }
        progress("插件状态已提交");
        let inventory = self
            .ctx
            .get_typed::<Arc<dsh_host_plugin_inventory::PluginInventoryGateway>>(
                "pluginInventory",
                false,
            )
            .map(|slot| slot.as_ref().clone())
            .ok_or("plugin inventory service unavailable")?;
        let selected = inventory
            .list()
            .await
            .entries
            .into_iter()
            .find(|entry| entry.entry_id.as_str() == entry_id)
            .ok_or("updated plugin entry disappeared")?;
        Ok(PluginSetEnabledResult { entry: selected })
    }
}
