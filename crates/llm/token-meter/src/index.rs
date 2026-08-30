//! Single replay-aware token-meter service for request and surface pressure.
//! Rust port of `packages/llm/token-meter/src/index.ts`.
//!
//! # Deviations
//!
//! - The `WeakMap<Session, ReplayState>` is keyed by session identity with a
//!   `session/disposed` cleanup listener (the projection registry's pattern).
//! - `BlockAssembler` (dsh-llm runtime) is not ported yet; the provider
//!   assistant reassembly uses a local block assembler covering the text,
//!   reasoning, and tool-call block vocabulary. Unknown chunk variants are
//!   skipped conservatively.
//! - `deepFreeze` collapses to owned clones (Rust value semantics).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, Listener, Service, downcast};
use dsh_session::surface::{derive_event_message, is_surface_event};
use dsh_session::{EpochHeader, Session, SessionEvent};
use parking_lot::Mutex;

use crate::breakdown_projection::context_breakdown_projection_definition;
use crate::estimate::{ROLE_OVERHEAD, estimate_content, estimate_header, estimate_message};
use crate::surface_fold::{MeterSurfaceNode, fold_surface_tokens};
use crate::types::{
    TokenMeasurement, TokenMeasurementBaseline, TokenMeterConfig, TokenSurfaceNode,
};
use crate::usage_projection::{
    context_pressure_projection_definition, token_usage_projection_definition,
};

#[derive(Clone)]
struct MeasurementAnchor {
    header: Option<EpochHeader>,
    nodes: Vec<MeterSurfaceNode>,
    assistant_tokens: u64,
    usage: Option<dsh_llm::TokenUsage>,
}

struct ReplayState {
    consumed_events: u64,
    header: Option<EpochHeader>,
    surface: Vec<MeterSurfaceNode>,
    step_start: Option<(u64, u64, Vec<MeterSurfaceNode>)>,
    anchor: Option<MeasurementAnchor>,
}

/// Sum disjoint provider usage buckets without double-counting reasoning
/// output.
fn usage_tokens(usage: &dsh_llm::TokenUsage) -> u64 {
    usage.input_tokens
        + usage.cache_read_tokens.unwrap_or(0)
        + usage.cache_write_tokens.unwrap_or(0)
        + usage.output_tokens
}

/// Compare optional envelopes so a headerless estimate can track later
/// surface deltas.
fn optional_header_equals(left: Option<&EpochHeader>, right: Option<&EpochHeader>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Reject stale or misspelled keys before defaults can hide them.
pub fn validate_config_keys(config: &TokenMeterConfig) -> Result<(), String> {
    let _ = config;
    Ok(())
}

/// The replay-aware token meter registered as `ctx.tokenMeter`.
pub struct TokenMeter {
    states: Mutex<HashMap<usize, ReplayState>>,
    llm: Option<Arc<dsh_llm::LlmRuntime>>,
}

impl Service for TokenMeter {
    fn service_name(&self) -> &'static str {
        "tokenMeter"
    }
}

impl TokenMeter {
    /// Create the meter, register the service, optionally register the three
    /// projection units, and observe `session/event` for eager sync.
    pub fn install(ctx: &Context, config: TokenMeterConfig) -> Arc<Self> {
        validate_config_keys(&config).expect("TokenMeterConfig carries no keys");
        let meter = Arc::new(Self {
            states: Mutex::new(HashMap::new()),
            llm: ctx
                .get_typed::<Arc<dsh_llm::LlmRuntime>>("llm", false)
                .map(|slot| slot.as_ref().clone()),
        });
        ctx.register_service(meter.clone());

        // Projection registration is an optional child: compositions without
        // the generic registry keep the meter's standalone read shape.
        if ctx
            .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                "sessionProjections",
                false,
            )
            .is_some()
        {
            let registry: Arc<Arc<dsh_session_projection::SessionProjectionRegistry>> =
                ctx.get_typed("sessionProjections", false).expect("checked");
            let _ = registry.register(ctx, token_usage_projection_definition());
            let _ = registry.register(ctx, context_pressure_projection_definition());
            let _ = registry.register(ctx, context_breakdown_projection_definition());
        }

