//! The spill-policy PLUGIN: a `tools/post-execute` result transformer that
//! keeps oversized plain-text tool results out of the model's context. When
//! a final result's UTF-8 size exceeds `maxInlineBytes`, it saves the FULL
//! text to a session-scoped spill artifact (`ctx.spillStore`) and replaces
//! the model-facing result with a bounded head/tail preview plus the
//! backend's locator and retrieval guidance. Rust port of
//! `packages/spill/spill-policy/src/index.ts`.
//!
//! It registers NO service and owns NO storage or preview mechanics: preview
//! is `dsh-output-retention` (`TextRetainer`), storage is `ctx.spillStore`.
//! The policy only decides WHEN to spill and composes the notice.
//!
//! ## Deliberately narrow
//!
//! - Omitted `maxInlineBytes` ⇒ the plugin registers nothing (a true no-op).
//! - Plain-text results only: a result carrying any non-text block is left
//!   untouched.
//! - Nested composite calls skip the model-facing arm.
//! - Accepted value replacements pass through for registry revalidation and
//!   rendering.
//! - `read` is skipped by the model-facing arm to avoid a
//!   `read → spill → read again` loop.
//! - Best-effort: no session owner, no `ctx.spillStore` backend, or a save
//!   failure ⇒ log and return the original result.
//!
//! # Deviations
//!
//! - The second (durable `tools/code-dispatch-log`) arm waits for the
//!   dsh-code-runtime milestone; the model-facing post-execute arm is
//!   complete.
//! - TS `maxInlineBytes` runtime validation (integer, non-negative) is the
//!   Rust `u64` type itself; the load-time rejection tests are
//!   inexpressible.

use std::sync::Arc;

use cordis::{
    ArcValue, Context, Disposer, EventOptions, InjectSpec, Listener, NextFn, Plugin, PluginError,
    arc, downcast_arc, make_disposer,
};
use dsh_llm::{CallId, ContentBlock};
use dsh_output_retention::{NoticeUnit, Omitted, TextRetainer, TextRetentionStrategy, describe_omitted};
use dsh_session::SessionId;
use dsh_spill::{SaveTextSpill, SpillRef, SpillStore, SpillOwner, SpillSource};
use dsh_tools::{PostToolDecision, ToolExecution, ToolExecutionResult};

/// Plugin config.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The model-facing context cap for a plain-text tool result, in UTF-8
    /// bytes. Omitted disables the policy entirely (no-op). When set, a
    /// result larger than this is spilled and replaced with a preview derived
    /// from this same budget.
    pub max_inline_bytes: Option<u64>,
}

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "spill-policy";

/// Require the tool registry (its `tools/post-execute` waterfall is the
/// extension point we transform).
pub const INJECT: [&str; 1] = ["tools"];

/// All-text content flattened to one UTF-8 string, or `None` if any block is
/// non-text.
fn flatten_plain_text(content: &[ContentBlock]) -> Option<String> {
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text { text: block_text } => text.push_str(block_text),
            _ => return None,
        }
    }
    Some(text)
}

/// The owning session id, or `None` for a call with no agent (a direct/test
/// call).
fn owner_session_id(exec: &ToolExecution) -> Option<SessionId> {
    exec.agent.as_ref().map(|agent| agent.session().header().id.clone())
}

/// Build the bounded head/tail preview for `text`, splitting `budget` bytes
/// across the two ends.
fn preview(text: &str, budget: usize) -> (String, Omitted) {
    let head_bytes = budget.div_ceil(2);
    let tail_bytes = budget / 2;
    let mut retainer = TextRetainer::new(TextRetentionStrategy::HeadTail { head_bytes, tail_bytes });
    retainer.push(text.as_bytes());
    let kept = retainer.finish();
    (kept.text, kept.omitted_bytes)
}

/// The spill-notice line for a given omission + saved reference (no preview,
/// no leading blank line).
fn spill_notice(omitted: Omitted, reference: &SpillRef) -> String {
    let omission = describe_omitted(omitted, NoticeUnit::Bytes);
    format!(
        "({omission} Full formatted result stored at: {}. {})",
        reference.locator, reference.retrieval_hint
    )
}

