//! First-human-message model provider for `ctx.sessionTitle`. Rust port of
//! `@deepseek-ai/dsh-session-title-first-prompt-llm`.

pub mod index;
pub mod invariant;

pub use dsh_session_title_llm::SessionTitleLlmConfig as Config;
pub use index::{INJECT, NAME, SessionTitleFirstPromptLlmPlugin, apply};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    /// The selector rejects an empty revision with the TS message.
    #[test]
    fn selector_requires_one_human_message() {
        let selector: dsh_session_title_llm::SessionTitleLlmMessageSelector =
            Arc::new(|messages| match messages.into_iter().next() {
                Some(first) => Ok(vec![first]),
                None => Err("first-prompt title provider requires one human message".to_string()),
            });
        let error = (selector)(Vec::new()).err().expect("reject");
        assert_eq!(
            error,
            "first-prompt title provider requires one human message"
        );
        let selected = (selector)(vec![dsh_session_title::SessionTitleUserMessage {
            seq: 1,
            text: "first".to_string(),
        }])
        .expect("select");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].seq, 1);
    }
}
