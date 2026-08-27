#![allow(clippy::type_complexity)] // Cleanup state mirrors the shared async terminal lifecycle.

//! Native PTY allocation over `portable-pty` (ConPTY on Windows).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use dsh_subprocess::{
    SubprocessOutcome, SubprocessTerminalForeground, SubprocessTerminalHandle,
    SubprocessTerminalSignal, SubprocessTerminalSpawnSpec,
};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use futures::stream::{BoxStream, StreamExt};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::spawn::child_env;

type DoneFuture = Shared<BoxFuture<'static, Result<SubprocessOutcome, String>>>;
type ReaderDoneFuture = Shared<BoxFuture<'static, Result<(), String>>>;

/// One real native PTY process. `native_pty_system()` selects ConPTY on
/// Windows and the platform PTY implementation on POSIX.
pub struct PortableTerminalHandle {
    pid: u32,
    foreground_id: u32,
    grace_ms: u64,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    output_receiver: Mutex<Option<futures::channel::mpsc::UnboundedReceiver<Vec<u8>>>>,
    done: DoneFuture,
    reader_done: ReaderDoneFuture,
    exited: Arc<AtomicBool>,
    cleanup: Mutex<Option<Arc<tokio::sync::Mutex<Option<Result<(), String>>>>>>,
    self_arc: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl PortableTerminalHandle {
    pub fn spawn(spec: SubprocessTerminalSpawnSpec) -> Result<Arc<Self>, String> {
        let Some(_program) = spec.argv.first().filter(|program| !program.is_empty()) else {
            return Err("subprocess-local: terminal argv must contain a program".to_string());
        };
        if spec.rows == 0 || spec.cols == 0 {
            return Err("subprocess-local: terminal rows and cols must be positive".to_string());
        }
        if spec.grace_ms == 0 {
            return Err("subprocess-local: terminal graceMs must be positive".to_string());
        }
        if spec.signal.as_ref().is_some_and(|signal| signal()) {
            return Err("subprocess-local: terminal allocation aborted before spawn".to_string());
        }

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| {
                format!("subprocess-local: failed to allocate native PTY: {error:#}")
            })?;

        let argv: Vec<OsString> = spec.argv.iter().map(OsString::from).collect();
        let mut command = CommandBuilder::from_argv(argv);
        command.cwd(&spec.cwd);
        command.env_clear();
        let env = spec
            .env
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), Some(value.clone())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (key, value) in child_env(Some(&env)) {
            command.env(key, value);
        }

