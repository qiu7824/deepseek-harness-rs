use super::*;

#[derive(Default)]
struct MemorySettings {
    fail_writes: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl dsh_settings::SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        true
    }
    async fn load(&self) -> Result<indexmap::IndexMap<String, dsh_schemastery::Data>, String> {
        Ok(Default::default())
    }
    async fn persist(
        &self,
        _: &dsh_settings::SettingsNamespace,
        _: dsh_schemastery::Data,
    ) -> Result<(), String> {
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            Err("fixture settings disk unavailable".into())
        } else {
            Ok(())
        }
    }
}
async fn setup() -> (Arc<AccountAuth>, cordis::Context, std::path::PathBuf) {
    setup_with_storage(Arc::new(MemorySettings::default())).await
}
async fn setup_with_storage(
    storage: Arc<MemorySettings>,
) -> (Arc<AccountAuth>, cordis::Context, std::path::PathBuf) {
    let ctx = cordis::Context::root();
    let root = std::env::temp_dir().join(format!("dsh-model-catalog-{}", uuid::Uuid::new_v4()));
    let credentials = dsh_credentials_local::LocalCredentialProvider::install(
        &ctx,
        dsh_credentials_local::Config {
            dsh_home: Some(root.to_string_lossy().into()),
            watch: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    let settings = dsh_settings::SettingsProvider::install(&ctx, storage);
    settings.ready().await.unwrap();
    settings
        .register(
            &ctx,
            dsh_settings::settings_namespace("llm-pi-ai").unwrap(),
            crate::openai_compatible_schema(),
            dsh_settings::SettingsRegisterOptions {
                validate: Some(Arc::new(|data| crate::openai_profiles(data).map(|_| ()))),
                ..Default::default()
            },
        )
        .unwrap();
    settings
        .register(
            &ctx,
            dsh_settings::settings_namespace("llm-deepseek").unwrap(),
            crate::deepseek_settings::schema(),
            Default::default(),
        )
        .unwrap();
    (AccountAuth::new(credentials, settings).unwrap(), ctx, root)
}
fn tokens(account: &str) -> Session {
    tokens_for_subject(account, &format!("subject-{account}"))
}
fn tokens_for_subject(account: &str, subject: &str) -> Session {
    use base64::Engine;
    let payload = json!({"https://api.openai.com/auth":{"chatgpt_account_id":account},"sub":subject,"exp":now()+3600});
    let token = format!(
        "header.{}.fixture",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap())
    );
    Session::from_tokens(
        &json!({"access_token":token,"refresh_token":"fixture-refresh","expires_in":3600}),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn shared_workspace_logins_keep_distinct_identity_and_migrate_legacy_without_duplicates() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let alice = tokens_for_subject("shared-workspace", "alice");
    let bob = tokens_for_subject("shared-workspace", "bob");
    assert_ne!(alice.account_scope, bob.account_scope);
    let mut legacy = alice.clone();
    legacy.account_scope = format!(
        "account-{}",
        crate::provider_auth_catalog::key("shared-workspace")
    );
    auth.credentials
        .set(&reference(p.id), &serde_json::to_string(&legacy).unwrap())
        .await
        .unwrap();
    // The legacy single-account directory may already contain the same login.
    auth.write_accounts(p.id, &[legacy.clone()]).await.unwrap();
    auth.install_profile(p, &legacy).await.unwrap();
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns, json!({"providers":{"openai-codex":{"modelPreferences":{legacy.account_scope.clone():{"test-model":{"enabled":false}}}}}}), None).await.unwrap();
    auth.activate(p, &bob).await.unwrap();
    let saved = auth.saved_sessions(p.id).await.unwrap();
    assert_eq!(saved.len(), 2);
    assert!(
        saved
            .iter()
            .any(|item| item.account_scope == alice.account_scope)
    );
    assert!(
        saved
            .iter()
            .any(|item| item.account_scope == bob.account_scope)
    );
    let profile = auth.profile_snapshot(p.id, false).unwrap().0;
    assert_eq!(
        profile["modelPreferences"][&alice.account_scope]["test-model"]["enabled"],
        false
    );
    assert!(
        profile["modelPreferences"]
            .get(&bob.account_scope)
            .is_none()
    );
    let headers = vec![("ChatGPT-Account-ID".into(), "shared-workspace".into())];
    // Identical routing headers do not permit an old account snapshot to use Bob's token.
    let result = auth
        .resolve_request_token_for_scope(p.id, p.base, &headers, Some(&alice.account_scope))
        .await;
    assert!(result.unwrap_err().contains("配置已更新"));
    auth.handle(
        "switch",
        &json!({"provider":p.id,"accountScope":alice.account_scope}),
    )
    .await
    .unwrap();
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().access_token,
        alice.access_token
    );
    let renewed = Session::from_tokens(
        &json!({"access_token":"opaque-refreshed","expires_in":3600}),
        Some(&alice),
    )
    .unwrap();
    assert_eq!(renewed.account_scope, alice.account_scope);
    auth.save(p.id, &renewed).await.unwrap();
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        alice.account_scope
    );
    assert_eq!(auth.saved_sessions(p.id).await.unwrap().len(), 2);
    clean(auth, root).await;
}

