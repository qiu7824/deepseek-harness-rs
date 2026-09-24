//! Human-owned terminals survive agent retirement and never enter model PTY storage.
use cordis::{Context, make_disposer};
use dsh_session::SessionId;
use dsh_terminal::*;
use dsh_terminal_bash::{ShellTerminalBackend, UserTerminalSpawnSpec};
use futures::{FutureExt, future::BoxFuture};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Factory = Arc<
    dyn Fn(
            UserTerminalSpawnSpec,
        )
            -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, TerminalBackendSpawnError>>
        + Send
        + Sync,
>;

struct Pending {
    cancelled: AtomicBool,
    changed: tokio::sync::Notify,
    done: tokio::sync::watch::Sender<bool>,
}
impl Pending {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            changed: Default::default(),
            done: tokio::sync::watch::channel(false).0,
        }
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    async fn cancelled(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}
struct Admission {
    owner: SessionId,
    name: Option<String>,
    pending: Arc<Pending>,
}
struct Entry {
    owner: SessionId,
    name: Option<String>,
    order: u64,
    backend: Arc<dyn TerminalBackendSession>,
}
#[derive(Default)]
struct State {
    disposing: bool,
    next: u64,
    pending: HashMap<TerminalSessionId, Admission>,
    entries: HashMap<TerminalSessionId, Arc<Entry>>,
}
pub(super) struct UserTerminals {
    state: Mutex<State>,
    factory: Factory,
}
#[cfg(test)]
#[path = "user_terminal_tests.rs"]
mod tests;
struct RequestGuard(Option<Arc<Pending>>);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Some(pending) = &self.0 {
            pending.cancel();
        }
    }
}
struct FinishAdmission {
    service: Arc<UserTerminals>,
    id: TerminalSessionId,
    pending: Arc<Pending>,
}
impl Drop for FinishAdmission {
    fn drop(&mut self) {
        self.service.state.lock().pending.remove(&self.id);
        self.pending.done.send_replace(true);
    }
}