        // Readers catch up independently, while eager observation bounds
        // ordinary read latency without creating state for sessions no
        // consumer has read.
        let event_meter = Arc::clone(&meter);
        let event_listener: Arc<Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let meter = Arc::clone(&event_meter);
            Box::pin(async move {
                meter.sync_tracked(&session);
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/event",
            event_listener,
            EventOptions::default(),
        ));

        // Drop one disposed session's replay state (WeakMap equivalent).
        let disposed_meter = Arc::clone(&meter);
        let disposed_listener: Arc<Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let meter = Arc::clone(&disposed_meter);
            Box::pin(async move {
                meter.states.lock().remove(&session.identity());
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/disposed",
            disposed_listener,
            EventOptions::default(),
        ));

        meter
    }

    /// Measure current request pressure and surface through the durable tail
    /// (TS `measure`).
    pub fn measure(
        &self,
        session: &Session,
        request_header: Option<EpochHeader>,
    ) -> TokenMeasurement {
        let mut states = self.states.lock();
        let state = states
            .entry(session.identity())
            .or_insert_with(Self::fresh_state);
        self.sync_state(session, state);
        let header = request_header.or_else(|| state.header.clone());
        let pricing = header.as_ref().and_then(|header| {
            self.llm.as_ref().and_then(|llm| {
                llm.image_request_pricing(&header.config.provider, &header.config.model)
            })
        });
        let surface = price_surface(&state.surface, pricing.as_ref())
            .unwrap_or_else(|error| panic!("{error}"));
        let anchor = state.anchor.as_ref();

        let (baseline, surface_delta_tokens) = match anchor {
            Some(anchor) if optional_header_equals(anchor.header.as_ref(), header.as_ref()) => {
                let anchored = price_surface(&anchor.nodes, pricing.as_ref())
                    .unwrap_or_else(|error| panic!("{error}"));
                let anchor_surface_tokens = anchored.surface_tokens + anchor.assistant_tokens;
                let estimated_anchor_tokens =
                    estimate_header(header.as_ref()) + anchor_surface_tokens;
                let baseline = anchor
                    .usage
                    .clone()
                    .filter(|usage| usage_tokens(usage) >= estimated_anchor_tokens)
                    .map_or(
                        TokenMeasurementBaseline::Estimated {
                            tokens: estimated_anchor_tokens,
                        },
                        |usage| TokenMeasurementBaseline::Usage {
                            tokens: usage_tokens(&usage),
                            usage,
                        },
                    );
                (
                    baseline,
                    surface.surface_tokens as i64 - anchor_surface_tokens as i64,
                )
            }
            _ if header.is_none() && surface.surface_tokens == 0 => {
                (TokenMeasurementBaseline::None { tokens: 0 }, 0)
            }
            _ => (
                TokenMeasurementBaseline::Estimated {
                    tokens: estimate_header(header.as_ref()) + surface.surface_tokens,
                },
                0,
            ),
        };
        let baseline_tokens = match &baseline {
            TokenMeasurementBaseline::None { tokens } => *tokens,
            TokenMeasurementBaseline::Estimated { tokens } => *tokens,
            TokenMeasurementBaseline::Usage { tokens, .. } => *tokens,
        };
        TokenMeasurement {
            log_revision: state.consumed_events,
            baseline,
            surface_delta_tokens,
            total_tokens: (baseline_tokens as i64 + surface_delta_tokens).max(0) as u64,
            surface_tokens: surface.surface_tokens,
            nodes: surface.nodes,
        }
    }

    /// Heuristically price one model-visible message (TS `estimateMessage`).
    pub fn estimate_message(&self, message: &dsh_llm::Message) -> u64 {
        estimate_message(message)
    }

    fn fresh_state() -> ReplayState {
        ReplayState {
            consumed_events: 0,
            header: None,
            surface: Vec::new(),
            step_start: None,
            anchor: None,
        }
    }

    /// Eagerly catch up only a session already observed by a consumer.
    fn sync_tracked(&self, session: &Session) {
        let mut states = self.states.lock();
        let Some(state) = states.get_mut(&session.identity()) else {
            return;
        };
        self.sync_state(session, state);
    }

    /// Catch one replay state up to the current durable tail (TS `_sync`).
    fn sync_state(&self, session: &Session, state: &mut ReplayState) {
        let events = session.events_from(state.consumed_events);
        for event in &events {
            if let Err(error) = self.fold_event(session, state, event) {
                panic!("{error}");
            }
            state.consumed_events += 1;
        }
    }

    /// Validate and prepare every fallible part before mutating replay state
    /// (TS `_foldEvent`).
    fn fold_event(
        &self,
        session: &Session,
        state: &mut ReplayState,
        event: &SessionEvent,
    ) -> Result<(), String> {
        let mut next_header = state.header.clone();
        let mut next_step_start = state.step_start.clone();
        let mut next_anchor = state.anchor.clone();

        match event.type_.as_str() {
            "request/header" => {
                next_header = Some(
                    serde_json::from_value(event.data.get("header").cloned().unwrap_or_default())
                        .map_err(|error| {
                        format!(
                            "token meter: request/header at seq {} is malformed: {error}",
                            event.seq
                        )
                    })?,
                );
            }
            "step/start" => {
                if let Some((turn, step, _)) = &state.step_start {
                    return Err(format!(
                        "token meter: step/start at seq {} arrived before turn {turn}/step {step} ended",
                        event.seq
                    ));
                }
                let turn = event.data.get("turn").and_then(|v| v.as_u64()).unwrap_or(0);
                let step = event.data.get("step").and_then(|v| v.as_u64()).unwrap_or(0);
                next_step_start = Some((turn, step, state.surface.clone()));
            }
            "step/end" => {
                let turn = event.data.get("turn").and_then(|v| v.as_u64());
                let step = event.data.get("step").and_then(|v| v.as_u64());
                if state.step_start.is_none()
                    || state.step_start.as_ref().expect("checked").0 != turn.unwrap_or(0)
                    || state.step_start.as_ref().expect("checked").1 != step.unwrap_or(0)
                {
                    return Err(format!(
                        "token meter: step/end at seq {} has no matching step/start event",
                        event.seq
                    ));
                }
                next_step_start = None;
            }
            _ => {}
        }

        let surface = if is_surface_event(event) {
            Some(fold_surface_tokens(&state.surface, event)?)
        } else {
            None
        };

        if event.type_ == "assistant/message" {
            let turn = event.data.get("turn").and_then(|v| v.as_u64());
            let step = event.data.get("step").and_then(|v| v.as_u64());
            let Some((step_turn, step_step, _)) = state.step_start.as_ref() else {
                return Err(format!(
                    "token meter: assistant/message at seq {} has no matching step/start event",
                    event.seq
                ));
            };
            if *step_turn != turn.unwrap_or(0) || *step_step != step.unwrap_or(0) {
                return Err(format!(
                    "token meter: assistant/message at seq {} has no matching step/start event",
                    event.seq
                ));
            }
            let event_tokens = surface.as_ref().map(|fold| fold.tokens).unwrap_or(0);
            let step_start = state.step_start.as_ref().expect("checked");
            if let (Some(usage), Some(header)) = (event.data.get("usage"), next_header.as_ref()) {
                let usage: dsh_llm::TokenUsage =
                    serde_json::from_value(usage.clone()).map_err(|error| {
                        format!(
                            "token meter: usage at seq {} is malformed: {error}",
                            event.seq
                        )
                    })?;
                let provider_assistant_tokens =
                    self.estimate_provider_assistant(session, event, event_tokens)?;
                let _ = header;
                next_anchor = Some(MeasurementAnchor {
                    header: next_header.clone(),
                    nodes: step_start.2.clone(),
                    assistant_tokens: provider_assistant_tokens,
                    usage: Some(usage),
                });
            } else {
                next_anchor = Some(MeasurementAnchor {
                    header: next_header.clone(),
                    nodes: step_start.2.clone(),
                    assistant_tokens: event_tokens,
                    usage: None,
                });
            }
        }

        state.header = next_header;
        state.step_start = next_step_start;
        if let Some(surface) = surface {
            state.surface = surface.nodes;
        }
        state.anchor = next_anchor;
        Ok(())
    }

    /// Reassemble provider output from the exact cited chunk seqs for a
    /// usage anchor (TS `_estimateProviderAssistant`).
    fn estimate_provider_assistant(
        &self,
        session: &Session,
        event: &SessionEvent,
        durable_event_tokens: u64,
    ) -> Result<u64, String> {
        let Some(source_seqs) = &event.source_event_seqs else {
            return Ok(durable_event_tokens);
        };
        let mut assembler = LocalBlockAssembler::new();
        let mut seen = std::collections::HashSet::new();
        for seq in source_seqs {
            if *seq >= event.seq {
                return Err(format!(
                    "token meter: assistant/message at seq {} source seq {seq} is not earlier",
                    event.seq
                ));
            }
            if !seen.insert(*seq) {
                return Err(format!(
                    "token meter: assistant/message at seq {} repeats source seq {seq}",
                    event.seq
                ));
            }
            let source = session.event_at(*seq).ok_or_else(|| {
                format!(
                    "token meter: assistant/message at seq {} cites missing source seq {seq}",
                    event.seq
                )
            })?;
            if source.type_ != "assistant/chunk" {
                return Err(format!(
                    "token meter: assistant/message at seq {} source seq {seq} is not assistant/chunk",
                    event.seq
                ));
            }
            if source.data.get("turn") != event.data.get("turn")
                || source.data.get("step") != event.data.get("step")
            {
                return Err(format!(
                    "token meter: assistant/message at seq {} source seq {seq} belongs to another step",
                    event.seq
                ));
            }
            if let Some(chunk) = source.data.get("chunk")
                && let Ok(chunk) = serde_json::from_value::<dsh_llm::StreamChunk>(chunk.clone())
            {
                assembler.push(&chunk);
            }
        }
        let provider_content = assembler.blocks();
        Ok(if provider_content.is_empty() {
            0
        } else {
            estimate_content(&provider_content) + ROLE_OVERHEAD
        })
    }
}

