//! Account-scoped access to the official Codex App Server account protocol.
//! Reset attempts are journaled before transport and retain their idempotency key.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::Mutex,
};

pub(crate) struct AccountTokens {
    pub access_token: String,
    pub account_id: String,
}
/// Invoked only from an active account RPC while ProviderAuth owns its refresh lock.
pub(crate) type TokenRefresher = std::sync::Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, Result<AccountTokens, String>> + Send + Sync,
>;
struct RefreshRequest {
    id: Value,
    account_id: String,
}
struct RefreshWindow(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for RefreshWindow {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

async fn refresh_external_tokens(
    binding: Option<&(String, TokenRefresher)>,
    account_id: &str,
    attempted: &mut bool,
) -> Result<Value, String> {
    let (expected, refresh) = binding
        .filter(|(expected, _)| expected == account_id)
        .ok_or("账户身份不匹配，拒绝刷新其他账号")?;
    if *attempted {
        return Err("同一账户请求最多刷新一次".into());
    }
    *attempted = true;
    match tokio::time::timeout(Duration::from_secs(8), refresh()).await {
        Ok(Ok(tokens)) if tokens.account_id == *expected && !tokens.access_token.is_empty() => Ok(
            json!({"accessToken":tokens.access_token,"chatgptAccountId":tokens.account_id,"chatgptPlanType":null}),
        ),
        _ => Err("当前模型账号的正常刷新未完成".into()),
    }
}

fn account_rpc_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let message: String = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(2048)
        .collect();
    let lower = message.to_ascii_lowercase();
    let status = ["/data/status", "/data/statusCode", "/data/httpStatus"]
        .into_iter()
        .find_map(|path| {
            error
                .pointer(path)
                .and_then(Value::as_u64)
                .filter(|status| (400..600).contains(status))
        })
        .or_else(|| {
            [401, 403, 429, 500, 502, 503, 504]
                .into_iter()
                .find(|status| {
                    [
                        format!("http {status}"),
                        format!("http status {status}"),
                        format!("status code: {status}"),
                        format!("status {status}"),
                        format!("{status} unauthorized"),
                        format!("{status} forbidden"),
                        format!("{status} too many requests"),
                    ]
                    .iter()
                    .any(|value| lower.contains(value))
                })
        });
    match (code, status) {
        (Some(-32601), _) => "此 Codex CLI 版本不支持该账户功能，请更新官方 Codex CLI".into(),
        (_, Some(401)) => "账户服务认证已失效（HTTP 401）；请刷新当前登录凭据后重试".into(),
        (_, Some(403)) => {
            "账户服务拒绝访问（HTTP 403）；请检查账户权限或服务限制，登录状态已保留".into()
        }
        (_, Some(429)) => "账户服务请求过于频繁（HTTP 429），请稍后重试".into(),
        (_, Some(status)) if status >= 500 => {
            format!("账户服务暂时不可用（HTTP {status}），请稍后重试")
        }
        _ if lower.contains("authentication required") || lower.contains("not authenticated") => {
            "用量服务尚未接入当前模型账号，请刷新用量重新连接".into()
        }
        _ if lower.contains("external auth must use")
            || lower.contains("external chatgpt auth is disabled") =>
        {
            "当前 Codex 账户策略不允许此模型账号，请检查工作区或管理员登录策略；登录状态已保留"
                .into()
        }
        _ if lower.contains("experimentalapi") || lower.contains("experimental api") => {
            "此账户功能需要官方 Codex CLI 的实验协议支持，请更新客户端".into()
        }
        (Some(-32602), _) => "账户接口请求不受当前 Codex CLI 支持，请检查客户端版本".into(),
        _ => "账户服务未能完成请求；请稍后重试，当前登录状态已保留".into(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetOperation {
    operation_id: String,
    account_id: String,
    credit_id: Option<String>,
    idempotency_key: String,
    state: String,
    outcome: Option<String>,
    created_at: u64,
}
impl ResetOperation {
    fn unsettled(&self) -> bool {
        matches!(self.state.as_str(), "prepared" | "sending" | "unknown")
    }
    fn view(&self) -> Value {
        json!({"operationId":self.operation_id,"state":self.state,"outcome":self.outcome,"createdAt":self.created_at})
    }
    fn settle(&mut self, result: Result<Value, String>) -> Option<String> {
        match result {
            Ok(value) => match value.get("outcome").and_then(Value::as_str) {
                Some(outcome @ ("reset" | "alreadyRedeemed")) => {
                    self.state = "succeeded".into();
                    self.outcome = Some(outcome.into());
                    None
                }
                Some(outcome @ ("noCredit" | "nothingToReset")) => {
                    self.state = "notApplied".into();
                    self.outcome = Some(outcome.into());
                    None
                }
                _ => {
                    self.state = "unknown".into();
                    Some("兑换结果尚未确认，请核对该次操作".into())
                }
            },
            Err(_) => {
                self.state = "unknown".into();
                Some("连接中断，兑换结果尚未确认；核对时将复用原请求".into())
            }
        }
    }
}

type Replies = std::sync::Arc<
    parking_lot::Mutex<
        std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>,
    >,
>;
struct Bridge {
    child: Child,
    input: std::sync::Arc<Mutex<ChildStdin>>,
    pending: Replies,
    notifications: std::sync::Arc<parking_lot::Mutex<std::collections::VecDeque<Value>>>,
    reader: tokio::task::JoinHandle<()>,
    next_id: u64,
    refresh_rx: tokio::sync::mpsc::Receiver<RefreshRequest>,
    refresh_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    refresher: Option<(String, TokenRefresher)>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
#[async_trait::async_trait]
trait AccountRpc: Send {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    async fn stop(&mut self);
    fn alive(&mut self) -> bool {
        true
    }
    fn notifications(&mut self) -> Vec<Value> {
        Vec::new()
    }
    fn set_refresher(&mut self, _account_id: &str, _refresh: Option<TokenRefresher>) {}
}
#[async_trait::async_trait]
impl AccountRpc for Bridge {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.rpc(method, params).await
    }
    async fn stop(&mut self) {
        self.reader.abort();
        let _ = self.child.kill().await;
    }
    fn alive(&mut self) -> bool {
        !self.reader.is_finished() && self.child.try_wait().is_ok_and(|status| status.is_none())
    }
    fn notifications(&mut self) -> Vec<Value> {
        self.notifications.lock().drain(..).collect()
    }
    fn set_refresher(&mut self, account_id: &str, refresh: Option<TokenRefresher>) {
        self.refresher = refresh.map(|refresh| (account_id.to_string(), refresh));
    }
}
type Connector = std::sync::Arc<
    dyn Fn(PathBuf) -> futures::future::BoxFuture<'static, Result<Box<dyn AccountRpc>, String>>
        + Send
        + Sync,
>;

async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Value>, String> {
    const MAX: usize = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|_| "读取账户响应失败")?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("账户响应意外结束".into())
            };
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let take = end.map(|index| index + 1).unwrap_or(available.len());
        if bytes.len() + take > MAX {
            return Err("账户响应超过限制".into());
        }
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if end.is_some() {
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| "账户服务返回无效数据".into());
        }
    }
}
async fn write_frame(input: &Mutex<ChildStdin>, value: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| "账户请求编码失败")?;
    bytes.push(b'\n');
    let mut input = input.lock().await;
    input
        .write_all(&bytes)
        .await
        .map_err(|_| "账户服务连接已关闭")?;
    input.flush().await.map_err(|_| "账户服务连接已关闭".into())
}
impl Bridge {
    async fn start(home: &Path) -> Result<Self, String> {
        let executable =
            find_codex().ok_or("未找到 Codex CLI；请安装受支持的官方 Codex CLI 后连接用量账户")?;
        tokio::fs::create_dir_all(home)
            .await
            .map_err(|_| "无法创建 Codex 账户目录")?;
        let mut command = Command::new(executable);
        command
            .arg("app-server")
            .env("CODEX_HOME", home)
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| "无法启动 Codex 账户服务")?;
        let input = std::sync::Arc::new(Mutex::new(
            child.stdin.take().ok_or("账户服务缺少输入通道")?,
        ));
        let mut output = BufReader::new(child.stdout.take().ok_or("账户服务缺少输出通道")?);
        let pending: Replies = Default::default();
        let notifications =
            std::sync::Arc::new(parking_lot::Mutex::new(std::collections::VecDeque::new()));
        let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<RefreshRequest>(2);
        let refresh_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let active = refresh_active.clone();
        let replies = pending.clone();
        let notices = notifications.clone();
        let writer = input.clone();
        let reader = tokio::spawn(async move {
            let failure = loop {
                let frame = match read_frame(&mut output).await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break "账户服务已退出".to_string(),
                    Err(error) => break error,
                };
                if frame.get("method").is_some() {
                    if let Some(id) = frame.get("id") {
                        if frame["method"] == "account/chatgptAuthTokens/refresh"
                            && frame.pointer("/params/reason").and_then(Value::as_str)
                                == Some("unauthorized")
                            && active.load(std::sync::atomic::Ordering::Acquire)
                            && let Some(account_id) = frame
                                .pointer("/params/previousAccountId")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty() && id.len() <= 256)
                            && refresh_tx
                                .try_send(RefreshRequest {
                                    id: id.clone(),
                                    account_id: account_id.into(),
                                })
                                .is_ok()
                        {
                            continue;
                        }
                        let response = json!({"id":id,"error":{"code":-32601,"message":"Unsupported account host request"}});
                        if tokio::time::timeout(
                            Duration::from_secs(5),
                            write_frame(&writer, response),
                        )
                        .await
                        .is_err()
                        {
                            break "账户服务请求回复超时".into();
                        }
                    } else {
                        let mut notices = notices.lock();
                        if notices.len() >= 64 || frame.to_string().len() > 65536 {
                            notices.clear();
                            notices.push_back(json!({"method":"account/state/invalidated"}));
                        } else {
                            notices.push_back(frame);
                        }
                    }
                    continue;
                }
                if let Some(id) = frame.get("id").and_then(Value::as_u64) {
                    if let Some(reply) = replies.lock().remove(&id) {
                        let value = if let Some(error) = frame.get("error") {
                            Err(account_rpc_error(error))
                        } else {
                            frame
                                .get("result")
                                .cloned()
                                .ok_or_else(|| "账户响应缺少结果".into())
                        };
                        let _ = reply.send(value);
                    }
                }
            };
            for (_, reply) in replies.lock().drain() {
                let _ = reply.send(Err(failure.clone()));
            }
        });
        let mut bridge = Self {
            child,
            input,
            pending,
            notifications,
            reader,
            next_id: 1,
            refresh_rx,
            refresh_active,
            refresher: None,
        };
        bridge.rpc("initialize",json!({"clientInfo":{"name":"dsh-account","title":"DeepSeek Harness","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        tokio::time::timeout(
            Duration::from_secs(5),
            write_frame(&bridge.input, json!({"method":"initialized","params":{}})),
        )
        .await
        .map_err(|_| "账户初始化超时")??;
        Ok(bridge)
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        while let Ok(stale) = self.refresh_rx.try_recv() {
            tokio::time::timeout(Duration::from_secs(5),
                write_frame(&self.input,json!({"id":stale.id,"error":{"code":-32000,"message":"The previous account request ended"}})))
                .await.map_err(|_|"账户旧请求清理超时".to_string())??;
        }
        let id = self.next_id;
        self.next_id += 1;
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        self.pending.lock().insert(id, tx);
        self.refresh_active.store(
            self.refresher.is_some(),
            std::sync::atomic::Ordering::Release,
        );
        let _refresh_window = RefreshWindow(self.refresh_active.clone());
        let result = tokio::time::timeout(Duration::from_secs(35), async {
            write_frame(&self.input,json!({"id":id,"method":method,"params":params})).await?;
            let mut refreshed = false;
            loop {
                tokio::select! {
                    result = &mut rx => return result.map_err(|_|"账户连接已关闭".to_string())?,
                    request = self.refresh_rx.recv() => {
                        let Some(request)=request else {return Err("账户刷新通道已关闭".into())};
                        let result=refresh_external_tokens(self.refresher.as_ref(),&request.account_id,&mut refreshed).await;
                        let frame=match result {Ok(value)=>json!({"id":request.id,"result":value}),Err(message)=>json!({"id":request.id,"error":{"code":-32000,"message":message}})};
                        write_frame(&self.input,frame).await?;
                    }
                }
            }
        }).await.map_err(|_|"账户服务响应超时".to_string()).and_then(|result|result);
        self.pending.lock().remove(&id);
        result
    }
}

fn find_codex() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DSH_CODEX_COMMAND").map(PathBuf::from) {
        return (path.is_absolute() && path.is_file()).then_some(path);
    }
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join(name);
        if candidate.is_absolute() && candidate.is_file() {
            return Some(candidate);
        }
    }
    #[cfg(windows)]
    {
        let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("OpenAI/Codex/bin");
        let mut paths = std::fs::read_dir(root)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path().join("codex.exe"))
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();
        paths.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        paths.pop()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

