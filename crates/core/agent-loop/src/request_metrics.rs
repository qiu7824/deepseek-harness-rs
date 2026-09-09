//! Monotonic diagnostics for the local consumer and its upstream stream.
use std::time::{Duration, Instant};

pub(crate) struct RequestMetrics {
    started: Instant,
    wait: Duration,
    processing: Duration,
    longest_processing: Duration,
    longest_wait: Duration,
    first_token: Option<Duration>,
    last_token: Option<Duration>,
    chunks: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_stream_wait_from_processing_and_excludes_structural_chunks_from_ttft() {
        use dsh_llm::StreamChunk;
        let mut metrics = RequestMetrics::new();
        metrics.started = Instant::now() - Duration::from_millis(100);
        metrics.waited(Duration::from_millis(12));
        metrics.waited(Duration::from_millis(28));
        metrics.processed(
            &StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            Instant::now(),
        );
        assert!(metrics.value()["firstTokenMs"].is_null());
        let arrival = Instant::now() - Duration::from_millis(5);
        metrics.processed(
            &StreamChunk::TextDelta {
                index: 0,
                text: "test".into(),
            },
            arrival,
        );
        metrics.processed(
            &StreamChunk::ReasoningDelta {
                index: 1,
                text: "reasoning".into(),
            },
            Instant::now(),
        );
        let value = metrics.value();
        assert_eq!(value["streamWaitMs"], 40);
        assert_eq!(value["longestStreamWaitMs"], 28);
        assert_eq!(value["chunks"], 3);
        assert!(value["localProcessingMs"].as_u64().unwrap() >= 5);
        assert!(value["lastTokenMs"].as_u64().unwrap() >= value["firstTokenMs"].as_u64().unwrap());
        assert!(value["requestWallMs"].as_u64().unwrap() >= value["lastTokenMs"].as_u64().unwrap());
    }
}

impl RequestMetrics {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            wait: Duration::ZERO,
            processing: Duration::ZERO,
            longest_processing: Duration::ZERO,
            longest_wait: Duration::ZERO,
            first_token: None,
            last_token: None,
            chunks: 0,
        }
    }
    pub(crate) fn waited(&mut self, duration: Duration) {
        self.wait += duration;
        self.longest_wait = self.longest_wait.max(duration);
    }
    pub(crate) fn processed(&mut self, chunk: &dsh_llm::StreamChunk, arrived: Instant) {
        let elapsed = arrived.elapsed();
        self.processing += elapsed;
        self.longest_processing = self.longest_processing.max(elapsed);
        self.chunks += 1;
        if dsh_llm::is_token_delta(chunk) {
            let offset = arrived.duration_since(self.started);
            self.first_token.get_or_insert(offset);
            self.last_token = Some(offset);
        }
    }
    pub(crate) fn value(&self) -> serde_json::Value {
        let ms = |duration: Duration| duration.as_millis().min(u64::MAX as u128) as u64;
        let elapsed = self.started.elapsed();
        serde_json::json!({"requestWallMs":ms(elapsed),"streamWaitMs":ms(self.wait),
            "localProcessingMs":ms(self.processing),"maxChunkProcessingMs":ms(self.longest_processing),
            "longestStreamWaitMs":ms(self.longest_wait),"chunks":self.chunks,
            "firstTokenMs":self.first_token.map(ms),"lastTokenMs":self.last_token.map(ms),
            "tailMs":self.last_token.map(|last|ms(elapsed.saturating_sub(last)))})
    }
}