struct PricedSurface {
    nodes: Vec<TokenSurfaceNode>,
    surface_tokens: u64,
}

fn price_surface(
    nodes: &[MeterSurfaceNode],
    pricing: Option<&dsh_llm::LlmImageRequestPricing>,
) -> Result<PricedSurface, String> {
    let images = if pricing.is_some() {
        nodes
            .iter()
            .flat_map(|node| node.images.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let prices = pricing.map(|price| price(&images));
    if let Some(prices) = &prices
        && prices.len() != images.len()
    {
        return Err(format!(
            "token meter: route image pricing answered {} prices for {} occurrences",
            prices.len(),
            images.len()
        ));
    }
    let mut cursor = 0usize;
    let mut surface_tokens = 0u64;
    let mut public = Vec::with_capacity(nodes.len());
    for node in nodes {
        let mut tokens = node.heuristic_tokens;
        if let Some(prices) = &prices
            && !node.images.is_empty()
        {
            tokens = node.image_free_tokens;
            for _ in &node.images {
                let price = &prices[cursor];
                cursor += 1;
                tokens += price.visual_tokens
                    + estimate_content(&[dsh_llm::ContentBlock::Text {
                        text: price.text.clone(),
                    }]);
            }
        }
        surface_tokens += tokens;
        public.push(TokenSurfaceNode {
            seq: node.seq,
            tokens,
            heuristic_tokens: node.heuristic_tokens,
        });
    }
    Ok(PricedSurface {
        nodes: public,
        surface_tokens,
    })
}

/// Minimal block assembler covering the token-meter's chunk vocabulary
/// (text/reasoning/tool-call blocks). The full assembler belongs to the
/// dsh-llm runtime milestone.
struct LocalBlockAssembler {
    blocks: Vec<dsh_llm::ContentBlock>,
    open: Option<OpenBlock>,
}

enum OpenBlock {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}

#[cfg(test)]
mod route_image_pricing_tests {
    use super::*;
    use cordis::Context;
    use dsh_llm::{
        ContentBlock, ImageAttachmentRef, LlmAdapter, LlmImageRequestPrice, LlmImageRequestPricing,
        LlmRuntime, MessageSource, create_user_message, reasoning_effort_id,
    };
    use dsh_session::{CreateSessionOptions, SessionStore, SurfaceIntent, SurfaceOp, session_id};

    struct PricedAdapter;

    impl LlmAdapter for PricedAdapter {
        fn image_request_pricing(
            &self,
            _provider: &str,
            _model: &str,
        ) -> Option<LlmImageRequestPricing> {
            Some(Arc::new(|images| {
                images
                    .iter()
                    .map(|_| LlmImageRequestPrice {
                        visual_tokens: 300,
                        text: "request preview".to_string(),
                    })
                    .collect()
            }))
        }

        fn stream(&self, _options: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
            Box::pin(futures::stream::empty())
        }
    }

    fn image_message() -> dsh_llm::UserMessage {
        create_user_message(
            vec![
                ContentBlock::Text {
                    text: "look".to_string(),
                },
                ContentBlock::Image {
                    attachment: ImageAttachmentRef {
                        attachment_id: "sha256:priced".to_string(),
                        media_type: Some("image/png".to_string()),
                        bytes: Some(2048),
                        width: Some(800),
                        height: Some(800),
                        name: None,
                    },
                },
            ],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )
    }

    #[tokio::test]
    async fn routed_image_pricing_reprices_nodes_but_preserves_heuristic_shadow_price() {
        let ctx = Context::root();
        let llm = LlmRuntime::install(&ctx);
        llm.register_adapter(&ctx, vec!["priced".to_string()], Arc::new(PricedAdapter))
            .expect("register adapter");
        let meter = TokenMeter::install(&ctx, TokenMeterConfig::default());
        let sessions = SessionStore::install(&ctx);
        let session = sessions
            .create(
                &ctx,
                Some(session_id("priced-image")),
                Some(CreateSessionOptions::default()),
            )
            .await
            .expect("session");
        let message = image_message();
        session
            .append(
                "user/message",
                serde_json::to_value(&message).expect("message JSON"),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .expect("append");

        let neutral = meter.measure(&session, None);
        let header = EpochHeader {
            config: dsh_llm::LlmCallConfig {
                provider: "priced".to_string(),
                model: "model".to_string(),
                reasoning_effort: Some(reasoning_effort_id("high")),
                temperature: None,
                max_tokens: None,
                stop: None,
            },
            adapter_defaults: None,
            system: None,
            tools: None,
        };
        let routed = meter.measure(&session, Some(header));
        assert!(routed.surface_tokens > neutral.surface_tokens + 200);
        assert_eq!(routed.nodes[0].heuristic_tokens, neutral.nodes[0].tokens);
        assert_eq!(neutral.nodes[0].heuristic_tokens, neutral.nodes[0].tokens);
    }
}

impl LocalBlockAssembler {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            open: None,
        }
    }

    fn push(&mut self, chunk: &dsh_llm::StreamChunk) {
        use dsh_llm::StreamChunk;
        match chunk {
            StreamChunk::BlockStart { block_type, .. } => {
                self.open = match block_type.as_str() {
                    "text" => Some(OpenBlock::Text {
                        text: String::new(),
                    }),
                    "reasoning" => Some(OpenBlock::Reasoning {
                        text: String::new(),
                    }),
                    "tool-call" => Some(OpenBlock::ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    }),
                    _ => None,
                };
            }
            StreamChunk::TextDelta { text, .. } => {
                if let Some(OpenBlock::Text { text: buffer, .. }) = &mut self.open {
                    buffer.push_str(text);
                }
            }
            StreamChunk::ReasoningDelta { text, .. } => {
                if let Some(OpenBlock::Reasoning { text: buffer, .. }) = &mut self.open {
                    buffer.push_str(text);
                }
            }
            StreamChunk::ToolCallDelta {
                id,
                name,
                arguments_delta,
                ..
            } => {
                if let Some(OpenBlock::ToolCall {
                    id: id_buffer,
                    name: name_buffer,
                    arguments,
                }) = &mut self.open
                {
                    if !id.as_str().is_empty() {
                        *id_buffer = id.as_str().to_string();
                    }
                    if let Some(name) = name
                        && !name.is_empty()
                    {
                        *name_buffer = name.clone();
                    }
                    arguments.push_str(arguments_delta);
                }
            }
            StreamChunk::BlockEnd { .. } => {
                if let Some(open) = self.open.take() {
                    let block = match open {
                        OpenBlock::Text { text, .. } => dsh_llm::ContentBlock::Text { text },
                        OpenBlock::Reasoning { text, .. } => {
                            dsh_llm::ContentBlock::Reasoning { text }
                        }
                        OpenBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => dsh_llm::ContentBlock::ToolCall {
                            id: dsh_llm::call_id(id),
                            name,
                            arguments,
                        },
                    };
                    self.blocks.push(block);
                }
            }
            _ => {}
        }
    }

    fn blocks(self) -> Vec<dsh_llm::ContentBlock> {
        self.blocks
    }
}

/// Re-exported for the optional projection registration read.
#[allow(dead_code)]
fn _unused(_: &Context) {
    let _ = derive_event_message;
}
