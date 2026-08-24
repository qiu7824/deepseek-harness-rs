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
