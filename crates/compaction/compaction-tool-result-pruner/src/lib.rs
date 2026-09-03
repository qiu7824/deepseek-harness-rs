use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc};
use dsh_llm::{ContentBlock, Message};
use dsh_session::{Session, SurfaceIntent, SurfaceOp};
use dsh_token_meter::TokenMeter;
use serde::{Deserialize, Serialize};

pub const PRUNE_MARKER: &str = "\n\n[... tool result middle pruned ...]\n\n";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResultPruneConfig {
    pub threshold_chars: Option<usize>,
    pub head_chars: Option<usize>,
    pub tail_chars: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub threshold_chars: usize,
    pub head_chars: usize,
    pub tail_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrunedEntry {
    pub original_seq: u64,
    pub replacement_seq: u64,
    pub call_id: String,
    pub chars_before: usize,
    pub chars_after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PruneResult {
    pub pruned: Vec<PrunedEntry>,
    pub chars_removed: usize,
}

pub fn code_point_length(text: &str) -> usize {
    text.chars().count()
}

pub fn resolve_config(config: ToolResultPruneConfig) -> Result<ResolvedConfig, String> {
    let resolved = ResolvedConfig {
        threshold_chars: config.threshold_chars.unwrap_or(8192),
        head_chars: config.head_chars.unwrap_or(4096),
        tail_chars: config.tail_chars.unwrap_or(1024),
    };
    if resolved.threshold_chars == 0 {
        return Err("ToolResultPruneConfig: thresholdChars (0) must be a positive integer".into());
    }
    let emitted = resolved.head_chars + code_point_length(PRUNE_MARKER) + resolved.tail_chars;
    if emitted > resolved.threshold_chars {
        return Err(format!(
            "ToolResultPruneConfig: headChars + marker + tailChars ({emitted}) must be at most thresholdChars ({})",
            resolved.threshold_chars
        ));
    }
    Ok(resolved)
}

pub struct ToolResultPruner {
    pub config: ResolvedConfig,
    meter: Option<Arc<TokenMeter>>,
}

impl Service for ToolResultPruner {
    fn service_name(&self) -> &'static str {
        "toolResultPruner"
    }
}

impl ToolResultPruner {
    pub fn standalone(config: ResolvedConfig) -> Self {
        Self {
            config,
            meter: None,
        }
    }
    pub fn install(ctx: &Context, config: ResolvedConfig) -> Result<Arc<Self>, String> {
        let meter = ctx
            .get_typed::<Arc<TokenMeter>>("tokenMeter", false)
            .map(|value| value.as_ref().clone())
            .ok_or_else(|| "compaction-tool-result-pruner requires tokenMeter".to_string())?;
        let service = Arc::new(Self {
            config,
            meter: Some(meter),
        });
        ctx.register_service(service.clone());
        Ok(service)
    }
    pub fn measure_content(&self, blocks: &[ContentBlock]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => code_point_length(text),
                _ => 0,
            })
            .sum()
    }
    pub fn prune_content(&self, blocks: &[ContentBlock]) -> Option<Vec<ContentBlock>> {
        let total = self.measure_content(blocks);
        if total <= self.config.threshold_chars {
            return None;
        }
        let removed_start = self.config.head_chars;
        let removed_end = total - self.config.tail_chars;
        let mut output = Vec::new();
        let mut consumed = 0;
        let mut marker = false;
        for block in blocks {
            let ContentBlock::Text { text } = block else {
                output.push(block.clone());
                continue;
            };
            let points: Vec<char> = text.chars().collect();
            let block_start = consumed;
            let block_end = consumed + points.len();
            let head_end = points.len().min(removed_start.saturating_sub(block_start));
            let tail_start = points.len().min(removed_end.saturating_sub(block_start));
            let intersects = block_start < removed_end && block_end > removed_start;
            let mut replacement: String = points[..head_end].iter().collect();
            if intersects && !marker {
                replacement.push_str(PRUNE_MARKER);
                marker = true;
            }
            replacement.extend(points[tail_start..].iter());
            if !replacement.is_empty() {
                output.push(ContentBlock::Text { text: replacement });
            }
            consumed = block_end;
        }
        Some(output)
    }
    pub fn prune_session(&self, session: &Session) -> Result<PruneResult, String> {
        let candidates: Vec<_> = session
            .surface()?
            .nodes
            .into_iter()
            .filter_map(|seq| {
                session
                    .event_at(dsh_session::SessionSeq::new(seq).ok()?)
                    .filter(|event| event.type_ == "tool/result")
                    .map(|event| (seq, event))
            })
            .collect();
        let mut result = PruneResult::default();
        for (seq, event) in candidates {
            let message: Message = serde_json::from_value(
                event
                    .data
                    .get("message")
                    .cloned()
                    .ok_or("tool/result missing message")?,
            )
            .map_err(|error| error.to_string())?;
            let Some(ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            }) = message.content.first()
            else {
                continue;
            };
            let Some(pruned_content) = self.prune_content(content) else {
                continue;
            };
            let before = self.measure_content(content);
            let after = self.measure_content(&pruned_content);
            let mut replacement_message = message.clone();
            replacement_message.content[0] = ContentBlock::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: pruned_content,
                is_error: *is_error,
            };
            let meter = self
                .meter
                .as_ref()
                .ok_or("prune_session requires an installed token meter")?;
            session.append("compaction/prune", serde_json::json!({ "shadowedRange": { "start": seq, "end": seq }, "shadowedSeqs": [seq], "shadowedTokenCount": meter.estimate_message(&message) }), None)?;
            let mut data = event.data.clone();
            data["message"] =
                serde_json::to_value(&replacement_message).map_err(|error| error.to_string())?;
            let replacement = session.append(
                "tool/result",
                data,
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace {
                        start: seq,
                        end: seq,
                    },
                    source_event_seqs: Some(vec![seq]),
                }),
            )?;
            result.pruned.push(PrunedEntry {
                original_seq: seq,
                replacement_seq: replacement.seq.get(),
                call_id: tool_call_id.as_str().to_string(),
                chars_before: before,
                chars_after: after,
            });
            result.chars_removed += before - after;
        }
        Ok(result)
    }
}

pub struct ToolResultPrunerPlugin;
#[async_trait::async_trait]
impl Plugin for ToolResultPrunerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("compaction-tool-result-pruner")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["tokenMeter"])
    }
    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let raw = config
            .downcast_ref::<serde_json::Value>()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let config: ToolResultPruneConfig = serde_json::from_value(raw)
            .map_err(|error| PluginError::new(arc(error.to_string())))?;
        let config = resolve_config(config).map_err(|error| PluginError::new(arc(error)))?;
        ToolResultPruner::install(ctx, config).map_err(|error| PluginError::new(arc(error)))?;
        Ok(())
    }
}

pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(ToolResultPrunerPlugin)
}