        let mut child = pair.slave.spawn_command(command).map_err(|error| {
            format!("subprocess-local: failed to spawn terminal argv: {error:#}")
        })?;
        let Some(pid) = child.process_id() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("subprocess-local: terminal child did not publish a pid".to_string());
        };
        let killer = child.clone_killer();
        let foreground_id = foreground_id(pair.master.as_ref(), pid);
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let mut killer = killer;
                let cleanup = killer.kill().err().map(|failure| failure.to_string());
                let _ = child.wait();
                return Err(match cleanup {
                    Some(cleanup) => format!(
                        "subprocess-local: failed to clone PTY reader ({error:#}); cleanup failed: {cleanup}"
                    ),
                    None => format!("subprocess-local: failed to clone PTY reader: {error:#}"),
                });
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let mut killer = killer;
                let cleanup = killer.kill().err().map(|failure| failure.to_string());
                let _ = child.wait();
                return Err(match cleanup {
                    Some(cleanup) => format!(
                        "subprocess-local: failed to take PTY writer ({error:#}); cleanup failed: {cleanup}"
                    ),
                    None => format!("subprocess-local: failed to take PTY writer: {error:#}"),
                });
            }
        };
        drop(pair.slave);

        #[cfg(windows)]
        {
            // portable-pty 0.9 creates ConPTY with
            // PSEUDOCONSOLE_INHERIT_CURSOR. Windows asks the host for the
            // inherited cursor position before releasing the child. A raw
            // transport has no terminal emulator to answer that query, so
            // seed the conventional home-position reply. Without it even a
            // one-shot `whoami.exe` remains STILL_ACTIVE indefinitely.
            if let Err(error) = writer.write_all(b"\x1b[1;1R").and_then(|()| writer.flush()) {
                let mut killer = killer;
                let cleanup = killer.kill().err().map(|failure| failure.to_string());
                let _ = child.wait();
                return Err(match cleanup {
                    Some(cleanup) => format!(
                        "subprocess-local: failed to answer ConPTY cursor query ({error}); cleanup failed: {cleanup}"
                    ),
                    None => {
                        format!("subprocess-local: failed to answer ConPTY cursor query: {error}")
                    }
                });
            }
        }

        let writer = Arc::new(Mutex::new(Some(writer)));
        let writer_for_reader = writer.clone();
        let (output_tx, output_rx) = futures::channel::mpsc::unbounded();
        let (reader_done_tx, reader_done_rx) = futures::channel::oneshot::channel();
        let mut reader_start_killer = killer.clone_killer();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("dsh-pty-reader-{pid}"))
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let mut buffer = vec![0u8; 16 * 1024];
                    let mut dsr_tail = Vec::<u8>::new();
                    loop {
                        match reader.read(&mut buffer) {
                            Ok(0) => return Ok(()),
                            Ok(count) => {
                                #[cfg(windows)]
                                {
                                    let mut probe = dsr_tail.clone();
                                    probe.extend_from_slice(&buffer[..count]);
                                    if probe.windows(4).any(|window| window == b"\x1b[6n") {
                                        let mut locked = writer_for_reader.lock();
                                        let writer = locked.as_mut().ok_or_else(|| {
                                            "terminal process is closing during cursor query"
                                                .to_string()
                                        })?;
                                        writer
                                            .write_all(b"\x1b[1;1R")
                                            .and_then(|()| writer.flush())
                                            .map_err(|error| {
                                                format!(
                                                    "failed to answer ConPTY cursor query: {error}"
                                                )
                                            })?;
                                    }
                                    dsr_tail = probe[probe.len().saturating_sub(3)..].to_vec();
                                }
                                if output_tx.unbounded_send(buffer[..count].to_vec()).is_err() {
                                    return Ok(());
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                                return Ok(());
                            }
                            Err(error) => {
                                return Err(format!("terminal output read failed: {error}"));
                            }
                        }
                    }
                })();
                let _ = reader_done_tx.send(result);
            })
        {
            let cleanup = reader_start_killer
                .kill()
                .err()
                .map(|failure| failure.to_string());
            let _ = child.wait();
            return Err(match cleanup {
                Some(cleanup) => format!(
                    "subprocess-local: failed to start PTY reader ({error}); cleanup failed: {cleanup}"
                ),
                None => format!("subprocess-local: failed to start PTY reader: {error}"),
            });
        }

        let exited = Arc::new(AtomicBool::new(false));
        let exited_waiter = exited.clone();
        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        let mut waiter_start_killer = killer.clone_killer();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("dsh-pty-wait-{pid}"))
            .spawn(move || {
                let outcome = child.wait().map_err(|error| {
                    format!("subprocess-local: terminal child wait failed: {error}")
                });
                exited_waiter.store(true, SeqCst);
                let outcome = outcome.map(|status| SubprocessOutcome {
                    exit_code: status.signal().is_none().then(|| status.exit_code() as i32),
                    signal: status.signal().map(str::to_string),
                });
                let _ = done_tx.send(outcome);
            })
        {
            let cleanup = waiter_start_killer
                .kill()
                .err()
                .map(|failure| failure.to_string());
            drop(pair.master);
            return Err(match cleanup {
                Some(cleanup) => format!(
                    "subprocess-local: failed to start PTY waiter ({error}); cleanup failed: {cleanup}"
                ),
                None => format!("subprocess-local: failed to start PTY waiter: {error}"),
            });
        }

        let done = async move {
            done_rx
                .await
                .map_err(|_| "subprocess-local: terminal waiter disappeared".to_string())?
        }
        .boxed()
        .shared();
        let reader_done = async move {
            reader_done_rx
                .await
                .map_err(|_| "subprocess-local: terminal reader disappeared".to_string())?
        }
        .boxed()
        .shared();

        let handle = Arc::new(Self {
            pid,
            foreground_id,
            grace_ms: spec.grace_ms,
            writer,
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(killer),
            output_receiver: Mutex::new(Some(output_rx)),
            done,
            reader_done,
            exited,
            cleanup: Mutex::new(None),
            self_arc: std::sync::OnceLock::new(),
        });
        handle
            .self_arc
            .set(Arc::downgrade(&handle))
            .expect("portable terminal self arc is set once");
        Ok(handle)
    }

    fn force_stop_tree(&self) {
        #[cfg(windows)]
        {
            // portable-pty's ConPTY killer may synchronously close the
            // pseudoconsole and wait for attached clients. Use the explicit
            // Windows tree primitive; ClosePseudoConsole is delegated below.
            crate::spawn::taskkill_process_tree(self.pid as i32);
        }
        #[cfg(not(windows))]
        {
            let _ = self.killer.lock().kill();
        }
    }

    pub fn terminate_for_host_exit(&self) {
        self.writer.lock().take();
        self.force_stop_tree();
        if let Some(master) = self.master.lock().take() {
            // ClosePseudoConsole can wait for attached clients. The process
            // tree has already been force-stopped; keep the synchronous Drop
            // fallback non-blocking even if Windows delays the close.
            let _ = std::thread::Builder::new()
                .name(format!("dsh-conpty-close-{}", self.pid))
                .spawn(move || drop(master));
        }
    }

    async fn close_once(self: &Arc<Self>) -> Result<(), String> {
        self.writer.lock().take();
        if !self.exited.load(SeqCst) {
            self.force_stop_tree();
        }
        let grace = std::time::Duration::from_millis(self.grace_ms);
        let close_master = self.master.lock().take().map(|master| {
            let (closed_tx, closed_rx) = futures::channel::oneshot::channel();
            let name = format!("dsh-conpty-close-{}", self.pid);
            std::thread::Builder::new()
                .name(name)
                .spawn(move || {
                    drop(master);
                    let _ = closed_tx.send(());
                })
                .map(|_| closed_rx)
                .map_err(|error| format!("terminal PTY close thread failed: {error}"))
        });
        tokio::time::timeout(grace, async {
            self.done.clone().await?;
            if let Some(close_master) = close_master {
                close_master?
                    .await
                    .map_err(|_| "terminal PTY close thread disappeared".to_string())?;
            }
            self.reader_done.clone().await?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|_| format!("terminal cleanup failed; surviving pid: {}", self.pid))?
    }
}

