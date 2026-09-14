//! Request-attempt diagnostics use one backend clock and carry no prompt data.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, OnceLock, Weak},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPhase {
    pub phase: String,
    pub stage: String,
    pub attempt_id: String,
    pub execution_instance_id: String,
    pub provider: String,
    pub model: String,
    pub elapsed_ms: u64,
    pub network_elapsed_ms: Option<u64>,
    pub output_tokens: Option<u64>,
    pub code: Option<String>,
    pub measurement: String,
}
struct Timing {
    started: Instant,
    network: Option<Instant>,
    attempt: String,
    provider: String,
    model: String,
    usage: Option<u64>,
    done: bool,
    last_progress: Option<Instant>,
    stage: String,
}
impl Timing {
    fn new(provider: String, model: String) -> Self {
        Self {
            started: Instant::now(),
            network: None,
            attempt: uuid::Uuid::new_v4().to_string(),
            provider,
            model,
            usage: None,
            done: false,
            last_progress: None,
            stage: "preparing".into(),
        }
    }
}
#[derive(Clone)]
pub struct RequestTelemetry {
    sink: Arc<dyn Fn(RequestPhase) + Send + Sync>,
    timing: Arc<Mutex<Timing>>,
    active: Arc<Mutex<Vec<Weak<Mutex<Timing>>>>>,
}
impl RequestTelemetry {
    pub fn new(sink: Arc<dyn Fn(RequestPhase) + Send + Sync>) -> Self {
        Self {
            sink,
            timing: Arc::new(Mutex::new(Timing::new(String::new(), String::new()))),
            active: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn for_attempt(&self, provider: String, model: String) -> Self {
        let timing = Arc::new(Mutex::new(Timing::new(provider, model)));
        let mut active = self.active.lock();
        active.retain(|entry| entry.strong_count() > 0);
        active.push(Arc::downgrade(&timing));
        Self {
            sink: self.sink.clone(),
            timing,
            active: self.active.clone(),
        }
    }
    /// Terminate live attempts when the caller drops a cancelled provider stream.
    pub fn cancel_pending(&self) {
        let attempts: Vec<_> = self.active.lock().iter().filter_map(Weak::upgrade).collect();
        for timing in attempts {
            Self { sink: self.sink.clone(), timing, active: self.active.clone() }
                .finish("cancelled", Some("CANCELLED"));
        }
    }
    pub fn network_start(&self) {
        if self.timing.lock().network.is_some() {
            self.finish("superseded", Some("REQUEST_RETRIED"));
            let mut state = self.timing.lock();
            let provider = state.provider.clone();
            let model = state.model.clone();
            *state = Timing::new(provider, model);
        }
        self.timing.lock().network = Some(Instant::now());
        self.phase("request_sent", None);
    }
    pub fn phase(&self, phase: &str, code: Option<&str>) {
        static INSTANCE: OnceLock<String> = OnceLock::new();
        let instance = INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string());
        let mut state = self.timing.lock();
        if !matches!(phase, "completed" | "failed" | "cancelled" | "superseded") {
            state.stage = phase.into();
        }
        let millis = |elapsed: Duration| elapsed.as_millis().min(u64::MAX as u128) as u64;
        let event = RequestPhase {
            phase: phase.into(),
            stage: state.stage.clone(),
            attempt_id: state.attempt.clone(),
            execution_instance_id: instance.clone(),
            provider: state.provider.clone(),
            model: state.model.clone(),
            elapsed_ms: millis(state.started.elapsed()),
            network_elapsed_ms: state.network.map(|time| millis(time.elapsed())),
            output_tokens: state.usage,
            code: code.map(str::to_string),
            measurement: "request-average".into(),
        };
        drop(state);
        (self.sink)(event);
    }
    pub fn finish(&self, phase: &str, code: Option<&str>) {
        let mut state = self.timing.lock();
        if state.done {
            return;
        }
        state.done = true;
        drop(state);
        self.phase(phase, code);
    }
    pub fn observe(&self, chunk: &crate::StreamChunk) {
        if crate::is_token_delta(chunk) {
            let mut state = self.timing.lock();
            let first = state.last_progress.is_none();
            if first
                || state
                    .last_progress
                    .is_some_and(|time| time.elapsed() >= Duration::from_millis(500))
            {
                state.last_progress = Some(Instant::now());
                drop(state);
                self.phase(
                    if first {
                        "first_output"
                    } else {
                        "stream_progress"
                    },
                    None,
                );
            }
        }
        match chunk {
            crate::StreamChunk::Usage { usage } => {
                self.timing.lock().usage = Some(usage.output_tokens)
            }
            crate::StreamChunk::Finish { reason, .. } => match reason {
                crate::FinishReason::Error { failure } => self.finish(
                    if failure.code == "CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    Some(&failure.code),
                ),
                crate::FinishReason::Aborted { failure } => {
                    self.finish("cancelled", Some(&failure.code))
                }
                _ => self.finish("completed", None),
            },
            _ => {}
        }
    }
}
