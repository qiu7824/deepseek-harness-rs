//! Exhaustiveness helper for closed core unions. Rust port of
//! `packages/llm/llm/src/never.ts`.

/// Mark an unreachable closed-union branch: always panics with the offending
/// value rendered and the optional context label.
pub fn assert_never<T: std::fmt::Debug>(value: T, context: Option<&str>) -> ! {
    match context {
        Some(context) => panic!("unreachable variant in {context}: {value:?}"),
        None => panic!("unreachable variant: {value:?}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn assert_never_panics() {
        let result = std::panic::catch_unwind(|| super::assert_never(42u8, Some("test")));
        let message = result.unwrap_err();
        let rendered = message
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "not a string panic".to_string());
        assert_eq!(rendered, "unreachable variant in test: 42");
    }
}
