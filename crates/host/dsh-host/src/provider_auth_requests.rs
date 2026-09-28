//! Account lifetime binding for model calls and precise logout cancellation.

use super::*;
use dsh_llm::{RequestAuthentication, RequestAuthenticationIdentity};

impl AccountAuth {
    pub(crate) fn set_agents(&self, agents: &Arc<dsh_agent::AgentRegistry>) {
        *self.agents.write() = Arc::downgrade(agents);
    }

    pub(super) fn authorize_request_identity(
        &self,
        id: &str,
        account_scope: &str,
    ) -> RequestAuthentication {
        let mut identities = self.request_identities.lock();
        let key = (id.to_owned(), account_scope.to_owned());
        let identity = identities
            .entry(key)
            .or_insert_with(|| Self::new_request_identity(id, account_scope));
        if identity.is_revoked() {
            *identity = Self::new_request_identity(id, account_scope);
        }
        identity.clone()
    }

    fn new_request_identity(id: &str, account_scope: &str) -> RequestAuthentication {
        RequestAuthentication::new(RequestAuthenticationIdentity::Account {
            auth_provider: id.into(),
            account_scope: account_scope.into(),
            login_generation: uuid::Uuid::new_v4().to_string(),
        })
    }

    pub(super) fn current_request_identity(
        &self,
        id: &str,
        account_scope: &str,
    ) -> Result<RequestAuthentication, String> {
        let mut identities = self.request_identities.lock();
        let identity = identities
            .entry((id.to_owned(), account_scope.to_owned()))
            .or_insert_with(|| Self::new_request_identity(id, account_scope));
        if identity.is_revoked() {
            return Err("账号已退出，请重新登录".into());
        }
        Ok(identity.clone())
    }

    pub(crate) async fn capture_request_authentication(
        &self,
        id: &str,
        expected_scope: Option<&str>,
    ) -> Result<RequestAuthentication, String> {
        let _guard = self.refresh.lock().await;
        let session = self
            .session(id)
            .await?
            .filter(|session| !session.invalid)
            .ok_or("账号尚未登录，请在模型设置中登录")?;
        if expected_scope != Some(session.account_scope.as_str()) {
            return Err("账号请求配置已更新，请重试当前请求".into());
        }
        self.current_request_identity(id, &session.account_scope)
    }

    pub(crate) async fn resolve_request_token_for_authentication(
        &self,
        id: &str,
        base: &str,
        headers: &[(String, String)],
        expected_scope: Option<&str>,
        authentication: &RequestAuthentication,
    ) -> Result<Option<String>, String> {
        let _guard = self.refresh.lock().await;
        let RequestAuthenticationIdentity::Account {
            auth_provider,
            account_scope,
            ..
        } = authentication.identity()
        else {
            return Err("账号请求缺少已捕获的授权身份".into());
        };
        let current = self.current_request_identity(id, account_scope)?;
        if authentication.is_revoked()
            || auth_provider != id
            || current.identity() != authentication.identity()
            || self
                .session(id)
                .await?
                .is_none_or(|session| session.account_scope != *account_scope)
        {
            return Err("账号已退出或登录状态已更改，请重新发起任务".into());
        }
        self.resolve_request_token_for_scope_locked(id, base, headers, expected_scope)
            .await
    }

    pub(super) async fn logout_impact(&self, body: &Value) -> Result<Value, String> {
        let id = string(body, "provider")?;
        provider(&id)?;
        let identity = {
            let _guard = self.refresh.lock().await;
            let accounts = self.account_sessions(&id).await?;
            let requested = if body.get("accountScope").is_some() {
                string(body, "accountScope")?
            } else {
                self.session(&id)
                    .await?
                    .map(|session| session.account_scope)
                    .ok_or("未找到要移除的登录账号")?
            };
            let selected = accounts
                .iter()
                .find(|session| session.account_scope == requested)
                .ok_or("未找到要移除的登录账号")?;
            self.current_request_identity(&id, &selected.account_scope)?
        };
        let agents = self
            .agents
            .read()
            .upgrade()
            .ok_or("暂时无法读取运行任务，退出影响未知，请重试")?;
        let mut owners = agents
            .list()
            .iter()
            .filter(|agent| {
                agent
                    .authentication_binding()
                    .is_some_and(|binding| &binding.identity == identity.identity())
            })
            .map(|agent| agent.id().as_str().to_owned())
            .collect::<std::collections::HashSet<_>>();
        let native_owners = self.native_requests.owners(identity.identity());
        let native_count = native_owners.len();
        owners.extend(native_owners);
        let count = owners.len();
        let RequestAuthenticationIdentity::Account {
            account_scope,
            login_generation,
            ..
        } = identity.identity()
        else {
            unreachable!()
        };
        Ok(
            json!({"provider":id,"accountScope":account_scope,"loginGeneration":login_generation,"taskCount":count,
            "nativeRequestCount":native_count,"reason":"account-signed-out"}),
        )
    }

