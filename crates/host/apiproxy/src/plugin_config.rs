//! Typed configuration edits over the same Profile document used at boot.
use super::*;
use dsh_app_boot::plugin_profile::{Documents, Profile};
use dsh_cordis_loader::{Entry, EntryOptions, LoaderService};
use dsh_host_plugin_inventory::{
    PluginConfigSnapshot, PluginGetConfigRequest, PluginSetConfigRequest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn supported(module: &str) -> Result<(), String> {
    match module {
        "dsh-time-context" | "@deepseek-ai/dsh-time-context" => Ok(()),
        _ => {
            Err("PLUGIN_CONFIG_UNSUPPORTED: this plugin has no exposed typed configuration".into())
        }
    }
}

fn find_entry<'a>(entries: &'a [Value], id: &str) -> Result<&'a Value, String> {
    fn collect<'a>(entries: &'a [Value], id: &str, found: &mut Vec<&'a Value>) {
        for row in entries {
            if row.get("id").and_then(Value::as_str) == Some(id) {
                found.push(row);
            }
            if row.get("group").and_then(Value::as_bool) == Some(true) {
                if let Some(children) = row.get("config").and_then(Value::as_array) {
                    collect(children, id, found);
                }
            }
        }
    }
    let mut found = Vec::new();
    collect(entries, id, &mut found);
    match found.as_slice() {
        [row] => Ok(row),
        [] => Err("PLUGIN_CONFIG_NOT_FOUND: entry is absent from the current Profile".into()),
        _ => Err("PLUGIN_CONFIG_AMBIGUOUS: duplicate Profile entry identity".into()),
    }
}

fn replace_config(entries: &mut [Value], id: &str, config: &Value) -> bool {
    for row in entries {
        if row.get("id").and_then(Value::as_str) == Some(id) {
            row.as_object_mut()
                .expect("validated Profile entry")
                .insert("config".into(), config.clone());
            return true;
        }
        if row.get("group").and_then(Value::as_bool) == Some(true) {
            if let Some(children) = row.get_mut("config").and_then(Value::as_array_mut) {
                if replace_config(children, id, config) {
                    return true;
                }
            }
        }
    }
    false
}

fn snapshot(row: &Value) -> Result<PluginConfigSnapshot, String> {
    let entry_id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or("invalid plugin entry identity")?;
    let module_name = row
        .get("name")
        .and_then(Value::as_str)
        .ok_or("invalid plugin module")?;
    supported(module_name)?;
    let config = row.get("config").cloned();
    // Presence bits distinguish omitted defaults from explicit null values.
    // Enablement participates in CAS although this RPC never changes it.
    let identity = json!([
        entry_id,
        module_name,
        config.is_some(),
        config,
        row.get("disabled").is_some(),
        row.get("disabled")
    ]);
    let revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity).map_err(|e| e.to_string())?)
    );
    Ok(PluginConfigSnapshot {
        entry_id: entry_id.into(),
        module_name: module_name.into(),
        config,
        revision,
    })
}

fn check_runtime(row: &Value, entry: &Arc<Entry>) -> Result<EntryOptions, String> {
    let stored: EntryOptions = serde_json::from_value(row.clone()).map_err(|e| e.to_string())?;
    supported(&stored.name)?;
    if stored.group == Some(true) {
        return Err("PLUGIN_CONFIG_UNSUPPORTED: group configuration is not editable".into());
    }
    if row.get("disabled").is_some_and(|value| !value.is_boolean()) {
        return Err(
            "PLUGIN_CONFIG_UNSUPPORTED: conditional or invalid enablement is not editable".into(),
        );
    }
    let runtime = entry.options.lock().clone();
    if stored != runtime {
        return Err("PLUGIN_CONFIG_RUNTIME_CONFLICT: Profile and running entry differ; reload their configuration before saving".into());
    }
    Ok(runtime)
}

fn loader(ctx: &Context) -> Result<Arc<LoaderService>, String> {
    ctx.get_typed::<Arc<LoaderService>>("loader", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "loader service is not composed".into())
}