struct Binding {
    epoch: uuid::Uuid,
    last_used: tokio::time::Instant,
    scope: String,
    account_id: String,
    bridge: Box<dyn AccountRpc>,
    snapshot: Option<Value>,
    snapshot_at: u64,
    token_key: String,
}
pub(crate) struct CodexAccountService {
    root: PathBuf,
    binding: std::sync::Arc<Mutex<Option<Binding>>>,
    connector: Connector,
}
impl CodexAccountService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            binding: std::sync::Arc::new(Mutex::new(None)),
            connector: std::sync::Arc::new(|home| {
                Box::pin(async move {
                    Bridge::start(&home)
                        .await
                        .map(|bridge| Box::new(bridge) as Box<dyn AccountRpc>)
                })
            }),
        }
    }
    pub async fn disconnect(&self) {
        let mut slot = self.binding.lock().await;
        if let Some(mut binding) = slot.take() {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                binding.bridge.call("account/logout", json!({})),
            )
            .await;
            binding.bridge.stop().await;
        }
    }

    async fn release_idle(binding: &Mutex<Option<Binding>>, epoch: uuid::Uuid) -> Option<Duration> {
        const IDLE: Duration = Duration::from_secs(60);
        let mut guard = binding.lock().await;
        let current = guard.as_ref().filter(|current| current.epoch == epoch)?;
        let elapsed = tokio::time::Instant::now().saturating_duration_since(current.last_used);
        if elapsed < IDLE {
            return Some(IDLE - elapsed);
        }
        if let Some(mut old) = guard.take() {
            // Stop this helper only. Login credentials and reset journals stay
            // intact, and no account/logout or redemption RPC is sent.
            let _ = tokio::time::timeout(Duration::from_secs(5), old.bridge.stop()).await;
        }
        None
    }

    fn expire_idle(&self, epoch: uuid::Uuid) {
        let binding = std::sync::Arc::downgrade(&self.binding);
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(60);
            loop {
                tokio::time::sleep(delay).await;
                let Some(active) = binding.upgrade() else {
                    break;
                };
                let next = Self::release_idle(&active, epoch).await;
                drop(active);
                match next {
                    Some(next) => delay = next,
                    None => break,
                }
            }
        });
    }
    fn directory(&self, scope: &str) -> PathBuf {
        self.root.join(crate::provider_auth_catalog::key(scope))
    }
    async fn save_operation(path: &Path, operation: &ResetOperation) -> Result<(), String> {
        let bytes = serde_json::to_vec(operation).map_err(|_| "无法编码兑换记录")?;
        dsh_atomic_write::write_file_atomic(
            path,
            &bytes,
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|_| "无法保存兑换记录；请保留当前操作并核对结果".into())
    }
    async fn operation(path: &Path) -> Result<Option<ResetOperation>, String> {
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| "兑换记录损坏，无法安全建立新兑换".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("无法读取兑换记录".into()),
        }
    }
    async fn read_usage(
        binding: &mut Binding,
        account_id: &str,
        force: bool,
    ) -> Result<Value, String> {
        for notification in binding.bridge.notifications() {
            match notification.get("method").and_then(Value::as_str) {
                Some("account/rateLimits/updated") => {
                    if let Some(snapshot) = binding.snapshot.as_mut() {
                        if let Some(update) = notification.pointer("/params/rateLimits") {
                            merge_rate_limit(snapshot, update);
                        }
                    }
                }
                Some("account/updated")
                    if notification
                        .pointer("/params/authMode")
                        .and_then(Value::as_str)
                        == Some("chatgptAuthTokens") =>
                {
                    // The successful external bind already invalidated the
                    // cache before its RPC. Its delayed confirmation must not
                    // expire a quota snapshot fetched after that bind.
                }
                Some("account/login/completed")
                    if notification.pointer("/params/success") == Some(&Value::Bool(true))
                        && notification
                            .pointer("/params/loginId")
                            .is_none_or(Value::is_null) =>
                {
                    // Idempotent confirmation of the same verified identity.
                }
                Some(
                    "account/updated" | "account/login/completed" | "account/state/invalidated",
                ) => {
                    binding.snapshot = None;
                    binding.snapshot_at = 0;
                    binding.token_key.clear();
                }
                _ => (),
            }
        }
        if !force && now().saturating_sub(binding.snapshot_at) < 30 {
            if let Some(snapshot) = &binding.snapshot {
                return Ok(snapshot.clone());
            }
        }
        let result = binding
            .bridge
            .call("account/rateLimits/read", json!({}))
            .await;
        match result {
            Ok(mut value) => {
                if binding.token_key.is_empty()
                    || binding.account_id != account_id
                    || value
                        .get("accountId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id != account_id)
                {
                    binding.snapshot = None;
                    return Err("用量账号未核验或与当前模型账号不一致，请连接相同账号".into());
                }
                if !value.is_object() {
                    return Err("用量响应无效".into());
                }
                value.as_object_mut().unwrap().remove("accountId");
                value["status"] = json!("fresh");
                value["updatedAt"] = json!(now());
                binding.snapshot = Some(value.clone());
                binding.snapshot_at = now();
                Ok(value)
            }
            Err(error) => {
                if let Some(mut snapshot) = binding.snapshot.clone() {
                    snapshot["status"] = json!("stale");
                    snapshot["error"] = json!(error);
                    Ok(snapshot)
                } else {
                    Err(error)
                }
            }
        }
    }
    pub async fn handle_authenticated(
        &self,
        scope: &str,
        account_id: &str,
        access_token: &str,
        action: &str,
        body: &Value,
        refresh: Option<TokenRefresher>,
    ) -> Result<Value, String> {
        if scope.is_empty() || account_id.is_empty() || access_token.is_empty() {
            return Err("当前模型账号缺少可用的登录身份，请先登录模型账号".into());
        }
        if action == "reset-consume" && body.get("confirmed") != Some(&Value::Bool(true)) {
            return Err("使用重置卡前必须由用户确认".into());
        }
        let mut guard = self.binding.lock().await;
        let result = self
            .handle_locked(
                &mut guard,
                scope,
                account_id,
                access_token,
                action,
                body,
                refresh,
            )
            .await;
        if action != "reset-status" {
            if let Some(binding) = guard.as_mut() {
                binding.last_used = tokio::time::Instant::now();
            }
        }
        result
    }

    async fn handle_locked(
        &self,
        guard: &mut Option<Binding>,
        scope: &str,
        account_id: &str,
        access_token: &str,
        action: &str,
        body: &Value,
        refresh: Option<TokenRefresher>,
    ) -> Result<Value, String> {
        let home = self.directory(scope);
        if action == "reset-status" {
            return Ok(
                json!({"operation":Self::operation(&home.join("reset-operation.json")).await?.filter(|op|op.account_id==account_id).map(|op|op.view())}),
            );
        }
        if guard
            .as_ref()
            .is_some_and(|b| b.scope != scope || b.account_id != account_id)
        {
            if let Some(mut old) = guard.take() {
                old.bridge.stop().await;
            }
        }
        if guard
            .as_mut()
            .is_some_and(|binding| !binding.bridge.alive())
        {
            if let Some(mut previous) = guard.take() {
                previous.bridge.stop().await;
            }
        }
        if guard.is_none() {
            tokio::fs::create_dir_all(&home)
                .await
                .map_err(|_| "无法创建账户目录")?;
            let epoch = uuid::Uuid::new_v4();
            *guard = Some(Binding {
                epoch,
                last_used: tokio::time::Instant::now(),
                scope: scope.into(),
                account_id: account_id.into(),
                bridge: (self.connector)(home.clone()).await?,
                snapshot: None,
                snapshot_at: 0,
                token_key: String::new(),
            });
            self.expire_idle(epoch);
        }
        let binding = guard.as_mut().unwrap();
        binding.last_used = tokio::time::Instant::now();
        binding.bridge.set_refresher(account_id, refresh);
        let token_key = crate::provider_auth_catalog::key(access_token);
        if action == "usage-login" {
            binding.token_key.clear();
        }
        if binding.token_key != token_key {
            binding.snapshot_at = 0;
            binding.token_key.clear();
            let result=binding.bridge.call("account/login/start",json!({"type":"chatgptAuthTokens","accessToken":access_token,"chatgptAccountId":account_id,"chatgptPlanType":null})).await?;
            if result.get("type").and_then(Value::as_str) != Some("chatgptAuthTokens") {
                return Err("Codex 账户服务未确认当前模型登录身份，请更新官方客户端".into());
            }
            binding.token_key = token_key;
        }
        let operation_path = home.join("reset-operation.json");
        let _operation_lock = if action.starts_with("reset-") {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(home.join("reset-operation.lock"))
                .map_err(|_| "无法锁定兑换记录")?;
            file.try_lock()
                .map_err(|_| "另一项兑换操作正在处理，请稍后核对")?;
            Some(file)
        } else {
            None
        };
        if action == "reset-status" {
            return Ok(
                json!({"operation":Self::operation(&operation_path).await?.filter(|op|op.account_id==account_id).map(|op|op.view())}),
            );
        }
        let usage = Self::read_usage(
            binding,
            account_id,
            action != "usage" || body.get("refresh") == Some(&Value::Bool(true)),
        )
        .await?;
        if matches!(action, "usage" | "usage-login") {
            let mut value = usage;
            value["operation"] = Self::operation(&operation_path)
                .await?
                .filter(|op| op.account_id == account_id)
                .map(|op| op.view())
                .unwrap_or(Value::Null);
            return Ok(value);
        }
        if usage.get("status").and_then(Value::as_str) != Some("fresh") {
            return Err("请先刷新并核验当前账号用量".into());
        }
        if action == "usage-history" {
            return binding.bridge.call("account/usage/read", json!({})).await;
        }
        let old = Self::operation(&operation_path).await?;
        if action == "reset-prepare" {
            if let Some(operation) = old.as_ref().filter(|op| op.unsettled()) {
                if operation.account_id != account_id {
                    return Err("存在其他账户的未结算兑换".into());
                }
                return Ok(json!({"operation":operation.view()}));
            }
            if usage
                .pointer("/rateLimitResetCredits/availableCount")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                == 0
            {
                return Err("当前没有已确认可用的重置卡".into());
            }
            let credit_id = body
                .get("creditId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(id) = &credit_id {
                if id.is_empty()
                    || !usage
                        .pointer("/rateLimitResetCredits/credits")
                        .and_then(Value::as_array)
                        .is_some_and(|cards| {
                            cards.iter().any(|card| {
                                card.get("id").and_then(Value::as_str) == Some(id)
                                    && card.get("status").and_then(Value::as_str)
                                        == Some("available")
                            })
                        })
                {
                    return Err("重置卡不在当前账户可用列表中".into());
                }
            }
            let operation = ResetOperation {
                operation_id: uuid::Uuid::new_v4().to_string(),
                account_id: account_id.into(),
                credit_id,
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                state: "prepared".into(),
                outcome: None,
                created_at: now(),
            };
            Self::save_operation(&operation_path, &operation).await?;
            return Ok(json!({"operation":operation.view()}));
        }
        if action != "reset-consume" {
            return Err("未知账户操作".into());
        }
        let mut operation = old.ok_or("请先准备本次重置操作")?;
        if operation.account_id != account_id
            || body.get("operationId").and_then(Value::as_str) != Some(&operation.operation_id)
        {
            return Err("兑换操作与当前账户不匹配".into());
        }
        if !operation.unsettled() {
            return Ok(json!({"operation":operation.view(),"usage":usage}));
        }
        operation.state = "sending".into();
        Self::save_operation(&operation_path, &operation).await?;
        let result = binding
            .bridge
            .call(
                "account/rateLimitResetCredit/consume",
                json!({"idempotencyKey":operation.idempotency_key,"creditId":operation.credit_id}),
            )
            .await;
        let error = operation.settle(result);
        Self::save_operation(&operation_path, &operation).await?;
        binding.snapshot_at = 0;
        let refreshed = Self::read_usage(binding, account_id, true).await.ok();
        Ok(json!({"operation":operation.view(),"usage":refreshed,"error":error}))
    }
    #[cfg(test)]
    async fn handle(
        &self,
        scope: &str,
        account_id: &str,
        action: &str,
        body: &Value,
    ) -> Result<Value, String> {
        self.handle_authenticated(
            scope,
            account_id,
            "fixture-account-token",
            action,
            body,
            None,
        )
        .await
    }
}