#[tokio::test]
async fn legacy_active_scope_is_repaired_without_rejecting_the_first_request() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let current = tokens_for_subject("workspace", "alice");
    let mut legacy = current.clone();
    legacy.account_scope = format!("account-{}", crate::provider_auth_catalog::key("workspace"));
    auth.credentials
        .set(&reference(p.id), &serde_json::to_string(&legacy).unwrap())
        .await
        .unwrap();
    auth.install_profile(p, &legacy).await.unwrap();
    let headers = vec![("ChatGPT-Account-ID".into(), "workspace".into())];
    let token = auth
        .resolve_request_token_for_scope(p.id, p.base, &headers, Some(&legacy.account_scope))
        .await
        .unwrap();
    assert_eq!(token, Some(current.access_token));
    assert_eq!(
        auth.profile_snapshot(p.id, false).unwrap().0["modelCatalogScope"],
        current.account_scope
    );
    clean(auth, root).await;
}

fn cached_account_catalog(provider: &str, scope: &str) -> crate::provider_auth_catalog::Catalog {
    serde_json::from_value(json!({
        "provider":provider,"accountScope":scope,"status":"synced","source":"remote",
        "endpoint":"https://chatgpt.com/backend-api/codex/models","updatedAt":1234,"error":null,
        "models":[{"id":"gpt-6-astra","name":"GPT-6-Astra","description":"Fixture model",
            "api":"openai-responses","available":true,"contextWindow":200000,"maxTokens":32000,
            "input":["text","image"],"reasoningEfforts":{"high":"high"},"reasoningDefault":"high",
            "supportsReasoningSummaries":true,"supportedParameters":["reasoning_effort"]}]
    }))
    .unwrap()
}

#[tokio::test]
async fn legacy_login_catalog_survives_scope_migration_and_new_refresh_wins() {
    for missing_scope in [false, true] {
        let (auth, _ctx, root) = setup().await;
        let p = provider("openai-codex").unwrap();
        let current = tokens_for_subject("workspace", "alice");
        let legacy_scope = format!("account-{}", crate::provider_auth_catalog::key("workspace"));
        let mut legacy = current.clone();
        legacy.account_scope = if missing_scope {
            String::new()
        } else {
            legacy_scope.clone()
        };
        let catalog = cached_account_catalog(p.id, &legacy_scope);
        auth.catalogs.store(catalog.clone()).await.unwrap();
        auth.credentials
            .set(&reference(p.id), &serde_json::to_string(&legacy).unwrap())
            .await
            .unwrap();
        auth.install_profile(p, &legacy).await.unwrap();
        let view = auth.model_view(p.id).await.unwrap();
        assert_eq!(view["catalog"]["count"], 1);
        assert_eq!(view["models"][0]["name"], "GPT-6-Astra");
        assert_eq!(view["models"][0]["enabled"], true);
        assert_eq!(view["models"][0]["availability"], "available");
        let migrated = auth.catalogs.get(p.id, &current.account_scope);
        assert_eq!(migrated.models, catalog.models);
        assert_eq!(migrated.updated_at, catalog.updated_at);
        assert_eq!(migrated.endpoint, catalog.endpoint);
        let reopened =
            crate::provider_auth_catalog::CatalogStore::new(root.join("cache/model-catalogs"));
        assert_eq!(
            reopened.get(p.id, &current.account_scope).models,
            catalog.models
        );
        let mut fresh = migrated;
        fresh.models[0].name = Some("Updated remote name".into());
        fresh.updated_at = Some(5678);
        auth.catalogs.store(fresh.clone()).await.unwrap();
        // The raw credential can remain in the old format until its next refresh.
        auth.session(p.id).await.unwrap();
        assert_eq!(
            auth.catalogs.get(p.id, &current.account_scope).models,
            fresh.models
        );
        clean(auth, root).await;
    }
}

