//! Native image/search requests share the exact account revocation boundary.
use super::*;
use dsh_agent::Agent;
use dsh_llm::{RequestAuthentication, RequestAuthenticationIdentity};
use std::sync::Weak;

const PAUSED: &str = "agent/native-auth-paused";
#[derive(Clone)]
struct Active {
    agent: Weak<dyn Agent>,
    authentication: RequestAuthentication,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_logout_is_scoped_durable_and_requires_new_user_admission() {
        let (auth, ctx, root) = super::super::model_tests::setup().await;
        let p = provider("openai-codex").unwrap();
        let a = super::super::model_tests::tokens("native-a");
        let b = super::super::model_tests::tokens("native-b");
        auth.activate(p, &a).await.unwrap();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let sessions = dsh_session::SessionStore::install(&ctx);
        dsh_session_persistence_jsonl::JsonlSessionPersistence::install(
            &ctx,
            dsh_session_persistence_jsonl::JsonlConfig {
                root: root.join("sessions").to_string_lossy().into(),
                ..Default::default()
            },
        )
        .unwrap();
        let session = sessions
            .create(&ctx, Some(dsh_session::session_id("native-owner")), None)
            .await
            .unwrap();
        let agent: Arc<dyn Agent> = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session.id().clone(),
            Default::default(),
            session.clone(),
        )
        .unwrap();
        let user = |text: &str| {
            dsh_llm::create_user_message(
                vec![dsh_llm::ContentBlock::Text { text: text.into() }],
                dsh_llm::MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            )
        };
        let first = user("generate a fixture");
        let namespace = dsh_settings::settings_namespace("llm-pi-ai").unwrap();
        assert!(auth.settings.update(&namespace,json!({"providers":{"openai-codex":{"baseURL":"https://untrusted.invalid/v1"}}}),None).await.is_err(),"untrusted subscription endpoints are rejected before credentials can be resolved");
        session
            .append(
                "user/message",
                serde_json::to_value(&first).unwrap(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        let (profile, key, lease) = auth
            .native_connection_for_agent(p.id, &agent)
            .await
            .unwrap();
        auth.check_native_connection(p.id, &profile, key.as_deref(), &lease)
            .await
            .unwrap();
        let queued = user("already queued before logout");
        session
            .append(
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"inserted":[queued]}),
                None,
            )
            .unwrap();
        auth.activate(p, &b).await.unwrap();
        assert!(
            auth.check_native_connection(p.id, &profile, key.as_deref(), &lease)
                .await
                .unwrap_err()
                .starts_with("EXECUTION_CONTEXT_CHANGED:")
        );
        let session_b = sessions
            .create(&ctx, Some(dsh_session::session_id("native-other")), None)
            .await
            .unwrap();
        let agent_b: Arc<dyn Agent> = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session_b.id().clone(),
            Default::default(),
            session_b,
        )
        .unwrap();
        let (_, _, lease_b) = auth
            .native_connection_for_agent(p.id, &agent_b)
            .await
            .unwrap();
        let impact = auth
            .handle(
                "logout-impact",
                &json!({"provider":p.id,"accountScope":a.account_scope}),
            )
            .await
            .unwrap();
        assert_eq!(impact["nativeRequestCount"], 1);
        let result = auth.handle("logout", &impact).await.unwrap();
        assert_eq!(result["cancelledNativeSessionCount"], 1);
        assert!(lease.revoked());
        assert!(!lease_b.revoked());
        assert!(
            auth.native_connection_for_agent(p.id, &agent)
                .await
                .err()
                .unwrap()
                .starts_with("AUTHENTICATION_REVOKED:")
        );
        session
            .append(
                "user/message",
                serde_json::to_value(&queued).unwrap(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        let recovered = NativeRequests::default();
        assert!(
            recovered.check(&agent, p.id).is_err(),
            "claiming an old queued user message is not new consent, even without the memory cache"
        );
        let fresh = user("resume now");
        session
            .append(
                "user/message",
                serde_json::to_value(fresh).unwrap(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        recovered.check(&agent, p.id).unwrap();
        auth.native_connection_for_agent(p.id, &agent)
            .await
            .unwrap();
        let event = session
            .find_event_rev(|e| e.type_ == PAUSED)
            .unwrap()
            .unwrap();
        assert!(!event.data.to_string().contains(&a.access_token));
        drop(lease);
        drop(lease_b);
        auth.credentials.drain().await;
        for dispose in ctx.fiber.disposables.clear() {
            dispose().await;
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[derive(Default)]
pub(super) struct NativeRequests {
    active: parking_lot::Mutex<HashMap<String, Active>>,
    paused: parking_lot::Mutex<HashMap<(String, String), u64>>,
}
pub(crate) struct NativeRequestLease {
    state: Weak<NativeRequests>,
    id: Option<String>,
    authentication: Option<RequestAuthentication>,
}
impl NativeRequestLease {
    pub fn revoked(&self) -> bool {
        self.authentication
            .as_ref()
            .is_some_and(RequestAuthentication::is_revoked)
    }
    pub fn cancellation(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let auth = self.authentication.clone();
        Arc::new(move || auth.as_ref().is_some_and(RequestAuthentication::is_revoked))
    }
}
impl Drop for NativeRequestLease {
    fn drop(&mut self) {
        if let (Some(state), Some(id)) = (self.state.upgrade(), &self.id) {
            state.active.lock().remove(id);
        }
    }
}
pub(super) struct Pause {
    pub agent: Arc<dyn Agent>,
    pub provider: String,
    pub cutoff: u64,
}

fn admitted_user(session: &dsh_session::Session) -> Result<Option<u64>, String> {
    let Some(message) = session
        .find_event_rev(|e| e.type_ == "user/message" && e.data["source"]["kind"] == "user")?
    else {
        return Ok(None);
    };
    let id = message.data["id"].as_str().unwrap_or("");
    let admitted = session.find_event_rev(|e| {
        e.seq.get() <= message.seq.get()
            && dsh_agent::inbox_splice_of(e).is_some_and(|splice| {
                splice.inserted.iter().any(|item| {
                    item.id.as_str() == id
                        && matches!(item.source, dsh_llm::MessageSource::User { .. })
                })
            })
    })?;
    Ok(Some(
        admitted.map_or(message.seq.get(), |event| event.seq.get()),
    ))
}
impl NativeRequests {
    fn check(&self, agent: &Arc<dyn Agent>, provider: &str) -> Result<(), String> {
        let owner = agent.id().as_str();
        let session = agent.session();
        let inherited = session.inherited_event_count().get();
        let stored = session
            .find_event_rev(|e| {
                e.type_ == PAUSED
                    && e.data["owner"] == owner
                    && e.data["provider"] == provider
                    && e.seq.get() >= inherited
            })?
            .and_then(|e| e.data["cutoff"].as_u64());
        let memory = self
            .paused
            .lock()
            .get(&(owner.into(), provider.into()))
            .copied();
        let cutoff = stored.into_iter().chain(memory).max();
        if let Some(cutoff) = cutoff {
            if admitted_user(session)?.is_none_or(|seq| seq < cutoff) {
                return Err("AUTHENTICATION_REVOKED: 此账号关联的图像或搜索请求已暂停；需要用户重新发送指令后才能使用当前账号继续，不能自动换账号重试。".into());
            }
            self.paused.lock().remove(&(owner.into(), provider.into()));
        }
        Ok(())
    }
    pub fn register(
        self: &Arc<Self>,
        agent: &Arc<dyn Agent>,
        authentication: Option<RequestAuthentication>,
    ) -> Result<NativeRequestLease, String> {
        let mut id = None;
        if let Some(auth) = &authentication {
            let RequestAuthenticationIdentity::Account { auth_provider, .. } = auth.identity()
            else {
                return Err("Unsupported native authentication identity".into());
            };
            self.check(agent, auth_provider)?;
            if auth.is_revoked() {
                return Err("AUTHENTICATION_REVOKED: 账号已退出".into());
            }
            let mut active = self.active.lock();
            if active.len() >= 64 {
                return Err("Too many active native requests".into());
            }
            let key = uuid::Uuid::new_v4().to_string();
            active.insert(
                key.clone(),
                Active {
                    agent: Arc::downgrade(agent),
                    authentication: auth.clone(),
                },
            );
            id = Some(key);
        }
        Ok(NativeRequestLease {
            state: Arc::downgrade(self),
            id,
            authentication,
        })
    }
    pub fn owners(&self, identity: &RequestAuthenticationIdentity) -> Vec<String> {
        self.active
            .lock()
            .values()
            .filter(|v| v.authentication.identity() == identity)
            .filter_map(|v| v.agent.upgrade().map(|a| a.id().as_str().to_owned()))
            .collect()
    }
    pub fn pause(&self, identity: &RequestAuthenticationIdentity) -> Vec<Pause> {
        let RequestAuthenticationIdentity::Account { auth_provider, .. } = identity else {
            return vec![];
        };
        let mut selected = std::collections::BTreeMap::new();
        for active in self
            .active
            .lock()
            .values()
            .filter(|v| v.authentication.identity() == identity)
        {
            if let Some(agent) = active.agent.upgrade() {
                selected.insert(agent.id().as_str().to_owned(), agent);
            }
        }
        selected
            .into_values()
            .map(|agent| {
                let cutoff = agent.session().seq().get();
                self.paused
                    .lock()
                    .insert((agent.id().as_str().into(), auth_provider.clone()), cutoff);
                Pause {
                    agent,
                    provider: auth_provider.clone(),
                    cutoff,
                }
            })
            .collect()
    }
    pub async fn persist(pauses: Vec<Pause>) -> Vec<String> {
        let mut failures = vec![];
        for pause in pauses {
            let session = pause.agent.session();
            if let Err(error)=session.append(PAUSED,json!({"owner":pause.agent.id().as_str(),"provider":pause.provider,"cutoff":pause.cutoff}),None){failures.push(error);continue;}
            if let Some(store) = pause
                .agent
                .ctx()
                .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            {
                if let Err(error) = store.flush(session).await {
                    failures.push(error);
                }
            } else {
                failures.push("Native authorization pause has no durability service".into());
            }
        }
        failures
    }
}

impl AccountAuth {
    pub(crate) async fn check_native_connection(
        &self,
        route: &str,
        profile: &Value,
        key: Option<&str>,
        lease: &NativeRequestLease,
    ) -> Result<(), String> {
        if lease.revoked() {
            return Err("AUTHENTICATION_REVOKED: 账号已退出，请由用户重新发起操作".into());
        }
        let (current, credential, _) = self.native_connection_snapshot(route).await?;
        if &current != profile || credential.as_deref() != key {
            return Err(
                "EXECUTION_CONTEXT_CHANGED: 模型连接或凭据已改变，请在当前配置下重新发起操作"
                    .into(),
            );
        }
        if lease.revoked() {
            return Err("AUTHENTICATION_REVOKED: 账号已退出，请由用户重新发起操作".into());
        }
        Ok(())
    }
    pub(crate) async fn native_connection_for_agent(
        &self,
        route: &str,
        agent: &Arc<dyn Agent>,
    ) -> Result<(Value, Option<String>, NativeRequestLease), String> {
        let _guard = self.refresh.lock().await;
        let profile = self.model_profile(route)?;
        crate::native_tool_compatibility::ensure_supported(
            route,
            crate::native_tool_compatibility::status(Some(&profile)),
        )?;
        let (key, authentication) = if let Some(auth) = profile["authProvider"].as_str() {
            let base = profile["baseURL"].as_str().unwrap_or("");
            let api = profile["api"].as_str().unwrap_or("");
            if !valid_profile(auth, base, api) {
                return Err("AUTH_TARGET_MISMATCH: 订阅凭据不能发送到其他服务地址".into());
            }
            self.native_requests.check(agent, auth)?;
            let headers = profile["headers"]
                .as_object()
                .map(|headers| {
                    headers
                        .iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let key = self
                .resolve_request_token_for_scope_locked(
                    auth,
                    base,
                    &headers,
                    profile["modelCatalogScope"].as_str(),
                )
                .await?;
            let session = self
                .session(auth)
                .await?
                .filter(|s| !s.invalid)
                .ok_or("AUTHENTICATION_REQUIRED: 账号尚未登录")?;
            (
                key,
                Some(self.current_request_identity(auth, &session.account_scope)?),
            )
        } else if profile["keyless"] == true {
            (None, None)
        } else if let Some(reference) = profile["apiKeyEnv"].as_str() {
            super::super::deepseek_settings::validate_api_key_reference(reference)?;
            (
                self.credentials
                    .resolve(&dsh_credentials::credential_ref(reference))
                    .await
                    .map(|value| value.value),
                None,
            )
        } else {
            (None, None)
        };
        let lease = self.native_requests.register(agent, authentication)?;
        Ok((profile, key, lease))
    }
}
