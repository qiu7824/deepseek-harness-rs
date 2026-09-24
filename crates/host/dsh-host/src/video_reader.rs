//! Video evidence is sampled from decoded frames, with explicit audio/subtitle coverage.
use dsh_attachment::{AttachmentStore, ImageMediaType, SaveImageAttachment};
use dsh_tool_computer_use_command::{
    AdapterRequest, ComputerUseAdapter, NativeBrowserAdapter, NativeBrowserConfig,
};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(super) fn install(
    ctx: &cordis::Context,
    settings: Arc<dsh_settings::SettingsProvider>,
) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("tools unavailable")?;
    let context = ctx.clone();
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    tools.register(ctx,ToolDefinition{
        name:"read_video".into(),description:"Inspect an actual video using timestamped decoded frames and available subtitles. Supply a workspace video path, a playable video URL, or browser_session to inspect a video in an existing native-browser session (including a page the user logged into). At most 12 frames are sampled; use times for specific moments. Returned images are visual evidence for analysis, not proof of the entire video. Local audio is transcribed when the existing voice STT command and ffmpeg are available; audio/subtitle coverage is reported explicitly. Never claim to have heard audio when transcription is absent. Use consult_model task=vision with the returned image attachment IDs if a separate vision model is needed.".into(),
        parameters:json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"url":{"type":"string"},"browser_session":{"type":"string"},"selector":{"type":"string"},"times":{"type":"array","maxItems":12,"items":{"type":"number","minimum":0}},"frame_count":{"type":"integer","minimum":1,"maximum":12},"transcribe":{"type":"boolean"},"language":{"type":"string"}}}),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|{let mut output=vec![dsh_llm::ContentBlock::Text{text:value.to_string()}];for frame in value["frames"].as_array().into_iter().flatten(){output.push(dsh_llm::ContentBlock::Text{text:format!("Video frame at {} seconds",frame["timeSeconds"])});output.push(dsh_llm::ContentBlock::Image{attachment:serde_json::from_value(frame["attachment"].clone()).map_err(|e|e.to_string())?,
offloaded: None,
});}Ok(output)}),presentation_meta:None},
        timeout_ms:Some(180000),is_concurrency_safe:Some(Arc::new(|_|false)),finalize_content:None,present_call:None,present_result:None,
        execute:Arc::new(move|args,run|{let ctx=context.clone();let settings=settings.clone();let gate=gate.clone();let args=args.clone();let execution=run.execution.clone();Box::pin(async move{
            let signal=execution.signal.lock().clone();let permit=gate.acquire();tokio::pin!(permit);
            let _permit=loop{tokio::select!{p=&mut permit=>break p.map_err(|_|ToolBodyError::plain("video reader closed"))?,_=tokio::time::sleep(Duration::from_millis(20))=>if signal(){return Err(ToolBodyError::plain("video reading cancelled"));}}};
            read(&ctx,&settings,&args,&execution).await.map_err(|e|ToolBodyError::coded(e,"VideoReadError","VIDEO_READ_FAILED"))
        })})
    })?;
    Ok(())
}
fn setting(settings: &dsh_settings::SettingsProvider, name: &str) -> Value {
    settings
        .get(&dsh_settings::settings_namespace(name).unwrap())
        .and_then(|v| v.to_json())
        .unwrap_or(json!({}))
}
async fn local_path(agent: &Arc<dyn dsh_agent::Agent>, raw: &str) -> Result<PathBuf, String> {
    let root = agent
        .session()
        .header()
        .cwd
        .clone()
        .ok_or("Video files require a workspace")?;
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| e.to_string())?;
    let path = Path::new(raw);
    let path = tokio::fs::canonicalize(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
    .await
    .map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err(
            "Video files must stay inside the workspace; attach external files first".into(),
        );
    }
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["mp4", "webm", "mov", "m4v", "ogv", "ogg"].contains(&ext.as_str()) {
        return Err("Use a browser-playable MP4, WebM, MOV or Ogg video".into());
    }
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > 512 * 1024 * 1024 {
        return Err("Video must be a regular file no larger than 512 MiB".into());
    }
    Ok(path)
}
async fn read(
    ctx: &cordis::Context,
    settings: &dsh_settings::SettingsProvider,
    args: &Value,
    execution: &ToolExecution,
) -> Result<Value, String> {
    let agent = execution
        .agent
        .as_ref()
        .ok_or("Video reading requires a session")?;
    let signal = execution.signal.lock().clone();
    let sources = ["path", "url", "browser_session"]
        .iter()
        .filter(|k| args[**k].as_str().is_some_and(|s| !s.trim().is_empty()))
        .count();
    if sources != 1 {
        return Err("Supply exactly one of path, url or browser_session".into());
    }
    let path = match args["path"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(path) => Some(local_path(agent, path).await?),
        None => None,
    };
    let server = match &path {
        Some(path) => Some(super::video_http::serve(path.clone()).await?),
        None => None,
    };
    let url = server
        .as_ref()
        .map(|s| s.url.as_str())
        .or_else(|| args["url"].as_str());
    let shared = if args["browser_session"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty())
    {
        Some(
            ctx.get_typed::<Arc<dsh_tool_computer_use_command::ComputerUseRuntime>>(
                "computerUse",
                false,
            )
            .ok_or("The browser controller is unavailable")?
            .as_ref()
            .clone(),
        )
    } else {
        None
    };
    if shared
        .as_ref()
        .is_some_and(|r| r.adapter_id() != "native-browser")
    {
        return Err("browser_session requires the native-browser adapter; a UU desktop stream is not a video page".into());
    }
    let config = setting(settings, "computer-use");
    let browser = NativeBrowserAdapter::new(NativeBrowserConfig {
        executable: config["browserExecutable"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        data_root: std::env::temp_dir().join("dsh-video-browser"),
        headless: true,
        max_sessions: 1,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let session = args["browser_session"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("video-reader");
    let call = |mut value: Value| {
        value["sessionId"] = json!(session);
        let signal = signal.clone();
        let shared = shared.clone();
        let browser = &browser;
        let agent = agent.clone();
        async move {
            if let Some(shared) = shared {
                shared
                    .execute_for_agent(agent, &value, signal)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let request = AdapterRequest::from_arguments(&value)
                    .map_err(|e| e.to_string())?
                    .with_owner_id(agent.id().as_str());
                browser
                    .execute(request, signal)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    };
    let work = async {
        if let Some(url) = url {
            call(json!({"action":"start","url":url,"includeScreenshot":false})).await?;
        }
        let selector = args["selector"].as_str().unwrap_or("video");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let mut info = loop {
            match call(json!({"action":"video_info","selector":selector})).await {
                Ok(value) => break value.value["video"].clone(),
                Err(error) => {
                    if signal() || tokio::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        };
        let times = frame_times(
            args,
            info["duration"].as_f64(),
            info["currentTime"].as_f64().unwrap_or(0.0),
        )?;
        let store = ctx
            .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
            .ok_or("Image attachment store is unavailable")?;
        let automatic = args.get("times").is_none();
        let mut frames = Vec::new();
        let mut bytes = 0usize;
        for (index, requested_time) in times.into_iter().enumerate() {
            let mut retries = 0;
            let (time, output) = loop {
                let time = if automatic {
                    let latest = call(json!({"action":"video_info","selector":selector})).await?;
                    ensure_same_video(&info, &latest.value["video"])?;
                    info = latest.value["video"].clone();
                    automatic_frame_time(args, &info, index)?
                } else {
                    requested_time
                };
                match call(json!({"action":"video_frame","selector":selector,"timeSeconds":time,"includeScreenshot":true})).await {
                    Ok(output) => {
                        ensure_same_video(&info, &output.value["video"])?;
                        break (time, output);
                    }
                    Err(error) if automatic && retries < 2 && error.contains("Frame time is outside the video duration") => {
                        // A streaming timeline can shrink between info and seek.
                        // Refresh automatic samples only; explicit timestamps keep
                        // their original meaning and must never be silently clamped.
                        retries += 1;
                    }
                    Err(error) => return Err(error),
                }
            };
            let screenshot = output.screenshot.ok_or("Video returned no decoded frame")?;
            bytes = bytes.saturating_add(screenshot.data.len());
            if bytes > 32 * 1024 * 1024 {
                return Err("Sampled frames exceeded 32 MiB".into());
            }
            let image = store
                .save_image(&SaveImageAttachment {
                    data: screenshot.data,
                    media_type: ImageMediaType::Png,
                    name: Some(format!("video-frame-{time:.3}s.png")),
                })
                .await
                .map_err(|e| e.to_string())?;
            frames.push(
                json!({"timeSeconds":output.value["video"]["currentTime"],"captureMethod":output.value["video"]["captureMethod"],"attachment":image}),
            );
        }
        let mut subtitles = Value::Null;
        if let Some(path) = &path {
            for ext in ["vtt", "srt"] {
                let sidecar = path.with_extension(ext);
                if let Ok(meta) = tokio::fs::symlink_metadata(&sidecar).await {
                    if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 128 * 1024
                    {
                        if let Ok(text) = tokio::fs::read_to_string(&sidecar).await {
                            subtitles = json!({"kind":ext,"text":text.chars().take(32000).collect::<String>(),"truncated":text.chars().count()>32000});
                            break;
                        }
                    }
                }
            }
        }
        let voice = setting(settings, "voice");
        let audio = if args["transcribe"] == false {
            json!({"status":"disabled"})
        } else if let (Some(path), Some(command)) = (
            &path,
            voice["sttCommand"]
                .as_str()
                .filter(|s| !s.trim().is_empty()),
        ) {
            match transcribe(
                path,
                command,
                args["language"].as_str().unwrap_or(""),
                signal.clone(),
            )
            .await
            {
                Ok(text) => {
                    json!({"status":"transcribed","truncated":info["duration"].as_f64().is_some_and(|d|d>600.0)||text.chars().count()>=32000,"text":text,"maxAudioSeconds":600})
                }
                Err(error) => json!({"status":"unavailable","reason":error}),
            }
        } else {
            json!({"status":"not-transcribed","reason":"Local audio requires the existing voice STT command and ffmpeg; remote browser playback provides frames and loaded subtitles only"})
        };
        let source = ["path", "url", "browser_session"]
            .iter()
            .find_map(|k| args[*k].as_str().filter(|s| !s.trim().is_empty()));
        Ok(
            json!({"source":source,"duration":info["duration"],"frames":frames,"subtitles":subtitles,"browserSubtitleTracks":info["tracks"],"audio":audio,"coverage":"Only the listed timestamps were visually inspected. Audio evidence is available only when audio.status is transcribed."}),
        )
    };
    let result = tokio::time::timeout(Duration::from_secs(170), work)
        .await
        .map_err(|_| "Video reading timed out".to_string())
        .and_then(|r| r);
    let _ = tokio::time::timeout(Duration::from_secs(5), browser.shutdown()).await;
    result
}
fn ensure_same_video(previous: &Value, current: &Value) -> Result<(), String> {
    for field in ["source", "elementId"] {
        let (Some(previous), Some(current)) = (previous[field].as_str(), current[field].as_str())
        else {
            return Err(
                "Video identity is unavailable; refresh the browser session before sampling".into(),
            );
        };
        if previous != current || (field == "elementId" && previous.is_empty()) {
            return Err(
                "Video source changed during sampling; retry after playback stabilizes".into(),
            );
        }
    }
    Ok(())
}
fn automatic_frame_time(args: &Value, info: &Value, index: usize) -> Result<f64, String> {
    let times = frame_times(
        args,
        info["duration"].as_f64(),
        info["currentTime"].as_f64().unwrap_or(0.0),
    )?;
    times.get(index).copied().ok_or_else(|| {
        "Video timeline changed during sampling; retry after playback stabilizes".into()
    })
}
fn frame_times(args: &Value, duration: Option<f64>, current: f64) -> Result<Vec<f64>, String> {
    if let Some(times) = args["times"].as_array() {
        if times.is_empty() || times.len() > 12 {
            return Err("Supply 1 to 12 timestamps".into());
        }
        return times
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|t| t.is_finite() && *t >= 0.0 && duration.is_none_or(|d| *t < d))
                    .ok_or("Timestamp is outside the video".into())
            })
            .collect();
    }
    let count = match args.get("frame_count") {
        None => 6,
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && n.fract() == 0.0 && (1.0..=12.0).contains(n))
            .ok_or("frame_count must be an integer between 1 and 12")?
            as usize,
    };
    match duration.filter(|d| d.is_finite() && *d > 0.0) {
        Some(d) => Ok((0..count)
            .map(|i| (d - 0.05).max(0.0) * (i as f64) / (count.saturating_sub(1).max(1) as f64))
            .collect()),
        None => Ok(vec![current]),
    }
}
struct AudioTemp(PathBuf);
impl Drop for AudioTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn transcribe(
    path: &Path,
    command: &str,
    language: &str,
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<String, String> {
    let directory = std::env::temp_dir().join(format!("dsh-video-audio-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&directory)
        .await
        .map_err(|e| e.to_string())?;
    let _guard = AudioTemp(directory.clone());
    let audio = directory.join("audio.wav");
    let mut ffmpeg = tokio::process::Command::new("ffmpeg");
    ffmpeg
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-t", "600"])
        .arg(&audio)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    ffmpeg.creation_flags(0x08000000);
    let work = async {
        let status = ffmpeg
            .status()
            .await
            .map_err(|e| format!("ffmpeg is required for audio extraction: {e}"))?;
        if !status.success() {
            return Err("ffmpeg could not decode the audio track".into());
        }
        let output = dsh_native_command::run_native_command(
            command,
            &[audio.to_string_lossy().into_owned(), language.into()],
            Some(signal.clone()),
        )
        .await
        .map_err(|e| e.to_string())?;
        let text = output.stdout.trim();
        if text.is_empty() {
            return Err("STT returned no transcript".into());
        }
        Ok(text.chars().take(32000).collect())
    };
    let work = tokio::time::timeout(Duration::from_secs(60), work);
    tokio::pin!(work);
    loop {
        tokio::select! {result=&mut work=>return result.map_err(|_|"Audio transcription timed out".to_string())?,_=tokio::time::sleep(Duration::from_millis(20))=>if signal(){return Err("Audio transcription cancelled".into());}}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_sampling_refreshes_duration_without_rewriting_explicit_times() {
        let args = json!({"frame_count": 3});
        assert_eq!(
            automatic_frame_time(&args, &json!({"duration": 10}), 2).unwrap(),
            9.95
        );
        assert_eq!(
            automatic_frame_time(&args, &json!({"duration": 2}), 2).unwrap(),
            1.95
        );
        assert!(frame_times(&json!({"times":[9.95]}), Some(2.0), 0.0).is_err());
        assert!(
            automatic_frame_time(&args, &json!({"duration":null,"currentTime":23}), 2).is_err()
        );
    }
    #[test]
    fn frames_from_replaced_media_are_not_mixed() {
        assert!(
            ensure_same_video(
                &json!({"source":"blob:first","elementId":"first"}),
                &json!({"source":"blob:second","elementId":"first"})
            )
            .is_err()
        );
        assert!(
            ensure_same_video(
                &json!({"source":"blob:first","elementId":"first","duration":10}),
                &json!({"source":"blob:first","elementId":"first","duration":2})
            )
            .is_ok()
        );
        assert!(
            ensure_same_video(
                &json!({"source":"same-url","elementId":"first"}),
                &json!({"source":"same-url","elementId":"replacement"})
            )
            .is_err()
        );
        assert!(
            ensure_same_video(&json!({"source":"same-url"}), &json!({"source":"same-url"}))
                .is_err()
        );
        assert!(
            ensure_same_video(
                &json!({"source":"","elementId":"live-stream"}),
                &json!({"source":"","elementId":"live-stream"})
            )
            .is_ok()
        );
    }
    #[test]
    fn sampling_is_bounded_and_timestamped() {
        assert_eq!(
            frame_times(&json!({"frame_count":3}), Some(10.0), 0.0).unwrap(),
            vec![0.0, 4.975, 9.95]
        );
        assert!(frame_times(&json!({"times":[10]}), Some(10.0), 0.0).is_err());
        assert_eq!(frame_times(&json!({}), None, 23.0).unwrap(), vec![23.0]);
        assert_eq!(
            frame_times(&json!({"frame_count":3.0}), Some(10.0), 0.0)
                .unwrap()
                .len(),
            3
        );
        assert!(frame_times(&json!({"frame_count":0}), Some(10.0), 0.0).is_err());
    }
}