#[tokio::test]
async fn saved_legacy_catalog_migrates_only_to_its_original_login() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let alice = tokens_for_subject("workspace", "alice");
    let bob = tokens_for_subject("workspace", "bob");
    let mut legacy = alice.clone();
    legacy.account_scope = format!("account-{}", crate::provider_auth_catalog::key("workspace"));
    let catalog = cached_account_catalog(p.id, &legacy.account_scope);
    auth.catalogs.store(catalog.clone()).await.unwrap();
    auth.credentials
        .set(&reference(p.id), &serde_json::to_string(&bob).unwrap())
        .await
        .unwrap();
    auth.write_accounts(p.id, &[legacy]).await.unwrap();
    auth.install_profile(p, &bob).await.unwrap();
    assert!(
        auth.model_view(p.id).await.unwrap()["models"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    auth.handle(
        "switch",
        &json!({"provider":p.id,"accountScope":alice.account_scope}),
    )
    .await
    .unwrap();
    assert_eq!(
        auth.model_view(p.id).await.unwrap()["models"][0]["enabled"],
        true
    );
    assert_eq!(
        auth.catalogs.get(p.id, &alice.account_scope).models,
        catalog.models
    );
    assert!(
        auth.catalogs
            .get(p.id, &bob.account_scope)
            .models
            .is_empty()
    );
    clean(auth, root).await;
}
async fn clean(auth: Arc<AccountAuth>, root: std::path::PathBuf) {
    auth.credentials.drain().await;
    drop(auth);
    if root.exists() {
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}

#[test]
fn account_claim_is_a_literal_url_key_and_refresh_keeps_identity() {
    let a = tokens("account-a");
    assert_eq!(a.account_id.as_deref(), Some("account-a"));
    assert_ne!(a.account_scope, tokens("account-b").account_scope);
    let refreshed = Session::from_tokens(
        &json!({"access_token":"opaque-next","expires_in":3600}),
        Some(&a),
    )
    .unwrap();
    assert_eq!(a.account_scope, refreshed.account_scope);
    use base64::Engine;
    let without_identity = format!(
        "header.{}.fixture",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!({"exp":now()+7200}).to_string())
    );
    let refreshed =
        Session::from_tokens(&json!({"access_token":without_identity}), Some(&a)).unwrap();
    assert_eq!(a.account_scope, refreshed.account_scope);
}

#[tokio::test]
async fn multiple_logins_preserve_legacy_account_switch_identity_and_remove_credentials() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let a = tokens("account-a");
    let b = tokens("account-b");
    // Upgrade from the single-account format without an ACCOUNTS credential.
    auth.credentials
        .set(&reference(p.id), &serde_json::to_string(&a).unwrap())
        .await
        .unwrap();
    auth.save(p.id, &b).await.unwrap();
    auth.install_profile(p, &b).await.unwrap();
    assert_eq!(auth.saved_sessions(p.id).await.unwrap().len(), 2);
    let body = json!({"provider":p.id,"accountScope":a.account_scope});
    let switched = auth.handle("switch", &body).await.unwrap();
    assert_eq!(switched["status"], "switched");
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().access_token,
        a.access_token
    );
    let profile = auth.profile_snapshot(p.id, false).unwrap().0;
    assert_eq!(profile["modelCatalogScope"], a.account_scope);
    assert_eq!(profile["headers"]["ChatGPT-Account-ID"], "account-a");
    assert_eq!(auth.catalogs.scope(p.id), Some(a.account_scope.clone()));
    let directory = auth.handle("providers", &json!({})).await.unwrap();
    let codex = directory["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == p.id)
        .unwrap();
    assert_eq!(codex["accountCount"], 2);
    assert!(!directory.to_string().contains(&a.access_token));
    assert!(!directory.to_string().contains("fixture-refresh"));

    // Removing a saved account leaves the current account usable and cannot be undone by switch.
    auth.handle(
        "logout",
        &json!({"provider":p.id,"accountScope":b.account_scope}),
    )
    .await
    .unwrap();
    assert!(
        auth.handle(
            "switch",
            &json!({"provider":p.id,"accountScope":b.account_scope})
        )
        .await
        .is_err()
    );
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        a.account_scope
    );
    assert_eq!(auth.saved_sessions(p.id).await.unwrap().len(), 1);
    auth.save(p.id, &b).await.unwrap();
    auth.handle("logout", &json!({"provider":p.id}))
        .await
        .unwrap();
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        a.account_scope
    );
    auth.handle("logout", &json!({"provider":p.id}))
        .await
        .unwrap();
    assert!(auth.session(p.id).await.unwrap().is_none());
    assert!(
        auth.credentials
            .resolve(&accounts_reference(p.id))
            .await
            .is_none()
    );
    let directory = auth.handle("providers", &json!({})).await.unwrap();
    let codex = directory["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == p.id)
        .unwrap();
    assert_eq!(codex["signedIn"], false);
    assert_eq!(codex["accountCount"], 0);
    clean(auth, root).await;
}

