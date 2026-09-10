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
fn setup() -> (Arc<AccountAuth>, std::path::PathBuf) {
    let ctx = cordis::Context::root();
    let root = std::env::temp_dir().join(format!("dsh-auth-review-{}", uuid::Uuid::new_v4()));
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
    (AccountAuth::new(credentials, settings).unwrap(), root)
}
fn pending(auth: &AccountAuth, id: &str) -> Provider {
    let p = provider("openai-codex").unwrap();
    auth.pending.lock().insert(
        id.into(),
        (
            p,
            Arc::new(tokio::sync::Mutex::new(Pending {
                provider: p,
                device: "state".into(),
                user_code: String::new(),
                expires: now() + 900,
                next_poll: now(),
                interval: 5,
                verifier: None,
                authorization: None,
            })),
        ),
    );
    p
}
fn tokens() -> Session {
    Session::from_tokens(
        &json!({"access_token":"test-access","refresh_token":"test-refresh","expires_in":3600}),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn cancellation_queued_before_commit_prevents_credential_write() {
    let (auth, root) = setup();
    let p = pending(&auth, "attempt");
    let guard = auth.refresh.lock().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let actor = auth.clone();
    let cancel = tokio::spawn(async move {
        signal.notify_one();
        actor.handle("cancel", &json!({"attempt":"attempt"})).await
    });
    started.notified().await;
    let actor = auth.clone();
    let commit = tokio::spawn(async move { actor.commit("attempt", p, &tokens()).await });
    drop(guard);
    assert_eq!(cancel.await.unwrap().unwrap()["status"], "cancelled");
    assert!(commit.await.unwrap().unwrap_err().contains("取消"));
    assert!(auth.session("openai-codex").await.unwrap().is_none());
    auth.credentials.drain().await;
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn logout_invalidates_all_provider_attempts_before_late_commit() {
    let (auth, root) = setup();
    let p = pending(&auth, "first");
    pending(&auth, "second");
    auth.save("openai-codex", &tokens()).await.unwrap();
    auth.handle("logout", &json!({"provider":"openai-codex"}))
        .await
        .unwrap();
    assert!(!auth.pending.lock().contains_key("first"));
    assert!(!auth.pending.lock().contains_key("second"));
    assert!(auth.commit("first", p, &tokens()).await.is_err());
    assert!(auth.session("openai-codex").await.unwrap().is_none());
    auth.credentials.drain().await;
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[test]
fn token_refresh_preserves_account_route_and_unrotated_refresh_token() {
    let mut original = tokens();
    original.account_id = Some("account".into());
    original.base_url = Some("https://api.business.githubcopilot.com".into());
    let renewed = Session::from_tokens(
        &json!({"access_token":"renewed","expires_in":1800}),
        Some(&original),
    )
    .unwrap();
    assert_eq!(renewed.account_id, original.account_id);
    assert_eq!(renewed.base_url, original.base_url);
    assert_eq!(renewed.refresh_token, original.refresh_token);
}

#[tokio::test]
async fn transient_grant_exchange_keeps_one_grant_and_cancellation_remains_final() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url: &'static str =
        Box::leak(format!("http://{}/token", listener.local_addr().unwrap()).into_boxed_str());
    let (auth, root) = setup();
    pending(&auth, "attempt");
    let state = auth.pending.lock()["attempt"].1.clone();
    let grant = json!({"authorization_code":"single-grant","code_verifier":"pkce-verifier"});
    {
        let mut state = state.lock().await;
        state.provider.token = url;
        state.authorization = Some(grant.clone());
    }
    let server = tokio::spawn(async move {
        for status in [503, 429] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let size = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..size]);
            assert!(request.contains("code=single-grant"));
            assert!(request.contains("code_verifier=pkce-verifier"));
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    for _ in 0..2 {
        state.lock().await.next_poll = 0;
        let reply = auth.poll("attempt").await.unwrap();
        assert_eq!(reply["status"], "pending");
        assert_eq!(reply["retryable"], true);
        assert!(!reply.to_string().contains("single-grant"));
        assert_eq!(state.lock().await.authorization, Some(grant.clone()));
        assert!(auth.session("openai-codex").await.unwrap().is_none());
    }
    server.await.unwrap();
    auth.handle("cancel", &json!({"attempt":"attempt"}))
        .await
        .unwrap();
    assert!(auth.poll("attempt").await.is_err());
    auth.credentials.drain().await;
    let _ = tokio::fs::remove_dir_all(root).await;
}