    pub(super) async fn logout_account(&self, body: &Value) -> Result<Value, String> {
        let id = string(body, "provider")?;
        let p = provider(&id)?;
        // Old clients cannot safely infer an account after a confirmation
        // delay. Fail closed instead of removing whichever account is current.
        let scope = string(body, "accountScope")
            .map_err(|_| "请刷新账号列表并重新确认退出（缺少 accountScope）".to_string())?;
        let generation = string(body, "loginGeneration")
            .map_err(|_| "请刷新账号列表并重新确认退出（缺少 loginGeneration）".to_string())?;
        let (identity, next_account, native_pauses) = {
            let _guard = self.refresh.lock().await;
            let current = self.session(&id).await?;
            let previous_accounts = self.account_sessions(&id).await?;
            if !previous_accounts
                .iter()
                .any(|session| session.account_scope == scope)
            {
                return Err("账号状态已改变，请重新确认退出".into());
            }
            let identity = self.current_request_identity(&id, &scope)?;
            if !matches!(identity.identity(), RequestAuthenticationIdentity::Account { login_generation, .. } if login_generation == &generation)
            {
                return Err("账号登录状态已改变，请重新确认退出".into());
            }
            let accounts: Vec<_> = previous_accounts
                .iter()
                .filter(|session| session.account_scope != scope)
                .cloned()
                .collect();
            let remove_active = current
                .as_ref()
                .is_some_and(|session| session.account_scope == scope);
            if remove_active && id == "openai-codex" {
                self.account_usage.disconnect().await;
            }
            self.write_accounts(&id, &accounts).await?;
            if remove_active && let Err(error) = self.credentials.unset(&reference(&id)).await {
                let restored = self.write_accounts(&id, &previous_accounts).await;
                return Err(match restored {
                    Ok(()) => error,
                    Err(rollback) => format!("账号退出失败且目录恢复失败：{error}；{rollback}"),
                });
            }
            // Publish revocation before releasing the account lock. A late
            // first bind and a next step switching automatically to B both
            // see this same lease; cancellation itself runs outside the lock.
            let native_pauses = self.native_requests.pause(identity.identity());
            identity.revoke();
            self.pending.lock().retain(|_, (owner, _)| owner.id != id);
            if remove_active {
                self.catalogs.unbind(&id);
            }
            let next = remove_active
                .then(|| accounts.into_iter().find(|session| !session.invalid))
                .flatten();
            (identity, next, native_pauses)
        };
        let mut cancelled = 0;
        let mut paused = Vec::new();
        let native_cancelled = native_pauses.len();
        let mut pause_failures = Vec::new();
        if let Some(agents) = self.agents.read().upgrade() {
            for agent in agents.list() {
                if let Some(binding) = agent.authentication_binding()
                    && &binding.identity == identity.identity()
                {
                    match agent.cancel_authentication(&binding) {
                        Ok(true) => {
                            cancelled += 1;
                            paused.push(agent);
                        }
                        Ok(false) => {}
                        Err(error) => pause_failures.push(format!("{}: {error}", agent.id())),
                    }
                }
            }
        }
        pause_failures.extend(native::NativeRequests::persist(native_pauses).await);
        for agent in paused {
            if let Err(error) = agent.flush_authentication_control().await {
                pause_failures.push(format!("{}: {error}", agent.id()));
            }
        }
        if !pause_failures.is_empty() {
            return Err(format!(
                "账号凭据已移除，任务已停止自动继续，但暂停记录的持久化未确认；没有自动激活其他账号：{}",
                pause_failures.join("；")
            ));
        }
        // No await separates committed revocation from the precise stop scan.
        // A new login winning this later lock owns the active route; fallback
        // activation must never overwrite it while finishing an old logout.
        let activation: Result<(), String> = async {
            if let Some(next) = next_account {
                let _guard = self.refresh.lock().await;
                if self.session(&id).await?.is_none() {
                    let saved = self.account_sessions(&id).await?;
                    if let Some(current) = saved.iter().find(|session| {
                        session.account_scope == next.account_scope && !session.invalid
                    }) {
                        self.activate(p, current).await?;
                    }
                }
            }
            Ok(())
        }
        .await;
        let activation_error = activation.err();
        Ok(
            json!({"status":"signedOut","provider":id,"removedAccountScope":scope,"loginGeneration":generation,
            "reason":"account-signed-out","cancelledTaskCount":cancelled,"cancelledNativeSessionCount":native_cancelled,"warning":activation_error}),
        )
    }
}