#[tokio::test]
async fn switch_waits_for_refresh_before_reading_saved_tokens() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let mut a = tokens("account-a");
    auth.save(p.id, &a).await.unwrap();
    auth.install_profile(p, &a).await.unwrap();
    let guard = auth.refresh.lock().await;
    let actor = auth.clone();
    let body = json!({"provider":p.id,"accountScope":a.account_scope});
    let (started, ready) = tokio::sync::oneshot::channel();
    let switch = tokio::spawn(async move {
        started.send(()).unwrap();
        actor.handle("switch", &body).await
    });
    ready.await.unwrap();
    tokio::task::yield_now().await;
    a.access_token = "renewed-access".into();
    a.refresh_token = Some("rotated-refresh".into());
    auth.save(p.id, &a).await.unwrap();
    drop(guard);
    switch.await.unwrap().unwrap();
    let current = auth.session(p.id).await.unwrap().unwrap();
    assert_eq!(current.access_token, "renewed-access");
    assert_eq!(current.refresh_token.as_deref(), Some("rotated-refresh"));
    clean(auth, root).await;
}

#[tokio::test]
async fn switching_while_catalog_request_is_running_rejects_the_old_accounts_result() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let a = tokens("account-a");
    let b = tokens("account-b");
    auth.save(p.id, &b).await.unwrap();
    auth.activate(p, &a).await.unwrap();
    let requested = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let on_request = requested.clone();
    let can_finish = release.clone();
    let expected_token = format!("Bearer {}", a.access_token);
    *auth.catalog_transport.write() = Some(Arc::new(move |request| {
        assert_eq!(request.headers["chatgpt-account-id"], "account-a");
        assert_eq!(request.headers["authorization"], expected_token);
        let on_request = on_request.clone();
        let can_finish = can_finish.clone();
        Box::pin(async move {
            on_request.notify_one();
            can_finish.notified().await;
            Ok(json!({"models":[{"slug":"only-for-account-a"}]}))
        })
    }));
    let actor = auth.clone();
    let refresh = tokio::spawn(async move { actor.refresh_catalog("openai-codex").await });
    requested.notified().await;
    auth.handle(
        "switch",
        &json!({"provider":p.id,"accountScope":b.account_scope}),
    )
    .await
    .unwrap();
    release.notify_one();
    assert!(refresh.await.unwrap().unwrap_err().contains("账号已切换"));
    assert_eq!(auth.catalogs.scope(p.id), Some(b.account_scope.clone()));
    assert!(auth.catalogs.get(p.id, &b.account_scope).models.is_empty());
    clean(auth, root).await;
}