impl UserTerminals {
    pub(super) fn install(ctx: &Context, backend: Arc<ShellTerminalBackend>) -> Arc<Self> {
        Self::with_factory(ctx, Arc::new(move |spec| backend.spawn_user(spec)))
    }
    fn with_factory(ctx: &Context, factory: Factory) -> Arc<Self> {
        let service = Arc::new(Self {
            state: Mutex::new(State::default()),
            factory,
        });
        let teardown = service.clone();
        let _ = ctx.effect(
            "user terminal teardown",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let service = teardown.clone();
                    Box::pin(async move {
                        if let Err(error) = service.dispose_all().await {
                            eprintln!("dsh: user terminal cleanup failed: {error}");
                        }
                    })
                }))
            }),
        );
        service
    }

    pub(super) fn spawn_limited(
        self: &Arc<Self>,
        owner: SessionId,
        request: TerminalSpawnRequest,
        signal: Option<TerminalAbort>,
        limit: usize,
    ) -> Result<BoxFuture<'static, Result<TerminalSpawnResult, String>>, String> {
        if request.type_ != "shell" {
            return Err("用户终端类型不可用".into());
        }
        let cwd = request.cwd.ok_or("用户终端缺少工作区")?;
        let id = terminal_session_id(format!("user-{}", uuid::Uuid::new_v4()));
        let pending = Arc::new(Pending::new());
        let order = {
            let mut state = self.state.lock();
            if state.disposing {
                return Err("用户终端服务正在关闭".into());
            }
            let count = state
                .entries
                .values()
                .filter(|entry| entry.owner == owner)
                .count()
                + state
                    .pending
                    .iter()
                    .filter(|(id, entry)| entry.owner == owner && !state.entries.contains_key(*id))
                    .count();
            if count >= limit {
                return Err(format!("每个会话最多打开 {limit} 个用户终端"));
            }
            if request.name.is_some()
                && (state
                    .entries
                    .values()
                    .any(|entry| entry.owner == owner && entry.name == request.name)
                    || state
                        .pending
                        .values()
                        .any(|entry| entry.owner == owner && entry.name == request.name))
            {
                return Err("该终端名称已被使用".into());
            }
            let order = state.next;
            state.next += 1;
            state.pending.insert(
                id.clone(),
                Admission {
                    owner: owner.clone(),
                    name: request.name.clone(),
                    pending: pending.clone(),
                },
            );
            order
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (ack, acknowledged) = tokio::sync::oneshot::channel();
        let guard = RequestGuard(Some(pending.clone()));
        let own = self.clone();
        let factory = self.factory.clone();
        tokio::spawn(async move {
            let _completion = FinishAdmission {
                service: own.clone(),
                id: id.clone(),
                pending: pending.clone(),
            };
            let cancellation = pending.clone();
            let abort: TerminalAbort = Arc::new(move || {
                cancellation.cancelled.load(Ordering::Acquire)
                    || signal.as_ref().is_some_and(|signal| signal())
            });
            let result = std::panic::AssertUnwindSafe(factory(UserTerminalSpawnSpec {
                terminal_id: id.clone(),
                session_id: owner.to_string(),
                cwd,
                signal: abort.clone(),
            }))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(TerminalBackendSpawnError::spawn("用户终端启动发生异常")));
            match result {
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                }
                Ok(backend) => {
                    let entry = Arc::new(Entry {
                        owner,
                        name: request.name.clone(),
                        order,
                        backend,
                    });
                    let published = {
                        let mut state = own.state.lock();
                        if state.disposing
                            || abort()
                            || sender.is_closed()
                        {
                            false
                        } else {
                            state.entries.insert(id.clone(), entry.clone());
                            true
                        }
                    };
                    let accepted = if published {
                        let snapshot = TerminalSpawnResult {
                            execution_context_id: entry.backend.execution_context_id(),
                            session_id: id.clone(),
                            name: request.name,
                            type_: "shell".into(),
                            pid: entry.backend.pid(),
                            status: entry.backend.status(),
                            motd: entry.backend.motd(),
                        };
                        if sender.send(Ok(snapshot)).is_ok() {
                            tokio::select! { result = acknowledged => result.is_ok(), _ = pending.cancelled() => false }
                        } else {
                            false
                        }
                    } else {
                        let _ = sender.send(Err("用户终端启动已取消".into()));
                        false
                    };
                    if !accepted {
                        match entry
                            .backend
                            .close("User terminal request cancelled before acceptance")
                            .await
                        {
                            Ok(()) => {
                                own.state.lock().entries.remove(&id);
                            }
                            Err(error) => {
                                // Keep a failed cleanup visible and addressable for retry.
                                own.state.lock().entries.insert(id.clone(), entry);
                                eprintln!("dsh: cancelled user terminal {id} cleanup failed: {error}");
                            }
                        }
                    }
                }
            }
            own.state.lock().pending.remove(&id);
            pending.done.send_replace(true);
        });
        Ok(Box::pin(async move {
            let mut guard = guard;
            let result = receiver
                .await
                .map_err(|_| "用户终端启动任务已结束".to_string())?;
            if result.is_ok() {
                let _ = ack.send(());
                guard.0 = None;
            }
            result
        }))
    }

    fn find(&self, owner: &SessionId, id: &TerminalSessionId) -> Result<Arc<Entry>, String> {
        self.state
            .lock()
            .entries
            .get(id)
            .filter(|entry| &entry.owner == owner)
            .cloned()
            .ok_or_else(|| "用户终端不存在或不属于当前会话".into())
    }
    pub(super) fn list(&self, owner: &SessionId) -> Vec<TerminalSessionSnapshot> {
        let mut entries: Vec<_> = self
            .state
            .lock()
            .entries
            .iter()
            .filter(|(_, entry)| &entry.owner == owner)
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect();
        entries.sort_by_key(|(_, entry)| entry.order);
        entries
            .into_iter()
            .map(|(id, entry)| TerminalSessionSnapshot {
                execution_context_id: entry.backend.execution_context_id(),
                session_id: id,
                name: entry.name.clone(),
                type_: "shell".into(),
                pid: entry.backend.pid(),
                status: entry.backend.status(),
            })
            .collect()
    }
    pub(super) fn read(
        &self,
        owner: &SessionId,
        id: &TerminalSessionId,
        request: TerminalReadRequest,
    ) -> Result<TerminalReadResult, String> {
        Ok(self.find(owner, id)?.backend.read(&request))
    }
    pub(super) fn write_input(
        &self,
        owner: &SessionId,
        id: &TerminalSessionId,
        text: &str,
    ) -> Result<BoxFuture<'static, Result<(), String>>, String> {
        Ok(self.find(owner, id)?.backend.write_input(text))
    }
    pub(super) fn resize(
        &self,
        owner: &SessionId,
        id: &TerminalSessionId,
        rows: u16,
        cols: u16,
    ) -> Result<BoxFuture<'static, Result<(), String>>, String> {
        Ok(self.find(owner, id)?.backend.resize(rows, cols))
    }
    pub(super) fn signal(
        &self,
        owner: &SessionId,
        id: &TerminalSessionId,
        signal: TerminalSignal,
    ) -> Result<BoxFuture<'static, Result<TerminalSignalResult, String>>, String> {
        Ok(self.find(owner, id)?.backend.signal(signal))
    }
    pub(super) fn start_send(
        &self,
        owner: &SessionId,
        id: &TerminalSessionId,
        request: TerminalSendRequest,
    ) -> Result<Arc<dyn TerminalSendOperation>, String> {
        Ok(self.find(owner, id)?.backend.start_send(&request))
    }
    pub(super) fn kill(
        self: &Arc<Self>,
        owner: &SessionId,
        id: &TerminalSessionId,
        reason: String,
    ) -> Result<BoxFuture<'static, Result<bool, String>>, String> {
        let entry = self.find(owner, id)?;
        let id = id.clone();
        let own = self.clone();
        Ok(Box::pin(async move {
            entry.backend.close(&reason).await?;
            Ok(own.state.lock().entries.remove(&id).is_some())
        }))
    }
    async fn dispose_all(&self) -> Result<(), String> {
        let pending: Vec<_> = {
            let mut state = self.state.lock();
            state.disposing = true;
            state
                .pending
                .values()
                .map(|entry| entry.pending.clone())
                .collect()
        };
        for entry in &pending {
            entry.cancel();
        }
        for entry in pending {
            let mut done = entry.done.subscribe();
            let _ = done.wait_for(|done| *done).await;
        }
        let entries: Vec<_> = self
            .state
            .lock()
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect();
        let mut errors = Vec::new();
        for (id, entry) in entries {
            match entry.backend.close("User terminal service stopped").await {
                Ok(()) => {
                    self.state.lock().entries.remove(&id);
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
