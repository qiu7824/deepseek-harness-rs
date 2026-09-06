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
                            Err(
                                if error.get("code").and_then(Value::as_i64) == Some(-32601) {
                                    "此 Codex CLI 版本不支持该账户功能"
                                } else {
                                    "账户服务拒绝请求；请检查登录状态或稍后重试"
                                }
                                .into(),
                            )
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
        };
        bridge.rpc("initialize",json!({"clientInfo":{"name":"dsh-account","title":"DeepSeek Harness","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}})).await?;
        tokio::time::timeout(
            Duration::from_secs(5),
            write_frame(&bridge.input, json!({"method":"initialized","params":{}})),
        )
        .await
        .map_err(|_| "账户初始化超时")??;
        Ok(bridge)
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().insert(id, tx);
        let result = tokio::time::timeout(Duration::from_secs(35), async {
            write_frame(
                &self.input,
                json!({"id":id,"method":method,"params":params}),
            )
            .await?;
            rx.await.map_err(|_| "账户连接已关闭".to_string())?
        })
        .await
        .map_err(|_| "账户服务响应超时".to_string())
        .and_then(|value| value);
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
    scope: String,
    account_id: String,
    bridge: Box<dyn AccountRpc>,
    snapshot: Option<Value>,
    snapshot_at: u64,
}
pub(crate) struct CodexAccountService {
    root: PathBuf,
    binding: Mutex<Option<Binding>>,
    connector: Connector,
}
impl CodexAccountService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            binding: Mutex::new(None),
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
                Some(
                    "account/updated" | "account/login/completed" | "account/state/invalidated",
                ) => {
                    binding.snapshot = None;
                    binding.snapshot_at = 0;
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
                if value.get("accountId").and_then(Value::as_str) != Some(account_id) {
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
    pub async fn handle(
        &self,
        scope: &str,
        account_id: &str,
        action: &str,
        body: &Value,
    ) -> Result<Value, String> {
        let mut guard = self.binding.lock().await;
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
        if guard.is_none() {
            tokio::fs::create_dir_all(&home)
                .await
                .map_err(|_| "无法创建账户目录")?;
            *guard = Some(Binding {
                scope: scope.into(),
                account_id: account_id.into(),
                bridge: (self.connector)(home.clone()).await?,
                snapshot: None,
                snapshot_at: 0,
            });
        }
        let binding = guard.as_mut().unwrap();
        if !binding.bridge.alive() {
            *guard = None;
            return Err("账户服务已退出，请重新连接".into());
        }
        if action == "usage-login" {
            binding.snapshot = None;
            return binding
                .bridge
                .call("account/login/start", json!({"type":"chatgptDeviceCode"}))
                .await;
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
        if action == "usage" {
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
        keys: std::collections::HashSet<String>,
        lose_response: bool,
        fail_refresh: bool,
        account: String,
    }
    struct FakeRpc(std::sync::Arc<std::sync::Mutex<FakeState>>);
    #[async_trait::async_trait]
    impl AccountRpc for FakeRpc {
        async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            let mut state = self.0.lock().unwrap();
            match method {
                "account/rateLimits/read" => {
                    if state.fail_refresh && !state.keys.is_empty() {
                        return Err("offline".into());
                    }
                    Ok(
                        json!({"accountId":state.account,"rateLimits":{"limitId":"codex","primary":{"usedPercent":75,"windowDurationMins":300,"resetsAt":null}},"rateLimitResetCredits":{"availableCount":2-state.keys.len(),"credits":null}}),
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
        async fn stop(&mut self) {}
    }
    fn fake_service(
        root: PathBuf,
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    ) -> CodexAccountService {
        CodexAccountService {
            root,
            binding: Mutex::new(None),
            connector: std::sync::Arc::new(move |_| {
                let state = state.clone();
                Box::pin(async move { Ok(Box::new(FakeRpc(state)) as Box<dyn AccountRpc>) })
            }),
        }
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
        let body = json!({"operationId":prepared["operation"]["operationId"]});
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
                &json!({"operationId":prepared["operation"]["operationId"]}),
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