#[tokio::test]
async fn broken_account_directory_does_not_overwrite_the_active_login() {
    let (auth, _ctx, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let a = tokens("account-a");
    auth.credentials
        .set(&reference(p.id), &serde_json::to_string(&a).unwrap())
        .await
        .unwrap();
    auth.credentials
        .set(&accounts_reference(p.id), "invalid-json")
        .await
        .unwrap();
    assert!(auth.save(p.id, &tokens("account-b")).await.is_err());
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        a.account_scope
    );
    assert_eq!(
        auth.credentials
            .resolve(&accounts_reference(p.id))
            .await
            .unwrap()
            .value,
        "invalid-json"
    );
    let directory = auth.handle("providers", &json!({})).await.unwrap();
    assert_eq!(
        directory["providers"].as_array().unwrap().len(),
        PROVIDERS.len()
    );
    let codex = directory["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == p.id)
        .unwrap();
    assert!(codex["error"].as_str().unwrap().contains("账号目录"));
    assert!(
        directory["providers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["id"] != p.id)
            .all(|item| item.get("error").is_none())
    );
    clean(auth, root).await;
}

#[tokio::test]
async fn failed_account_profile_write_rolls_back_active_credentials_and_catalog_binding() {
    let storage = Arc::new(MemorySettings::default());
    let (auth, _ctx, root) = setup_with_storage(storage.clone()).await;
    let p = provider("openai-codex").unwrap();
    let a = tokens("account-a");
    let b = tokens("account-b");
    auth.save(p.id, &b).await.unwrap();
    auth.activate(p, &a).await.unwrap();
    let original = auth.profile_snapshot(p.id, false).unwrap().0;
    storage
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = auth
        .handle(
            "switch",
            &json!({"provider":p.id,"accountScope":b.account_scope}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("活动账号保持不变"));
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().access_token,
        a.access_token
    );
    assert_eq!(auth.profile_snapshot(p.id, false).unwrap().0, original);
    assert_eq!(auth.catalogs.scope(p.id), Some(a.account_scope));
    assert_eq!(auth.saved_sessions(p.id).await.unwrap().len(), 2);
    storage
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        auth.handle(
            "switch",
            &json!({"provider":p.id,"accountScope":b.account_scope})
        )
        .await
        .unwrap()["status"],
        "switched"
    );
    clean(auth, root).await;
}

#[tokio::test]
async fn stale_usage_account_scope_is_rejected_before_account_rpc() {
    let (auth, _ctx, root) = setup().await;
    auth.save("openai-codex", &tokens("account-b"))
        .await
        .unwrap();
    let error = auth
        .handle(
            "usage",
            &json!({"provider":"openai-codex","accountScope":tokens("account-a").account_scope}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("账号已切换"));
    clean(auth, root).await;
}

#[tokio::test]
async fn login_connect_refresh_use_live_account_catalog_and_preserve_field_preferences() {
    use dsh_llm::LlmAdapter;
    let (auth, ctx, root) = setup().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_fetch = calls.clone();
    *auth.catalog_transport.write() = Some(Arc::new(move |request| {
        assert_eq!(
            request.url.as_str(),
            "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0"
        );
        assert!(
            request
                .headers
                .get("chatgpt-account-id")
                .is_some_and(|v| v == "account-a"),
            "directory request must identify the fixture account"
        );
        assert!(
            request.headers.get("authorization").is_some(),
            "access token required"
        );
        let round = calls_for_fetch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(
                json!({"models":[{"slug":"gpt-6-account-model","display_name":"Account model","context_window":if round==0{100000}else{200000},
            "default_reasoning_level":"ultra","supported_reasoning_levels":[{"effort":"high","description":"Careful"},{"effort":"ultra","description":"Thorough"}],
            "supports_reasoning_summaries":false,"supported_endpoints":["/responses"],"visibility":"list"},
            {"slug":"second-model","visibility":"list"},{"slug":"hidden-model","visibility":"hidden"}]}),
            )
        })
    }));
    let session = tokens("account-a");
    let p = provider("openai-codex").unwrap();
    auth.pending.lock().insert(
        "login".into(),
        (
            p,
            Arc::new(tokio::sync::Mutex::new(Pending {
                provider: p,
                device: "fixture".into(),
                user_code: "fixture".into(),
                expires: now() + 100,
                next_poll: now(),
                interval: 5,
                verifier: None,
            })),
        ),
    );
    auth.commit("login", p, &session).await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let view = auth.model_view(p.id).await.unwrap();
    assert_eq!(view["catalog"]["count"], 2);
    assert!(view["models"][0]["reasoning"]["defaultEffort"].is_null());
    assert_eq!(view["models"][0]["executionModes"], json!(["ultra"]));
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns,json!({"providers":{"openai-codex":{"modelPreferences":{session.account_scope.clone():{"gpt-6-account-model":{"enabled":false,"name":"My alias"}}}}}}),None).await.unwrap();
    auth.handle("connect", &json!({"provider":"openai-codex"}))
        .await
        .unwrap();
    let view = auth.model_view(p.id).await.unwrap();
    assert_eq!(view["models"][0]["contextWindow"], 200000);
    assert_eq!(view["models"][0]["name"], "My alias");
    assert_eq!(view["models"][0]["enabled"], false);
    assert!(view["models"][0]["effortDescriptions"]["ultra"].is_null());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let parsed = crate::openai_profiles(&auth.settings.get(&ns).unwrap()).unwrap();
    assert!(
        parsed.providers[p.id].models.is_empty(),
        "remote directory must not be copied into the user's manual model list"
    );
    let adapter = Arc::new(crate::OpenAiCompatibleAdapter {
        auth: Some(auth.clone()),
        profiles: Arc::new(parking_lot::Mutex::new(parsed.providers)),
        credentials: auth.credentials.clone(),
        attachment_ctx: ctx.clone(),
    });
    let listed = adapter.list_models(p.id).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "second-model");
    let resolved = adapter
        .resolve_model(p.id, "gpt-6-account-model", None)
        .await;
    let reasoning = resolved.reasoning.unwrap();
    assert!(reasoning.default_effort.is_none());
    assert!(
        reasoning
            .efforts
            .iter()
            .all(|effort| effort.id.as_str() != "ultra")
    );
    assert!(
        resolved
            .execution_modes
            .contains(&dsh_llm::ExecutionMode::Ultra)
    );
    let runtime = dsh_llm::LlmRuntime::install(&ctx);
    runtime
        .register_adapter(&ctx, vec![p.id.into()], adapter.clone())
        .unwrap();
    for legacy in [false, true] {
        let resolved = runtime
            .resolve_call_config(
                &dsh_llm::LlmCallConfig {
                    provider: p.id.into(),
                    model: "gpt-6-account-model".into(),
                    execution_mode: if legacy {
                        dsh_llm::ExecutionMode::Standard
                    } else {
                        dsh_llm::ExecutionMode::Ultra
                    },
                    reasoning_effort: legacy.then(|| dsh_llm::reasoning_effort_id("ultra")),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(resolved.execution_mode, dsh_llm::ExecutionMode::Ultra);
        assert_eq!(resolved.reasoning_effort.unwrap().as_str(), "high");
    }
    assert_eq!(resolved.context.unwrap().context_window, 200000);
    drop(adapter);
    clean(auth, root).await;
}

#[tokio::test]
async fn switching_accounts_and_failed_fetches_never_reuse_another_catalog() {
    let (auth, _, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let a = tokens("a");
    auth.save(p.id, &a).await.unwrap();
    auth.install_profile(p, &a).await.unwrap();
    *auth.catalog_transport.write() = Some(Arc::new(|_| {
        Box::pin(async { Ok(json!({"models":[{"slug":"a-only"}]})) })
    }));
    auth.refresh_catalog(p.id).await.unwrap();
    let b = tokens("b");
    auth.save(p.id, &b).await.unwrap();
    auth.install_profile(p, &b).await.unwrap();
    *auth.catalog_transport.write() = Some(Arc::new(|_| {
        Box::pin(async { Err("模型目录返回HTTP 503".into()) })
    }));
    assert!(auth.refresh_catalog(p.id).await.is_err());
    let view = auth.model_view(p.id).await.unwrap();
    assert_eq!(view["catalog"]["status"], "error");
    assert!(view["models"].as_array().unwrap().is_empty());
    assert_eq!(
        auth.catalogs.get(p.id, &a.account_scope).models[0].id,
        "a-only"
    );
    clean(auth, root).await;
}

#[tokio::test]
async fn an_existing_legacy_single_model_is_not_reported_as_a_synced_directory() {
    let (auth, _, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let session = tokens("legacy");
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns,json!({"providers":{"openai-codex":{"api":"openai-responses","baseURL":p.base,"apiKeyEnv":reference(p.id).as_str(),"authProvider":p.id,"models":[{"id":"gpt-5.4"}]}}}),None).await.unwrap();
    auth.save(p.id, &session).await.unwrap();
    auth.install_profile(p, &session).await.unwrap();
    let view = auth.model_view(p.id).await.unwrap();
    assert_eq!(view["catalog"]["status"], "not-synced");
    assert_eq!(view["models"][0]["enabled"], false);
    assert_eq!(view["models"][0]["source"], "legacy");
    clean(auth, root).await;
}

#[test]
fn vendor_catalog_endpoints_are_protocol_aware() {
    assert_eq!(
        models::catalog_url(
            &json!({"baseURL":"https://api.minimax.io/anthropic","api":"anthropic-messages"})
        )
        .unwrap()
        .as_str(),
        "https://api.minimax.io/v1/models"
    );
    assert_eq!(
        models::catalog_url(
            &json!({"baseURL":"https://api.anthropic.com","api":"anthropic-messages"})
        )
        .unwrap()
        .as_str(),
        "https://api.anthropic.com/v1/models"
    );
    assert!(models::catalog_url(&json!({"baseURL":"https://example.com","api":"openai-responses","authProvider":"openai-codex"})).is_err());
}

#[tokio::test]
async fn native_models_share_scoped_preferences_and_key_changes_reset_the_active_scope() {
    let (auth, _, root) = setup().await;
    let reference = format!(
        "DSH_TEST_NATIVE_{}",
        uuid::Uuid::new_v4().simple().to_string().to_uppercase()
    );
    auth.credentials
        .set(&dsh_credentials::credential_ref(&reference), "fixture-one")
        .await
        .unwrap();
    let ns = dsh_settings::settings_namespace("llm-deepseek").unwrap();
    auth.settings
        .update(&ns, json!({"apiKeyEnv":reference}), None)
        .await
        .unwrap();
    let first = auth.model_view("deepseek-official").await.unwrap();
    let scope = first["accountScope"].as_str().unwrap();
    let id = first["models"][0]["id"].as_str().unwrap();
    auth.settings
        .update(
            &ns,
            json!({"modelPreferences":{scope:{id:{"enabled":false,"name":"Native alias"}}}}),
            None,
        )
        .await
        .unwrap();
    let view = auth.model_view("deepseek-official").await.unwrap();
    assert_eq!(view["models"][0]["enabled"], false);
    let value = auth.settings.get(&ns).unwrap().to_json().unwrap();
    let native = crate::deepseek_settings::config(&auth.effective_native_config(&value)).unwrap();
    assert_eq!(native.models.unwrap()[0].enabled, Some(false));
    auth.credentials
        .set(&dsh_credentials::credential_ref(&reference), "fixture-two")
        .await
        .unwrap();
    let second = auth.model_view("deepseek-official").await.unwrap();
    assert_ne!(first["accountScope"], second["accountScope"]);
    assert_eq!(second["models"][0]["enabled"], true);
    clean(auth, root).await;
}

#[tokio::test]
async fn manual_provider_refresh_uses_the_same_view_without_turning_remote_rows_into_user_models() {
    let (auth, _, root) = setup().await;
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns,json!({"providers":{"fixture":{"keyless":true,"api":"openai-completions","baseURL":"http://127.0.0.1:9876/v1","models":[]}}}),None).await.unwrap();
    *auth.catalog_transport.write() = Some(Arc::new(|request| {
        assert!(request.headers.get("authorization").is_none());
        Box::pin(async {
            Ok(
                json!({"data":[{"id":"server-model","contextWindow":32000,"reasoningEfforts":{"deep":"deep"},"reasoningDefault":"deep"}]}),
            )
        })
    }));
    let view = auth.refresh_catalog("fixture").await.unwrap();
    assert_eq!(view["models"][0]["id"], "server-model");
    assert_eq!(view["models"][0]["reasoning"]["defaultEffort"], "deep");
    assert!(view["profileModels"].as_array().unwrap().is_empty());
    let scope = view["accountScope"].as_str().unwrap();
    auth.settings.update(&ns,json!({"providers":{"fixture":{"models":[{"id":"manual-model","source":"manual","accountScope":scope}]}}}),None).await.unwrap();
    assert_eq!(
        auth.model_view("fixture").await.unwrap()["models"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    clean(auth, root).await;
}

#[tokio::test]
async fn delayed_directory_refresh_preserves_preferences_saved_while_fetching() {
    let (auth, _, root) = setup().await;
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns,json!({"providers":{"fixture":{"keyless":true,"api":"openai-completions","baseURL":"http://127.0.0.1:9876/v1","models":[]}}}),None).await.unwrap();
    let view = auth.model_view("fixture").await.unwrap();
    let scope = view["accountScope"].as_str().unwrap().to_string();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let started_fetch = started.clone();
    let release_fetch = release.clone();
    *auth.catalog_transport.write() = Some(Arc::new(move |_| {
        let started = started_fetch.clone();
        let release = release_fetch.clone();
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            Ok(json!({"data":[{"id":"remote-model","contextWindow":96000}]}))
        })
    }));
    let fetch_auth = auth.clone();
    let fetch = tokio::spawn(async move { fetch_auth.refresh_catalog("fixture").await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    auth.settings.update(&ns,json!({"providers":{"fixture":{"modelPreferences":{scope.clone():{"remote-model":{"enabled":false,"name":"Saved during refresh"}}}}}}),Some(view["namespaceRevision"].as_u64().unwrap())).await.unwrap();
    release.notify_one();
    let refreshed = fetch.await.unwrap().unwrap();
    assert_eq!(refreshed["models"][0]["enabled"], false);
    assert_eq!(refreshed["models"][0]["name"], "Saved during refresh");
    assert_eq!(refreshed["models"][0]["contextWindow"], 96000);
    assert_eq!(refreshed["preferences"]["remote-model"]["enabled"], false);
    clean(auth, root).await;
}

#[tokio::test]
async fn existing_codex_tokens_repair_literal_claim_headers_before_directory_refresh() {
    let (auth, _, root) = setup().await;
    let p = provider("openai-codex").unwrap();
    let mut session = tokens("correct-account");
    session.account_id = Some("stale-account".into());
    session.account_scope = "stale-scope".into();
    auth.save(p.id, &session).await.unwrap();
    auth.install_profile(p, &session).await.unwrap();
    *auth.catalog_transport.write() = Some(Arc::new(|request| {
        assert_eq!(
            request.headers.get("chatgpt-account-id").unwrap(),
            "correct-account"
        );
        Box::pin(async { Ok(json!({"models":[{"slug":"new-account-model"}]})) })
    }));
    auth.refresh_catalog(p.id).await.unwrap();
    let profile = auth.profile_snapshot(p.id, false).unwrap().0;
    assert_eq!(profile["headers"]["ChatGPT-Account-ID"], "correct-account");
    assert_ne!(profile["modelCatalogScope"], "stale-scope");
    clean(auth, root).await;
}