#[cfg(unix)]
fn foreground_id(master: &dyn MasterPty, pid: u32) -> u32 {
    master
        .process_group_leader()
        .map(|pid| pid as u32)
        .unwrap_or(pid)
}

#[cfg(windows)]
fn foreground_id(_master: &dyn MasterPty, pid: u32) -> u32 {
    pid
}

impl SubprocessTerminalHandle for PortableTerminalHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn output(&self) -> BoxStream<'static, Vec<u8>> {
        self.output_receiver
            .lock()
            .take()
            .map(|receiver| receiver.boxed())
            .unwrap_or_else(|| futures::stream::empty().boxed())
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        self.done.clone().boxed()
    }

    fn write(&self, data: &str) -> BoxFuture<'static, Result<(), String>> {
        let data = data.to_string();
        let handle = self.self_arc();
        Box::pin(async move {
            if handle.exited.load(SeqCst) {
                return Err("terminal process has exited".to_string());
            }
            let mut writer = handle.writer.lock();
            let writer = writer
                .as_mut()
                .ok_or_else(|| "terminal process is closing".to_string())?;
            writer
                .write_all(data.as_bytes())
                .and_then(|()| writer.flush())
                .map_err(|error| format!("terminal input write failed: {error}"))
        })
    }

    fn inspect_foreground(
        &self,
    ) -> BoxFuture<'static, Result<Option<SubprocessTerminalForeground>, String>> {
        let foreground_id = self.foreground_id;
        let exited = self.exited.load(SeqCst);
        Box::pin(async move {
            Ok((!exited).then_some(SubprocessTerminalForeground {
                process_group_id: foreground_id,
                input_waiting: false,
            }))
        })
    }

    fn signal_foreground(
        &self,
        signal: SubprocessTerminalSignal,
    ) -> BoxFuture<'static, Result<u32, String>> {
        let handle = self.self_arc();
        Box::pin(async move {
            #[cfg(windows)]
            {
                match signal {
                    SubprocessTerminalSignal::SigInt => {
                        handle.write("\u{3}").await?;
                    }
                    SubprocessTerminalSignal::SigTstp => {
                        handle.write("\u{1a}").await?;
                    }
                    SubprocessTerminalSignal::SigKill => {
                        return Err(
                            "refusing to SIGKILL the terminal shell; terminate the terminal session instead"
                                .to_string(),
                        );
                    }
                    SubprocessTerminalSignal::SigTerm | SubprocessTerminalSignal::SigHup => {
                        handle
                            .killer
                            .lock()
                            .kill()
                            .map_err(|error| format!("terminal signal failed: {error}"))?;
                    }
                }
            }
            #[cfg(unix)]
            {
                let number = match signal {
                    SubprocessTerminalSignal::SigInt => libc::SIGINT,
                    SubprocessTerminalSignal::SigTerm => libc::SIGTERM,
                    SubprocessTerminalSignal::SigKill => libc::SIGKILL,
                    SubprocessTerminalSignal::SigTstp => libc::SIGTSTP,
                    SubprocessTerminalSignal::SigHup => libc::SIGHUP,
                };
                if signal == SubprocessTerminalSignal::SigKill && handle.foreground_id == handle.pid
                {
                    return Err(
                        "refusing to SIGKILL the terminal shell; terminate the terminal session instead"
                            .to_string(),
                    );
                }
                let result = unsafe { libc::kill(-(handle.foreground_id as i32), number) };
                if result != 0 {
                    return Err(format!(
                        "terminal signal failed: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
            Ok(handle.foreground_id)
        })
    }

    fn terminate(&self) -> BoxFuture<'static, Result<(), String>> {
        let handle = self.self_arc();
        let slot = {
            let mut cleanup = handle.cleanup.lock();
            cleanup
                .get_or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        Box::pin(async move {
            let mut guard = slot.lock().await;
            if let Some(result) = &*guard {
                return result.clone();
            }
            let result = handle.close_once().await;
            *guard = Some(result.clone());
            if result.is_err() {
                *handle.cleanup.lock() = None;
            }
            result
        })
    }
}

impl PortableTerminalHandle {
    fn self_arc(&self) -> Arc<Self> {
        self.self_arc
            .get()
            .and_then(std::sync::Weak::upgrade)
            .expect("PortableTerminalHandle must be held by an Arc")
    }
}
