use super::*;
use dsh_code_runtime::CodeBindingNamespace;
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessHandle, SubprocessOutcome,
    SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};
use futures::{future::BoxFuture, task::AtomicWaker};
use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const BUDGET_MS: u64 = 1000;

#[derive(Default)]
struct Events {
    frames: parking_lot::Mutex<Vec<u8>>,
    binding_started: AtomicBool,
    binding_dropped: AtomicBool,
    eof_observed: AtomicBool,
    terminated: AtomicBool,
    reaps: AtomicUsize,
    reader: AtomicWaker,
    at_deadline: parking_lot::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

struct Writer(Arc<Events>);
impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.0.frames.lock().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct DeadlineEof {
    frames: std::io::Cursor<Vec<u8>>,
    events: Arc<Events>,
    signal: SubprocessAbort,
}
impl AsyncRead for DeadlineEof {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.frames.position() < self.frames.get_ref().len() as u64 {
            return Pin::new(&mut self.frames).poll_read(cx, output);
        }
        self.events.reader.register(cx.waker());
        if !self.events.binding_started.load(Ordering::SeqCst) {
            return Poll::Pending;
        }
        if !self.events.eof_observed.swap(true, Ordering::SeqCst) {
            // Hold this one run-future poll across the deadline, then return
            // EOF before the outer timer can be polled. This forces the real
            // subprocess-EOF Abort branch, rather than relying on scheduling
            // or merely checking the final classification predicate.
            std::thread::sleep(Duration::from_millis(BUDGET_MS + 1));
            if let Some(action) = self.events.at_deadline.lock().take() {
                action();
            }
            assert!((self.signal)(), "the child's combined signal must be set");
        }
        Poll::Ready(Ok(()))
    }
}

struct Child {
    events: Arc<Events>,
    signal: SubprocessAbort,
}
impl SubprocessHandle for Child {
    fn stdin(&self) -> Option<Box<dyn AsyncWrite + Unpin + Send>> {
        Some(Box::new(Writer(self.events.clone())))
    }
    fn stdout(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        Some(Box::new(DeadlineEof {
            frames: std::io::Cursor::new(
                b"{\"type\":\"ready\"}\n{\"type\":\"binding_call\",\"id\":1,\"global\":\"host\",\"name\":\"wait\",\"args\":{}}\n".to_vec(),
            ),
            events: self.events.clone(),
            signal: self.signal.clone(),
        }))
    }
    fn stderr(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        None
    }
    fn collected(&self) -> SubprocessCollectedOutputs {
        Default::default()
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        Box::pin(async { std::future::pending().await })
    }
    fn terminate(&self) {
        self.events.terminated.store(true, Ordering::SeqCst);
    }
    fn wait_for_exit(&self, _: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        let events = self.events.clone();
        Box::pin(async move {
            assert!(events.terminated.load(Ordering::SeqCst));
            events.reaps.fetch_add(1, Ordering::SeqCst);
            true
        })
    }
}

struct Subprocess(Arc<Events>);
impl SubprocessRuntime for Subprocess {
    fn resolve_executable(
        &self,
        _: &str,
        _: Option<&[(String, String)]>,
        _: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Ok("fixture-node".into()) })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        assert_eq!(spec.stdio.stdin, SubprocessStdinMode::Pipe);
        assert_eq!(spec.stdio.stdout, SubprocessOutputMode::Pipe);
        Ok(Arc::new(Child {
            events: self.0.clone(),
            signal: spec
                .signal
                .expect("runtime must supply its combined signal"),
        }))
    }
    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async { Err("unexpected terminal".into()) })
    }
}

struct PendingBinding(Arc<Events>);
impl Drop for PendingBinding {
    fn drop(&mut self) {
        self.0.binding_dropped.store(true, Ordering::SeqCst);
    }
}

enum Cancellation {
    None,
    Stop,
    Dispose,
}

async fn run_deadline_eof(cancellation: Cancellation, expected: CodeRunFailureKind) {
    let events = Arc::new(Events::default());
    let ctx = Context::root();
    ctx.register_service(Arc::new(Subprocess(events.clone())) as Arc<dyn SubprocessRuntime>);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    match cancellation {
        Cancellation::None => {}
        Cancellation::Stop => {
            let stopped = stopped.clone();
            *events.at_deadline.lock() = Some(Box::new(move || {
                stopped.store(true, Ordering::SeqCst);
            }));
        }
        Cancellation::Dispose => {
            let runtime = runtime.clone();
            *events.at_deadline.lock() = Some(Box::new(move || {
                // This fixture's termination and reap complete immediately,
                // so invoke the real disposal before returning child EOF.
                assert!(runtime.dispose().now_or_never().is_some());
            }));
        }
    }
    let dispatched = Arc::new(AtomicUsize::new(0));
    let dispatches = dispatched.clone();
    let binding = events.clone();
    let flag = stopped.clone();
    let result = runtime
        .run(CodeRunRequest {
            timeout_ms: Some(BUDGET_MS),
            on_dispatch: Some(Arc::new(move || {
                dispatches.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })),
            program: "await host.wait({});".into(),
            signal: Some(Arc::new(move || flag.load(Ordering::SeqCst))),
            bindings: vec![CodeBindingNamespace {
                global: "host".into(),
                error_class: None,
                functions: vec![(
                    "wait".into(),
                    Arc::new(move |_| {
                        let binding = binding.clone();
                        Box::pin(async move {
                            let _pending = PendingBinding(binding.clone());
                            binding.binding_started.store(true, Ordering::SeqCst);
                            binding.reader.wake();
                            std::future::pending::<Result<Value, String>>().await
                        })
                    }),
                )],
            }],
        })
        .await
        .unwrap();
    runtime.dispose().await;
    tokio::task::yield_now().await;
    assert_eq!(result.error.unwrap().kind, expected);
    assert_eq!(dispatched.load(Ordering::SeqCst), 1);
    assert!(events.eof_observed.load(Ordering::SeqCst));
    assert!(events.binding_dropped.load(Ordering::SeqCst));
    assert!(events.terminated.load(Ordering::SeqCst));
    assert!(events.reaps.load(Ordering::SeqCst) > 0);
    assert!(runtime.lifecycle.state.lock().active.is_empty());
    let frames = events.frames.lock();
    let frames: Vec<Value> = frames
        .split(|byte| *byte == b'\n')
        .filter(|frame| !frame.is_empty())
        .map(|frame| serde_json::from_slice(frame).unwrap())
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["type"], "prepare");
    assert_eq!(frames[1]["type"], "run");
}

#[tokio::test]
async fn internal_deadline_child_eof_is_timeout_and_reaps_pending_binding() {
    run_deadline_eof(Cancellation::None, CodeRunFailureKind::Timeout).await;
}

#[tokio::test]
async fn caller_stop_wins_when_deadline_and_child_eof_arrive_together() {
    run_deadline_eof(Cancellation::Stop, CodeRunFailureKind::Abort).await;
}

#[tokio::test]
async fn runtime_disposal_wins_when_deadline_and_child_eof_arrive_together() {
    run_deadline_eof(Cancellation::Dispose, CodeRunFailureKind::Abort).await;
}
