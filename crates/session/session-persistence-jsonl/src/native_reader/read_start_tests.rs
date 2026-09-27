use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct ReadTrace {
    events: Mutex<Vec<(&'static str, std::thread::ThreadId)>>,
    source_seen: AtomicBool,
    cancelled: AtomicBool,
}
impl ReadTrace {
    fn record(&self, stage: &'static str) {
        self.events
            .lock()
            .unwrap()
            .push((stage, std::thread::current().id()));
    }
    fn cancellation_check(&self) -> bool {
        if !self.source_seen.swap(true, Ordering::SeqCst) {
            assert_eq!(
                self.events.lock().unwrap().first().map(|row| row.0),
                Some("begin")
            );
            self.record("source");
        }
        self.cancelled.load(Ordering::SeqCst)
    }
}

struct StartSink {
    trace: Arc<ReadTrace>,
    expected: Vec<SessionEvent>,
    output: Vec<SessionEvent>,
    begun: bool,
    inspected: usize,
    cancel_on_push: bool,
}
impl dsh_session_persistence::HistoryWindowSink for StartSink {
    fn begin_read(&mut self) {
        assert!(!self.begun);
        assert!(self.output.is_empty());
        self.begun = true;
        self.trace.record("begin");
    }
    fn inspect(&mut self, event: &SessionEvent) -> Result<(), String> {
        assert!(self.begun && self.trace.source_seen.load(Ordering::SeqCst));
        assert!(self.output.is_empty(), "inspection must not publish a page");
        assert_eq!(event, &self.expected[self.inspected]);
        self.inspected += 1;
        self.trace.record("inspect");
        Ok(())
    }
    fn push(&mut self, event: SessionEvent) -> Result<(), String> {
        assert_eq!(self.inspected, self.expected.len());
        assert_eq!(event, self.expected[self.output.len()]);
        self.output.push(event);
        self.trace.record("push");
        if self.cancel_on_push {
            self.trace.cancelled.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    fn finish(mut self: Box<Self>) -> Result<(Vec<SessionEvent>, bool), String> {
        assert!(self.begun);
        assert_eq!(self.output, self.expected);
        self.trace.record("finish");
        Ok((std::mem::take(&mut self.output), false))
    }
}
impl Drop for StartSink {
    fn drop(&mut self) {
        self.trace.record("drop");
    }
}

async fn read_on_actual_owner(
    fixture: &Fixture,
    forward: bool,
    empty: bool,
    max_events: usize,
    expected: Vec<SessionEvent>,
    trace: Arc<ReadTrace>,
    cancel_on_push: bool,
) -> (
    std::thread::ThreadId,
    Result<dsh_session_persistence::SessionReadWindowResult, String>,
) {
    // Mirror the API: construct on the async caller, then move into the
    // persistence helper's actual spawn_blocking closure.
    let sink = Box::new(StartSink {
        trace: trace.clone(),
        expected,
        output: Vec::new(),
        begun: false,
        inspected: 0,
        cancel_on_push,
    });
    assert!(trace.events.lock().unwrap().is_empty());
    run(
        &fixture.0,
        &dsh_session::session_id("fixture"),
        move |path, id| {
            let owner = std::thread::current().id();
            let cancelled = || trace.cancellation_check();
            let result = if forward {
                forward_window_with_sink(
                    path,
                    id,
                    dsh_session_persistence::SessionReadForwardWindowRequest {
                        after_seq: if empty { 1000 } else { 0 },
                        max_messages: 16,
                        max_events,
                    },
                    &cancelled,
                    Some(sink),
                )
            } else {
                window_with_sink(
                    path,
                    id,
                    SessionReadWindowRequest {
                        before_seq: empty.then_some(0),
                        max_messages: 16,
                        max_events,
                    },
                    &cancelled,
                    Some(sink),
                )
            };
            Ok((owner, result))
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn native_begin_precedes_the_first_source_scan_on_the_reader_thread() {
    let caller = std::thread::current().id();
    let source: Vec<_> = (0..3).map(|seq| event(seq, "user/message")).collect();
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for forward in [false, true] {
            for empty in [false, true] {
                let fixture = fixture(&source, compression);
                let bytes = std::fs::read(&fixture.0).unwrap();
                let trace = Arc::new(ReadTrace::default());
                let expected = if empty { vec![] } else { source.clone() };
                let (owner, result) = read_on_actual_owner(
                    &fixture,
                    forward,
                    empty,
                    16,
                    expected.clone(),
                    trace.clone(),
                    false,
                )
                .await;
                assert_ne!(owner, caller);
                assert_eq!(result.unwrap().events, expected);
                let observed = trace.events.lock().unwrap();
                let mut stages = vec!["begin", "source"];
                stages.extend(std::iter::repeat_n("inspect", expected.len()));
                stages.extend(std::iter::repeat_n("push", expected.len()));
                stages.extend(["finish", "drop"]);
                assert_eq!(observed.iter().map(|row| row.0).collect::<Vec<_>>(), stages);
                assert!(observed.iter().all(|row| row.1 == owner));
                assert_eq!(std::fs::read(&fixture.0).unwrap(), bytes);
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_begin_and_owner_drop_survive_early_errors_and_cancellation_without_finish() {
    let caller = std::thread::current().id();
    for forward in [false, true] {
        for failure in ["missing", "malformed", "cancel-before", "cancel-during"] {
            let source: Vec<_> = (0..3).map(|seq| event(seq, "user/message")).collect();
            let fixture = fixture(&source, JsonlCompression::None);
            if failure == "missing" {
                std::fs::remove_file(&fixture.0).unwrap();
            }
            if failure == "malformed" {
                std::fs::write(&fixture.0, b"not a native Session header\n").unwrap();
            }
            let bytes = std::fs::read(&fixture.0).ok();
            let trace = Arc::new(ReadTrace::default());
            trace
                .cancelled
                .store(failure == "cancel-before", Ordering::SeqCst);
            let (owner, result) = read_on_actual_owner(
                &fixture,
                forward,
                false,
                16,
                source,
                trace.clone(),
                failure == "cancel-during",
            )
            .await;
            let error = result.unwrap_err();
            if failure.starts_with("cancel") {
                assert!(error.contains("cancelled"), "{error}");
            }
            assert_ne!(owner, caller);
            let observed = trace.events.lock().unwrap();
            let stages: Vec<_> = observed.iter().map(|row| row.0).collect();
            let expected = match failure {
                "missing" => vec!["begin", "drop"],
                "cancel-during" => vec![
                    "begin", "source", "inspect", "inspect", "inspect", "push", "drop",
                ],
                _ => vec!["begin", "source", "drop"],
            };
            assert_eq!(stages, expected, "{failure}: {error}");
            assert!(observed.iter().all(|row| row.1 == owner));
            assert_eq!(std::fs::read(&fixture.0).ok(), bytes);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_begin_runs_before_an_oversized_boundary_scan_without_publishing_a_page() {
    let mut source = vec![event(0, "user/message")];
    source.extend((1..=12).map(|seq| event(seq, "assistant/chunk")));
    let fixture = fixture(&source, JsonlCompression::Zstd);
    let trace = Arc::new(ReadTrace::default());
    let (owner, result) =
        read_on_actual_owner(&fixture, false, false, 4, source, trace.clone(), false).await;
    let result = result.unwrap();
    assert!(result.events.is_empty());
    assert_eq!(result.oversized_event_count, Some(5));
    assert_eq!(
        *trace.events.lock().unwrap(),
        vec![("begin", owner), ("source", owner), ("drop", owner)]
    );
}