fn merge_rate_limit(snapshot: &mut Value, update: &Value) {
    let Some(id) = update.get("limitId").and_then(Value::as_str) else {
        return;
    };
    let merge = |target: &mut Value| {
        if !target.is_object() {
            *target = json!({});
        }
        if let Some(fields) = update.as_object() {
            for (name, value) in fields {
                if !value.is_null() {
                    target[name] = value.clone();
                }
            }
        }
    };
    if !snapshot["rateLimitsByLimitId"].is_object() {
        snapshot["rateLimitsByLimitId"] = json!({});
    }
    merge(&mut snapshot["rateLimitsByLimitId"][id]);
    if snapshot
        .pointer("/rateLimits/limitId")
        .and_then(Value::as_str)
        == Some(id)
    {
        merge(&mut snapshot["rateLimits"]);
    }
    snapshot["updatedAt"] = json!(now());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_failures_keep_distinct_safe_auth_policy_rate_and_transport_messages() {
        for (error, expected) in [
            (
                json!({"code":-32602,"message":"codex account authentication required to read rate limits"}),
                "尚未接入",
            ),
            (json!({"code":-32601,"message":"private payload"}), "不支持"),
            (
                json!({"code":-32603,"message":"HTTP 401 Unauthorized private-token"}),
                "HTTP 401",
            ),
            (
                json!({"code":-32603,"message":"HTTP 403 private-token"}),
                "HTTP 403",
            ),
            (
                json!({"code":-32603,"data":{"statusCode":429,"token":"private-token"}}),
                "HTTP 429",
            ),
            (
                json!({"code":-32603,"message":"HTTP 503 private-token"}),
                "HTTP 503",
            ),
            (
                json!({"code":-32602,"message":"External auth must use one of workspace(s) private-workspace"}),
                "账户策略",
            ),
        ] {
            let message = account_rpc_error(&error);
            assert!(message.contains(expected));
            assert!(!message.contains("private"));
        }
    }

    #[tokio::test]
    async fn external_refresh_is_same_account_once_only_and_never_echoes_rejected_tokens() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let refresh: TokenRefresher = std::sync::Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(AccountTokens {
                    access_token: "fixture-refreshed-token".into(),
                    account_id: "account-a".into(),
                })
            })
        });
        let binding = ("account-a".into(), refresh);
        let mut attempted = false;
        assert!(
            refresh_external_tokens(Some(&binding), "account-b", &mut attempted)
                .await
                .is_err()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let value = refresh_external_tokens(Some(&binding), "account-a", &mut attempted)
            .await
            .unwrap();
        assert_eq!(value["chatgptAccountId"], "account-a");
        assert_eq!(value["accessToken"], "fixture-refreshed-token");
        assert!(
            refresh_external_tokens(Some(&binding), "account-a", &mut attempted)
                .await
                .is_err()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let wrong: TokenRefresher = std::sync::Arc::new(|| {
            Box::pin(async {
                Ok(AccountTokens {
                    access_token: "private-token".into(),
                    account_id: "account-b".into(),
                })
            })
        });
        let mut attempted = false;
        let error = refresh_external_tokens(
            Some(&("account-a".into(), wrong)),
            "account-a",
            &mut attempted,
        )
        .await
        .unwrap_err();
        assert!(!error.contains("private-token"));
    }

    #[tokio::test]
    async fn current_model_tokens_bind_before_quota_and_official_reply_needs_no_account_id() {
        let root = std::env::temp_dir().join(format!("dsh-account-bind-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        let value = service
            .handle("scope-a", "account-a", "usage", &json!({}))
            .await
            .unwrap();
        assert_eq!(value["status"], "fresh");
        assert!(value.get("accountId").is_none());
        assert_eq!(
            state.lock().unwrap().calls,
            ["account/login/start", "account/rateLimits/read"]
        );
        {
            let mut state = state.lock().unwrap();
            state.notices.push(
                json!({"method":"account/updated","params":{"authMode":"chatgptAuthTokens"}}),
            );
            state.notices.push(json!({"method":"account/login/completed","params":{"success":true,"loginId":null}}));
        }
        service
            .handle("scope-a", "account-a", "usage", &json!({}))
            .await
            .unwrap();
        assert_eq!(
            state.lock().unwrap().calls.len(),
            2,
            "unforced refresh uses the current account cache"
        );
        state.lock().unwrap().offline = true;
        let stale = service
            .handle("scope-a", "account-a", "usage-login", &json!({}))
            .await
            .unwrap();
        assert_eq!(stale["status"], "stale");
        assert_eq!(stale["rateLimits"], value["rateLimits"]);
        assert!(stale.get("verificationUrl").is_none());
        assert!(!service.directory("scope-a").join("auth.json").exists());
        drop(service);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn unconfirmed_external_auth_never_reads_or_publishes_quota() {
        let root = std::env::temp_dir().join(format!("dsh-account-bind-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            login_type: Some("chatgptDeviceCode".into()),
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        assert!(
            service
                .handle("scope-a", "account-a", "usage", &json!({}))
                .await
                .is_err()
        );
        assert_eq!(state.lock().unwrap().calls, ["account/login/start"]);
        drop(service);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn framed_account_reader_is_bounded_and_preserves_following_messages() {
        let wire = b"{\"method\":\"account/updated\"}\n{\"id\":2,\"result\":{}}\n";
        let mut reader = BufReader::with_capacity(3, &wire[..]);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().unwrap()["method"],
            "account/updated"
        );
        assert_eq!(read_frame(&mut reader).await.unwrap().unwrap()["id"], 2);
        let oversized = vec![b'x'; 8 * 1024 * 1024 + 1];
        let mut reader = BufReader::new(&oversized[..]);
        assert!(read_frame(&mut reader).await.unwrap_err().contains("限制"));
    }
    #[test]
    fn rolling_limit_updates_preserve_other_buckets_and_unknown_metadata() {
        let mut snapshot = json!({"rateLimitsByLimitId":{"codex":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":20},"secondary":{"usedPercent":30}},"spark":{"primary":{"usedPercent":5}}}});
        merge_rate_limit(
            &mut snapshot,
            &json!({"limitId":"codex","limitName":null,"primary":{"usedPercent":40},"secondary":null}),
        );
        assert_eq!(
            snapshot["rateLimitsByLimitId"]["codex"]["primary"]["usedPercent"],
            40
        );
        assert_eq!(
            snapshot["rateLimitsByLimitId"]["codex"]["secondary"]["usedPercent"],
            30
        );
        assert_eq!(
            snapshot["rateLimitsByLimitId"]["spark"]["primary"]["usedPercent"],
            5
        );
        assert_eq!(
            snapshot["rateLimitsByLimitId"]["codex"]["limitName"],
            "Codex"
        );
    }
    #[derive(Default)]
    struct FakeState {
        stops: usize,
        keys: std::collections::HashSet<String>,
        lose_response: bool,
        fail_refresh: bool,
        account: String,
        calls: Vec<String>,
        offline: bool,
        login_type: Option<String>,
        notices: Vec<Value>,
    }
    struct FakeRpc(std::sync::Arc<std::sync::Mutex<FakeState>>);
    #[async_trait::async_trait]
    impl AccountRpc for FakeRpc {
        async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            let mut state = self.0.lock().unwrap();
            state.calls.push(method.into());
            match method {
                "account/login/start" => {
                    if params["type"] != "chatgptAuthTokens"
                        || params["chatgptAccountId"] != state.account
                    {
                        return Err("fixture account mismatch".into());
                    }
                    state.notices.push(json!({"method":"account/updated","params":{"authMode":"chatgptAuthTokens"}}));
                    state.notices.push(json!({"method":"account/login/completed","params":{"success":true,"loginId":null}}));
                    Ok(json!({"type":state.login_type.as_deref().unwrap_or("chatgptAuthTokens")}))
                }
                "account/rateLimits/read" => {
                    if state.offline || state.fail_refresh && !state.keys.is_empty() {
                        return Err("offline".into());
                    }
                    Ok(
                        json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":75,"windowDurationMins":300,"resetsAt":null}},"rateLimitResetCredits":{"availableCount":2-state.keys.len(),"credits":null}}),
                    )
                }
                "account/rateLimitResetCredit/consume" => {
                    let key = params["idempotencyKey"].as_str().unwrap().to_string();
                    if !state.keys.insert(key) {
                        return Ok(json!({"outcome":"alreadyRedeemed"}));
                    }
                    if state.lose_response {
                        state.lose_response = false;
                        return Err("response lost after redemption".into());
                    }
                    Ok(json!({"outcome":"reset"}))
                }
                _ => Err("unexpected method".into()),
            }
        }
        async fn stop(&mut self) {
            self.0.lock().unwrap().stops += 1;
        }
        fn notifications(&mut self) -> Vec<Value> {
            self.0.lock().unwrap().notices.drain(..).collect()
        }
    }
    fn fake_service(
        root: PathBuf,
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    ) -> CodexAccountService {
        CodexAccountService {
            root,
            binding: std::sync::Arc::new(Mutex::new(None)),
            connector: std::sync::Arc::new(move |_| {
                let state = state.clone();
                Box::pin(async move { Ok(Box::new(FakeRpc(state)) as Box<dyn AccountRpc>) })
            }),
        }
    }
    #[tokio::test]
    async fn reset_requires_confirmation_before_connecting_or_redeeming() {
        let root = std::env::temp_dir().join(format!("dsh-reset-confirm-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        for body in [
            json!({}),
            json!({"confirmed":false}),
            json!({"confirmed":"true"}),
        ] {
            let error = service
                .handle("scope-a", "account-a", "reset-consume", &body)
                .await
                .unwrap_err();
            assert!(error.contains("确认"));
        }
        assert!(state.lock().unwrap().keys.is_empty());
        assert!(
            !root.exists(),
            "unconfirmed requests must not initialize an account connection"
        );
    }

    #[tokio::test]
    async fn idle_usage_helper_stops_without_logging_out_or_redeeming_and_reconnects() {
        let root = std::env::temp_dir().join(format!("dsh-usage-idle-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        service
            .handle("scope-a", "account-a", "usage", &json!({}))
            .await
            .unwrap();
        let epoch = service.binding.lock().await.as_ref().unwrap().epoch;
        assert!(
            CodexAccountService::release_idle(&service.binding, epoch)
                .await
                .is_some()
        );
        assert_eq!(state.lock().unwrap().stops, 0);
        service.binding.lock().await.as_mut().unwrap().last_used =
            tokio::time::Instant::now() - Duration::from_secs(61);
        let calls = state.lock().unwrap().calls.clone();
        assert!(
            CodexAccountService::release_idle(&service.binding, epoch)
                .await
                .is_none()
        );
        assert!(service.binding.lock().await.is_none());
        assert_eq!(state.lock().unwrap().stops, 1);
        assert_eq!(
            state.lock().unwrap().calls,
            calls,
            "idle cleanup sends no account operation"
        );
        assert!(state.lock().unwrap().keys.is_empty());
        let again = service
            .handle("scope-a", "account-a", "usage", &json!({}))
            .await
            .unwrap();
        assert_eq!(again["status"], "fresh");
        assert!(
            CodexAccountService::release_idle(&service.binding, epoch)
                .await
                .is_none()
        );
        assert!(
            service.binding.lock().await.is_some(),
            "an old deadline cannot stop a new binding"
        );
        drop(service);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn account_reset_reconnect_reuses_persisted_key_and_never_double_redeems() {
        let root = std::env::temp_dir().join(format!("dsh-reset-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            lose_response: true,
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        let prepared = service
            .handle("scope-a", "account-a", "reset-prepare", &json!({}))
            .await
            .unwrap();
        let again = service
            .handle("scope-a", "account-a", "reset-prepare", &json!({}))
            .await
            .unwrap();
        assert_eq!(prepared, again);
        let body = json!({"operationId":prepared["operation"]["operationId"],"confirmed":true});
        let result = service
            .handle("scope-a", "account-a", "reset-consume", &body)
            .await
            .unwrap();
        assert_eq!(result["operation"]["state"], "unknown");
        drop(service);
        let resumed = fake_service(root.clone(), state.clone());
        let result = resumed
            .handle("scope-a", "account-a", "reset-consume", &body)
            .await
            .unwrap();
        assert_eq!(result["operation"]["outcome"], "alreadyRedeemed");
        resumed
            .handle("scope-a", "account-a", "reset-consume", &body)
            .await
            .unwrap();
        assert_eq!(state.lock().unwrap().keys.len(), 1);
        assert!(
            resumed
                .handle("scope-a", "account-b", "reset-consume", &body)
                .await
                .is_err()
        );
        assert_eq!(state.lock().unwrap().keys.len(), 1);
        drop(resumed);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn successful_reset_survives_usage_refresh_failure() {
        let root = std::env::temp_dir().join(format!("dsh-reset-{}", uuid::Uuid::new_v4()));
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeState {
            account: "account-a".into(),
            fail_refresh: true,
            ..Default::default()
        }));
        let service = fake_service(root.clone(), state.clone());
        let prepared = service
            .handle("scope-a", "account-a", "reset-prepare", &json!({}))
            .await
            .unwrap();
        let result = service
            .handle(
                "scope-a",
                "account-a",
                "reset-consume",
                &json!({"operationId":prepared["operation"]["operationId"],"confirmed":true}),
            )
            .await
            .unwrap();
        assert_eq!(result["operation"]["state"], "succeeded");
        assert_eq!(result["usage"]["status"], "stale");
        assert_eq!(state.lock().unwrap().keys.len(), 1);
        drop(service);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    fn attempt() -> ResetOperation {
        ResetOperation {
            operation_id: "op".into(),
            account_id: "account-a".into(),
            credit_id: None,
            idempotency_key: "fixed-key".into(),
            state: "sending".into(),
            outcome: None,
            created_at: 0,
        }
    }
    #[test]
    fn redemption_outcomes_and_unknown_retries_preserve_identity() {
        for (outcome, state) in [
            ("reset", "succeeded"),
            ("alreadyRedeemed", "succeeded"),
            ("noCredit", "notApplied"),
            ("nothingToReset", "notApplied"),
        ] {
            let mut operation = attempt();
            assert!(operation.settle(Ok(json!({"outcome":outcome}))).is_none());
            assert_eq!(operation.state, state);
            assert!(!operation.unsettled());
        }
        let mut operation = attempt();
        operation.settle(Err("lost response".into()));
        assert_eq!(operation.state, "unknown");
        assert!(operation.unsettled());
        assert_eq!(operation.idempotency_key, "fixed-key");
        operation.settle(Ok(json!({"outcome":"alreadyRedeemed"})));
        assert_eq!(operation.state, "succeeded");
    }
}