/// Spill `text` and build the bounded replacement (preview + notice), or
/// return `None` when the policy must keep the original (no session owner,
/// no backend, storage failure, or no within-cap replacement).
async fn spill_replacement(
    ctx: &Context,
    cap: usize,
    text: &str,
    total_bytes: usize,
    session_id: Option<SessionId>,
    tool_name: &str,
    call_id: &CallId,
    label: &str,
) -> Option<String> {
    let Some(session_id) = session_id else {
        ctx.named_logger(None).warn(vec![arc(format!(
            "spill-policy: no session owner for {tool_name} {label}; keeping the inline content"
        ))]);
        return None;
    };
    let Some(spill_store) = ctx.get_typed::<Arc<dyn SpillStore>>("spillStore", false) else {
        ctx.named_logger(None).warn(vec![arc(
            "spill-policy: no ctx.spillStore backend loaded; keeping the inline content"
                .to_string(),
        )]);
        return None;
    };
    let save = SaveTextSpill {
        owner: SpillOwner { session_id },
        source: SpillSource {
            tool_name: tool_name.to_string(),
            call_id: call_id.clone(),
            label: label.to_string(),
        },
        suggested_name: format!("{tool_name}.txt"),
        content: text.to_string(),
    };
    let reference = match spill_store.save_text(&save).await {
        Ok(reference) => reference,
        Err(error) => {
            ctx.named_logger(None).warn(vec![arc(format!(
                "spill-policy: saveText failed for {tool_name}: {error}; keeping the inline content"
            ))]);
            return None;
        }
    };

    // Reserve the notice's byte cost INSIDE maxInlineBytes so the replacement
    // (preview + blank line + notice) never exceeds the documented cap. The
    // reservation uses a notice priced at the worst-case omission count (the
    // full byte total): its digit count bounds the real count's, so the
    // reserved size is a safe upper bound and the final notice is never
    // longer than what we reserved. `\n\n` is the 2-byte join.
    let reserve = spill_notice(Omitted::Exact { count: total_bytes }, &reference).len() + 2;
    let preview_budget = cap.saturating_sub(reserve);
    let (preview_text, omitted) = preview(text, preview_budget);
    let notice = spill_notice(omitted, &reference);
    let replaced_text = if preview_text.is_empty() {
        notice
    } else {
        format!("{preview_text}\n\n{notice}")
    };
    // Invariant: the policy NEVER emits a replacement larger than the cap.
    // When the notice alone exceeds maxInlineBytes (a tiny cap or a long
    // spill root), there is no within-cap replacement, so keep the inline
    // content. (A within-cap replacement is always smaller than the
    // original, which is > cap by the entry condition, so this one check
    // subsumes "not smaller than the original" too. The spill file already
    // written is a harmless orphan; cleanup is deferred.)
    if replaced_text.len() > cap {
        ctx.named_logger(None).warn(vec![arc(format!(
            "spill-policy: spill notice for {tool_name} exceeds maxInlineBytes; keeping the inline content"
        ))]);
        return None;
    }
    Some(replaced_text)
}

/// Install the policy (the TS `apply`). Returns a disposer removing the
/// registered listener.
pub fn apply(ctx: &Context, config: Config) -> Result<Disposer, String> {
    let Some(cap) = config.max_inline_bytes else {
        // Omitted ⇒ no automatic spill policy: register nothing at all.
        return Ok(make_disposer(|| Box::pin(async {})));
    };
    let cap = cap as usize;

    let listener: Arc<Listener> = Arc::new(move |dispatch_ctx: &Context, args: Vec<ArcValue>| {
        // The waterfall args carry the live handles: `Arc<ToolExecution>`
        // and `Arc<Arc<ToolExecutionResult>>` (the registry's own dispatch
        // shape).
        let exec = downcast_arc::<Arc<ToolExecution>>(&args[0])
            .expect("tools/post-execute exec argument");
        let result = downcast_arc::<Arc<ToolExecutionResult>>(&args[1])
            .expect("tools/post-execute result argument");
        let next = downcast_arc::<NextFn>(&args[2])
            .expect("tools/post-execute next continuation")
            .clone();
        let ctx = dispatch_ctx.clone();
        Box::pin(async move {
            // Delegate first so a downstream listener (e.g. a hook) settles
            // the result; we bound whatever it accepted. A block passes
            // through — spill only shapes accepted plain-text results, never
            // corrective feedback.
            let decision_value = next.call().await;
            let decision = downcast_arc::<PostToolDecision>(&decision_value)
                .map(|decision| (*decision).clone())
                .unwrap_or_else(|| panic!("tools/post-execute listener returned no decision"));

            // Skip `read` to avoid a read → spill → read again loop; nested
            // calls and value replacements pass through unchanged.
            let PostToolDecision::Accept { content, value: None, additional_contexts } = &decision
            else {
                return Some(arc(decision));
            };
            if exec.parent.is_some() || exec.name == "read" {
                return Some(arc(decision));
            }

            let content = content.clone().unwrap_or_else(|| result.content.clone());
            let Some(text) = flatten_plain_text(&content) else {
                return Some(arc(decision));
            };
            let total_bytes = text.len();
            if total_bytes <= cap {
                return Some(arc(decision));
            }

            let replaced_text = spill_replacement(
                &ctx,
                cap,
                &text,
                total_bytes,
                owner_session_id(&exec),
                &exec.name,
                &exec.call_id,
                "result",
            )
            .await;
            let Some(replaced_text) = replaced_text else {
                return Some(arc(decision));
            };
            let replaced: Vec<ContentBlock> = vec![ContentBlock::Text { text: replaced_text }];
            Some(arc(PostToolDecision::Accept {
                content: Some(replaced),
                value: None,
                additional_contexts: additional_contexts.clone(),
            }))
        })
    });

    let disposer = futures::executor::block_on(ctx.on(
        "tools/post-execute",
        listener,
        EventOptions::default().prepend(true),
    ));
    Ok(disposer)
}

/// The Cordis plugin form of the policy.
pub struct SpillPolicyPlugin {
    config: Config,
}

impl SpillPolicyPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for SpillPolicyPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx, self.config.clone()).map(|_| ()).map_err(|error| PluginError::new(arc(error)))
    }
}