async fn open_profile(defaults: &ApiProxyDefaults) -> Result<(Profile, Documents), String> {
    let directory = defaults
        .plugins_document
        .as_ref()
        .and_then(|path| path.parent())
        .ok_or("PLUGIN_CONFIG_UNAVAILABLE: no writable Profile is composed")?
        .to_owned();
    tokio::task::spawn_blocking(move || {
        let profile = Profile::open(&directory)?;
        let documents = profile.documents()?;
        Ok((profile, documents))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn rollback(
    entry: &Arc<Entry>,
    previous: EntryOptions,
    error: String,
) -> Result<PluginConfigSnapshot, String> {
    entry.replace_options(previous).await.map_err(|rollback| {
        format!("PLUGIN_RUNTIME_RECOVERY_REQUIRED: {error}; runtime rollback failed: {rollback}")
    })?;
    Err(error)
}

async fn write_config(
    ctx: Context,
    defaults: Arc<ApiProxyDefaults>,
    request: PluginSetConfigRequest,
    signal: AbortSignal,
) -> Result<PluginConfigSnapshot, String> {
    let _mutation = tokio::select! {
        guard = defaults.plugin_mutation_lock.lock() => guard,
        _ = signal.cancelled() => return Err("PLUGIN_CONFIG_CANCELLED: configuration was not changed".into()),
    };
    if signal.aborted() {
        return Err("PLUGIN_CONFIG_CANCELLED: configuration was not changed".into());
    }
    let loader = loader(&ctx)?;
    let (profile, mut documents) = open_profile(&defaults).await?;
    let row = find_entry(&documents.entries, &request.entry_id)?;
    let current = snapshot(row)?;
    if current.revision != request.expected_revision {
        return Err(
            "PLUGIN_CONFIG_CONFLICT: configuration changed; read it again before saving".into(),
        );
    }
    let entry = loader
        .tree
        .resolve(&request.entry_id)
        .map_err(|e| e.to_string())?;
    let previous = check_runtime(row, &entry)?;
    if !request.config.is_object() {
        return Err("PLUGIN_CONFIG_INVALID: config must be an object".into());
    }
    // Validate disabled entries as strictly as live ones; Loader does not load
    // a disabled fiber and therefore cannot perform this check on our behalf.
    dsh_time_context::decode_config(&request.config)
        .map_err(|error| format!("PLUGIN_CONFIG_INVALID: {error}"))?;
    if signal.aborted() {
        return Err("PLUGIN_CONFIG_CANCELLED: configuration was not changed".into());
    }
    if current.config.as_ref() == Some(&request.config) {
        return Ok(current);
    }
    assert!(replace_config(
        &mut documents.entries,
        &request.entry_id,
        &request.config
    ));
    let next = snapshot(find_entry(&documents.entries, &request.entry_id)?)?;
    let mut next_options = previous.clone();
    next_options.config = Some(request.config);
    // Replace this plugin's complete Fiber so pending callbacks from the old
    // configuration are revoked before the new listener becomes active. Keep
    // every other option, including disabled, unchanged; Agents are untouched.
    if let Err(error) = entry.replace_options(next_options).await {
        return rollback(
            &entry,
            previous,
            format!("PLUGIN_CONFIG_APPLY_FAILED: {error}"),
        )
        .await;
    }
    if signal.aborted() {
        return rollback(
            &entry,
            previous,
            "PLUGIN_CONFIG_CANCELLED: previous configuration restored".into(),
        )
        .await;
    }
    // Keep the cross-process Profile lock through any runtime rollback. Once
    // commit starts it runs to completion even if the requesting carrier drops.
    let persisted = tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if signal.aborted() {
                Err("PLUGIN_CONFIG_CANCELLED: previous configuration restored".into())
            } else {
                profile.replace(documents, None)
            }
        }))
        .unwrap_or_else(|_| Err("Profile commit panicked; recovery may be required".into()));
        (profile, result)
    })
    .await;
    let (profile, committed) = match persisted {
        Ok(result) => result,
        Err(error) => {
            return rollback(
                &entry,
                previous,
                format!("PLUGIN_CONFIG_COMMIT_FAILED: {error}"),
            )
            .await;
        }
    };
    let result = match committed {
        Ok(()) => Ok(next),
        Err(error) if error.starts_with("PLUGIN_CONFIG_CANCELLED:") => {
            rollback(&entry, previous, error).await
        }
        Err(error) => {
            rollback(
                &entry,
                previous,
                format!("PLUGIN_CONFIG_COMMIT_FAILED: {error}"),
            )
            .await
        }
    };
    drop(profile);
    result
}

