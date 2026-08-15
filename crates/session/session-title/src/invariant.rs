//! Package-owned invariant companion for `@deepseek-ai/dsh-session-title`.
//! Rust port of `packages/session/session-title/src/invariant.ts`.
//!
//! Durable title-source invariant: an automatic title always cites at
//! least one human `user/message` seq, and an explicit user rename cites
//! none — `messageSeqs` is empty iff `source.kind` is `user`.

use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast, downcast_arc};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-title";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "session-title-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Check one `session/title` event's durable source relationship (pure,
/// exported for the unit spec).
pub fn check_title_event(event: &SessionEvent, fail: &dyn Fn(&str)) {
    if event.type_ != "session/title" {
        return;
    }
    let source_kind = event
        .data
        .get("source")
        .and_then(|source| source.get("kind"))
        .and_then(|kind| kind.as_str());
    let count = event
        .data
        .get("messageSeqs")
        .and_then(|seqs| seqs.as_array())
        .map(|seqs| seqs.len())
        .unwrap_or(0);
    let is_user = source_kind == Some("user");
    if (count == 0) != is_user {
        let kind = source_kind.unwrap_or("<absent>");
        let requirement = if is_user {
            "cite no message seqs"
        } else {
            "cite at least one message seq"
        };
        fail(&format!(
            "session/title event {} with source \"{kind}\" must {requirement}; got {count}",
            event.seq
        ));
    }
}

/// Build the installer registered under [`PACKAGE_NAME`].
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // internal/dispatch interception rejects the append before
                // publication (the session/event listener would only observe
                // the already-committed log).
                let dispatch_fail = fail.clone();
                let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                    let event_name = args
                        .get(1)
                        .and_then(|value| downcast::<String>(value))
                        .cloned()
                        .unwrap_or_default();
                    if event_name != "session/event" {
                        return Box::pin(async { None });
                    }
                    let dispatch_args = args
                        .get(2)
                        .and_then(|value| downcast_arc::<Vec<ArcValue>>(value));
                    let fail = dispatch_fail.clone();
                    Box::pin(async move {
                        let Some(dispatch_args) = dispatch_args else {
                            return None;
                        };
                        let Some(event) = dispatch_args
                            .get(1)
                            .and_then(|value| downcast::<SessionEvent>(value))
                            .cloned()
                        else {
                            return None;
                        };
                        if let Some(_session) =
                            dispatch_args.first().and_then(|value| downcast::<Session>(value))
                        {
                            let _ = _session;
                        }
                        check_title_event(&event, &|message| fail(message));
                        None
                    })
                });
                ctx.on(
                    "internal/dispatch",
                    listener,
                    EventOptions::default().global(true),
                )
                .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the session-title invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct SessionTitleInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for SessionTitleInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
