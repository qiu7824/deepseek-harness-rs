use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub stt_command: Option<String>,
    pub tts_command: Option<String>,
    pub timeout_ms: u64,
}

fn requires_approval(tool_name: &str) -> bool {
    tool_name == "voice_transcribe"
}

pub fn install(ctx: &Context, config: Config) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "voice command requires the tools service".to_string())?;
    let timeout_ms = config.timeout_ms.clamp(5_000, 300_000);

    let listener: Arc<Listener> = Arc::new(|_ctx, args| {
        let execution = args
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|slot| slot.as_ref().clone());
        let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
        Box::pin(async move {
            if execution
                .as_ref()
                .is_some_and(|execution| requires_approval(&execution.name))
            {
                return Some(arc(PreToolDecision::Ask {
                    reason: Some(
                        "语音转写将允许外部 STT 命令读取所选本地音频文件，需要用户确认".to_string(),
                    ),
                    grant_key: Some("tool:voice-input".to_string()),
                    rememberable: true,
                }));
            }
            match next {
                Some(next) => Some(next.call().await),
                None => Some(arc(PreToolDecision::Allow)),
            }
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    ));

    if let Some(command) = config.stt_command.filter(|value| !value.trim().is_empty()) {
        tools.register(
            ctx,
            ToolDefinition {
                name: "voice_transcribe".to_string(),
                description: "Transcribe an audio file with the configured external STT command.".to_string(),
                parameters: serde_json::json!({
                    "type": "object", "additionalProperties": false,
                    "properties": { "audioPath": { "type": "string" }, "language": { "type": "string" } },
                    "required": ["audioPath"]
                }),
                output: text_output("text"),
                timeout_ms: Some(timeout_ms),
                is_concurrency_safe: Some(Arc::new(|_| true)),
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let command = command.clone();
                    let audio = args.get("audioPath").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    let language = args.get("language").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    let signal = run.signal.lock().clone();
                    Box::pin(async move {
                        let output = dsh_native_command::run_native_command(
                            &command,
                            &[audio, language],
                            Some(signal),
                        )
                        .await
                        .map_err(|error| ToolBodyError::plain(format!("STT command failed: {error}")))?;
                        let text = output.stdout.trim().to_string();
                        if text.is_empty() {
                            return Err(ToolBodyError::plain("STT command returned empty text"));
                        }
                        Ok(serde_json::json!({ "text": text }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;
    }

    if let Some(command) = config.tts_command.filter(|value| !value.trim().is_empty()) {
        tools.register(
            ctx,
            ToolDefinition {
                name: "voice_synthesize".to_string(),
                description:
                    "Synthesize text to an audio file with the configured external TTS command."
                        .to_string(),
                parameters: serde_json::json!({
                    "type": "object", "additionalProperties": false,
                    "properties": { "text": { "type": "string" }, "voice": { "type": "string" } },
                    "required": ["text"]
                }),
                output: text_output("audioPath"),
                timeout_ms: Some(timeout_ms),
                is_concurrency_safe: Some(Arc::new(|_| true)),
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let command = command.clone();
                    let text = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let voice = args
                        .get("voice")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let signal = run.signal.lock().clone();
                    Box::pin(async move {
                        if text.trim().is_empty() {
                            return Err(ToolBodyError::plain("TTS text must be non-empty"));
                        }
                        let output = dsh_native_command::run_native_command(
                            &command,
                            &[text, voice],
                            Some(signal),
                        )
                        .await
                        .map_err(|error| {
                            ToolBodyError::plain(format!("TTS command failed: {error}"))
                        })?;
                        let path = output.stdout.trim().to_string();
                        if path.is_empty() {
                            return Err(ToolBodyError::plain("TTS command returned no audio path"));
                        }
                        Ok(serde_json::json!({ "audioPath": path }))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;
    }
    Ok(())
}

fn text_output(field: &'static str) -> ToolOutputDefinition {
    let mut properties = serde_json::Map::new();
    properties.insert(field.to_string(), serde_json::json!({ "type": "string" }));
    ToolOutputDefinition {
        schema: serde_json::json!({
            "type": "object", "additionalProperties": false,
            "properties": properties,
            "required": [field]
        }),
        render: Arc::new(move |_args, value| {
            Ok(vec![dsh_llm::ContentBlock::Text {
                text: value
                    .get(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            }])
        }),
        presentation_meta: None,
    }
}