struct CancelOnDrop(Option<AbortSignal>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(signal) = self.0.take() {
            signal.abort();
        }
    }
}

fn config_error(message: String) -> RpcError {
    let kind = message
        .split_once(':')
        .map(|(kind, _)| kind)
        .unwrap_or("")
        .to_owned();
    let body = RpcErrorBody {
        message,
        details: EmptyDetails {},
    };
    match kind.as_str() {
        "PLUGIN_CONFIG_UNSUPPORTED" => RpcError::PluginConfigUnsupported(body),
        "PLUGIN_CONFIG_NOT_FOUND" => RpcError::PluginConfigNotFound(body),
        "PLUGIN_CONFIG_AMBIGUOUS" => RpcError::PluginConfigAmbiguous(body),
        "PLUGIN_CONFIG_RUNTIME_CONFLICT" => RpcError::PluginConfigRuntimeConflict(body),
        "PLUGIN_CONFIG_UNAVAILABLE" => RpcError::PluginConfigUnavailable(body),
        "PLUGIN_CONFIG_CANCELLED" => RpcError::PluginConfigCancelled(body),
        "PLUGIN_CONFIG_CONFLICT" => RpcError::PluginConfigConflict(body),
        "PLUGIN_CONFIG_INVALID" => RpcError::PluginConfigInvalid(body),
        "PLUGIN_CONFIG_APPLY_FAILED" => RpcError::PluginConfigApplyFailed(body),
        "PLUGIN_CONFIG_COMMIT_FAILED" => RpcError::PluginConfigCommitFailed(body),
        "PLUGIN_RUNTIME_RECOVERY_REQUIRED" => RpcError::PluginConfigRecoveryRequired(body),
        _ => RpcError::Internal(body),
    }
}

fn response(rpc_id: RpcId, result: Result<PluginConfigSnapshot, String>) -> RpcResponse<Value> {
    match result {
        Ok(value) => ok(rpc_id, value),
        Err(message) => err(rpc_id, config_error(message)),
    }
}

impl ApiProxyService {
    pub(super) async fn plugin_inventory_get_config(
        &self,
        request: RpcRequest<PluginGetConfigRequest>,
    ) -> RpcResponse<Value> {
        let result = async {
            let _mutation = self.defaults.plugin_mutation_lock.lock().await;
            let loader = loader(&self.ctx)?;
            let (_profile, documents) = open_profile(&self.defaults).await?;
            let row = find_entry(&documents.entries, &request.payload.entry_id)?;
            let current = snapshot(row)?;
            let entry = loader
                .tree
                .resolve(&request.payload.entry_id)
                .map_err(|e| e.to_string())?;
            check_runtime(row, &entry)?;
            Ok(current)
        }
        .await;
        response(request.rpc_id, result)
    }

    pub(super) async fn plugin_inventory_set_config(
        &self,
        request: RpcRequest<PluginSetConfigRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<Value> {
        // The mutation owner survives transport cancellation and completes its
        // rollback/commit; dropping this waiter requests cancellation only.
        let mut guard = CancelOnDrop(Some(signal.clone()));
        let task = tokio::spawn(write_config(
            self.ctx.clone(),
            self.defaults.clone(),
            request.payload,
            signal,
        ));
        let result = task
            .await
            .unwrap_or_else(|e| Err(format!("PLUGIN_CONFIG_OPERATION_FAILED: {e}")));
        guard.0 = None;
        response(request.rpc_id, result)
    }
}

#[cfg(test)]
#[path = "plugin_config_tests.rs"]
mod tests;
