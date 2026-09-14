//! Bounded maintenance owned by the adapter, independent of request workers.
use crate::{DeepSeekFileScope, DeepSeekFilesClient, DeepSeekUploadIndex};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
struct Job {
    client: DeepSeekFilesClient,
    index: DeepSeekUploadIndex,
    scope: DeepSeekFileScope,
}
#[derive(Default)]
pub(crate) struct CleanupWorker {
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    stopped: Arc<AtomicBool>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
impl CleanupWorker {
    pub fn schedule(
        &self,
        client: DeepSeekFilesClient,
        index: DeepSeekUploadIndex,
        scope: DeepSeekFileScope,
    ) {
        let mut sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        if sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Job>(32);
            let stopped = self.stopped.clone();
            let thread=std::thread::Builder::new().name("dsh-file-cleanup".into()).spawn(move||{
                let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().expect("cleanup runtime");
                while !stopped.load(Ordering::SeqCst) {
                    let job=match rx.recv_timeout(Duration::from_millis(100)){Ok(job)=>job,Err(mpsc::RecvTimeoutError::Timeout)=>continue,Err(_)=>break};
                    runtime.block_on(async {
                        let work=async {
                            let pending=job.index.pending_cleanup(&job.scope,now()).await.unwrap_or_default();
                            for record in pending {
                                if stopped.load(Ordering::SeqCst){return;}
                                let outcome=tokio::time::timeout(Duration::from_secs(15),job.client.delete(&record.file_id)).await;
                                let success=matches!(&outcome,Ok(Ok(())))||matches!(&outcome,Ok(Err(error)) if error.status==Some(404));
                                let _=job.index.cleanup_result(&job.scope,&record.file_id,success,now()).await;
                                if !success {break;}
                            }
                        };
                        tokio::select! {
                            _=tokio::time::timeout(Duration::from_secs(60),work)=>{},
                            _=async {while !stopped.load(Ordering::SeqCst){tokio::time::sleep(Duration::from_millis(10)).await;}}=>{},
                        }
                    });
                }
                runtime.shutdown_timeout(Duration::from_millis(100));
            });
            if let Ok(thread) = thread {
                *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
                *sender = Some(tx);
            } else {
                return;
            }
        }
        // The ledger already holds retry records. A full queue only defers
        // maintenance until a later request; it never blocks model admission.
        if let Some(sender) = sender.as_ref() {
            let _ = sender.try_send(Job {
                client,
                index,
                scope,
            });
        }
    }
}
impl Drop for CleanupWorker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.sender.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(thread) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = thread.join();
        }
    }
}
