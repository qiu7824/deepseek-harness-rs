use super::*;

struct MemorySettings;
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
        Ok(())
    }
}
async fn setup() -> (Arc<AccountAuth>, cordis::Context, std::path::PathBuf) {
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
    let settings = dsh_settings::SettingsProvider::install(&ctx, Arc::new(MemorySettings));
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
    use base64::Engine;
    let payload = json!({"https://api.openai.com/auth":{"chatgpt_account_id":account},"sub":format!("subject-{account}"),"exp":now()+3600});
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
    assert_eq!(view["models"][0]["reasoning"]["defaultEffort"], "ultra");
    let ns = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
    auth.settings.update(&ns,json!({"providers":{"openai-codex":{"modelPreferences":{session.account_scope.clone():{"gpt-6-account-model":{"enabled":false,"name":"My alias"}}}}}}),None).await.unwrap();
    auth.handle("connect", &json!({"provider":"openai-codex"}))
        .await
        .unwrap();
    let view = auth.model_view(p.id).await.unwrap();
    assert_eq!(view["models"][0]["contextWindow"], 200000);
    assert_eq!(view["models"][0]["name"], "My alias");
    assert_eq!(view["models"][0]["enabled"], false);
    assert_eq!(view["models"][0]["effortDescriptions"]["ultra"], "Thorough");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let parsed = crate::openai_profiles(&auth.settings.get(&ns).unwrap()).unwrap();
    assert!(
        parsed.providers[p.id].models.is_empty(),
        "remote directory must not be copied into the user's manual model list"
    );
    let adapter = crate::OpenAiCompatibleAdapter {
        auth: Some(auth.clone()),
        profiles: Arc::new(parking_lot::Mutex::new(parsed.providers)),
        credentials: auth.credentials.clone(),
        attachment_ctx: ctx,
    };
    let listed = adapter.list_models(p.id).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "second-model");
    let resolved = adapter
        .resolve_model(p.id, "gpt-6-account-model", None)
        .await;
    let reasoning = resolved.reasoning.unwrap();
    assert_eq!(reasoning.default_effort.unwrap().as_str(), "ultra");
    assert_eq!(
        reasoning.efforts[1].description.as_deref(),
        Some("Thorough")
    );
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
