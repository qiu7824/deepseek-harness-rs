#[cfg(windows)]
mod business;
mod input;
mod profile;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod sdk;
#[cfg(windows)]
mod wire;

#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod video;
#[cfg(not(windows))]
fn main() {
    eprintln!("UU desktop controller requires Windows");
    std::process::exit(69)
}

#[cfg(windows)]
fn respond(value: &serde_json::Value) -> bool {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "DSH_UU_RESPONSE={value}")
        .and_then(|_| out.flush())
        .is_ok()
}

fn worker_error_code(message: &str) -> &str {
    let code = message.split([';', ':']).next().unwrap_or("");
    if code.starts_with("COMPUTER_USE_")
        && code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        code
    } else {
        "COMPUTER_USE_UU_ERROR"
    }
}

#[cfg(windows)]
fn main() {
    if std::env::args().any(|arg| arg == "--check-client") {
        let result = std::env::var_os("DSH_UU_INSTALL_DIR")
            .ok_or_else(|| "未找到 UU 安装目录".to_string())
            .and_then(|dir| profile::load(&std::path::PathBuf::from(dir).join("GameViewer.exe")))
            .map(|profile| serde_json::json!({"clientVersion":profile.version,"requestBuild":profile.build,"clientSha256":profile.hash,"signingDataRva":format!("0x{:x}",profile.signing_key_rva),"exactProfile":profile.exact_profile}));
        respond(&serde_json::json!({"ok":result.is_ok(),"result":result}));
        return;
    }
    if std::env::args().any(|arg| arg == "--check-engine") {
        let result = platform::MediaPlatform::new().and_then(|_media| sdk::check_engine());
        respond(&serde_json::json!({"ok":result.is_ok(),"result":result}));
        return;
    }
    use serde_json::{Value, json};
    use std::{
        collections::{HashMap, HashSet},
        io::{BufRead, Read},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        time::Duration,
    };
    struct Command {
        value: Value,
        cancelled: Arc<AtomicBool>,
        input_generation: u64,
    }
    let input_generation = Arc::new(AtomicU64::new(0));
    let input_reader = input_generation.clone();
    let paused = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    let cancellations = Arc::new(Mutex::new(HashMap::<u64, Arc<AtomicBool>>::new()));
    let (tx, rx) = mpsc::sync_channel::<Command>(8);
    let pause_reader = paused.clone();
    let stop_reader = stopped.clone();
    let cancellation_reader = cancellations.clone();
    std::thread::spawn(move || {
        let mut early_cancellations = HashSet::new();
        let input = std::io::stdin();
        let mut input = input.lock();
        loop {
            let mut bytes = Vec::new();
            let result = Read::by_ref(&mut input)
                .take(65537)
                .read_until(b'\n', &mut bytes);
            if !matches!(result,Ok(n)if n>0) || bytes.len() > 65536 {
                break;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                break;
            };
            let action = value["arguments"]["action"].as_str().unwrap_or("");
            if action == "cancel" {
                if let Some(id) = value["cancelId"].as_u64() {
                    if let Some(flag) = cancellation_reader.lock().unwrap().get(&id) {
                        flag.store(true, Ordering::SeqCst)
                    } else {
                        if early_cancellations.len() >= 64 {
                            break;
                        }
                        early_cancellations.insert(id);
                    }
                }
                continue;
            }
            if action == "close" {
                break;
            }
            if value["origin"] == "human"
                && matches!(
                    action,
                    "mouse_move"
                        | "mouse_down"
                        | "mouse_up"
                        | "key_down"
                        | "key_up"
                        | "release_inputs"
                        | "takeover"
                        | "resume_agent"
                        | "click"
                        | "double_click"
                        | "drag"
                        | "scroll"
                        | "key"
                        | "keypress"
                        | "type"
                        | "input"
                )
            {
                input_reader.fetch_add(1, Ordering::SeqCst);
                if action != "release_inputs" {
                    sdk::set_pause_state(
                        &pause_reader,
                        match action {
                            "takeover" => sdk::PauseReason::GuiTakeover,
                            "resume_agent" => sdk::PauseReason::ResumePending,
                            _ => sdk::PauseReason::GuiInput,
                        },
                    );
                }
            }
            let Some(id) = value["id"].as_u64() else {
                break;
            };
            let cancelled = Arc::new(AtomicBool::new(early_cancellations.remove(&id)));
            {
                let mut table = cancellation_reader.lock().unwrap();
                if table.len() >= 8 || table.contains_key(&id) {
                    drop(table);
                    respond(
                        &json!({"id":id,"ok":false,"error":{"code":"COMPUTER_USE_BUSY","message":"控制命令队列繁忙"}}),
                    );
                    continue;
                }
                table.insert(id, cancelled.clone());
            }
            if tx
                .try_send(Command {
                    value,
                    cancelled,
                    input_generation: input_reader.load(Ordering::SeqCst),
                })
                .is_err()
            {
                cancellation_reader.lock().unwrap().remove(&id);
                respond(
                    &json!({"id":id,"ok":false,"error":{"code":"COMPUTER_USE_BUSY","message":"控制命令队列繁忙"}}),
                );
            }
        }
        stop_reader.store(true, Ordering::SeqCst);
        for flag in cancellation_reader.lock().unwrap().values() {
            flag.store(true, Ordering::SeqCst)
        }
        sdk::set_pause_state(&pause_reader, sdk::PauseReason::ReaderShutdown);
    });
    let mut media = None;
    let mut engine: Option<sdk::Engine> = None;
    let mut owner = String::new();
    while !stopped.load(Ordering::SeqCst) {
        let command = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(v) => v,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(engine) = &mut engine {
                    engine.poll()
                }
                continue;
            }
            Err(_) => break,
        };
        let request = &command.value;
        let id = request["id"].as_u64().unwrap();
        let args = &request["arguments"];
        let action = args["action"].as_str().unwrap_or("");
        let human = request["origin"] == "human";
        let result = (|| -> Result<Value, String> {
            if command.cancelled.load(Ordering::SeqCst) {
                return Err("COMPUTER_USE_ABORTED".into());
            }
            if action == "start" {
                let requested = request["ownerId"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 256)
                    .ok_or("缺少会话归属")?;
                if engine.is_some() && requested != owner {
                    return Err("控制会话归属不匹配".into());
                }
                if engine.is_none() {
                    if media.is_none() {
                        media = Some(platform::MediaPlatform::new()?);
                    }
                    let binding = &request["binding"];
                    let account = business::Account::open(
                        std::path::PathBuf::from(
                            std::env::var_os("DSH_UU_INSTALL_DIR").ok_or("未找到 UU 安装目录")?,
                        ),
                        binding["account"].as_str().unwrap_or(""),
                    )?;
                    let (room, name) = account.join(binding["deviceId"].as_str().unwrap_or(""))?;
                    if command.cancelled.load(Ordering::SeqCst) {
                        return Err("COMPUTER_USE_ABORTED".into());
                    }
                    owner = requested.to_string();
                    sdk::set_pause_state(
                        &paused,
                        if human {
                            sdk::PauseReason::StartHuman
                        } else {
                            sdk::PauseReason::Agent
                        },
                    );
                    engine = Some(sdk::Engine::open(
                        room,
                        &account.device_id,
                        name,
                        paused.clone(),
                        command.cancelled.clone(),
                        input_generation.clone(),
                    )?);
                }
                return Ok(json!({"state":engine.as_ref().unwrap().state_for(human)}));
            }
            if request["ownerId"].as_str() != Some(owner.as_str()) {
                return Err("控制会话归属不匹配".into());
            }
            if action == "takeover" || action == "resume_agent" {
                if !human {
                    return Err("COMPUTER_USE_HUMAN_REQUIRED".into());
                }
                engine
                    .as_mut()
                    .ok_or("控制会话不存在")?
                    .control_mode(action == "takeover")?;
                return Ok(json!({"state":engine.as_ref().ok_or("控制会话不存在")?.state()}));
            }
            engine.as_mut().ok_or("控制会话不存在")?.act(
                args,
                human,
                command.cancelled.clone(),
                command.input_generation,
            )
        })();
        cancellations.lock().unwrap().remove(&id);
        let response = match result {
            Ok(mut value) => {
                value["control"] =
                    json!({"mode":if paused.load(Ordering::SeqCst){"manual"}else{"agent"}});
                json!({"id":id,"ok":true,"value":value})
            }
            Err(message) => {
                json!({"id":id,"ok":false,"error":{"code":worker_error_code(&message),"message":message}})
            }
        };
        if !respond(&response) {
            break;
        }
    }
    drop(engine);
}

#[cfg(test)]
mod error_tests {
    #[test]
    fn diagnostic_suffixes_do_not_change_error_identity() {
        assert_eq!(
            super::worker_error_code(
                "COMPUTER_USE_MANUAL_CONTROL; controlDiagnostics={\"pauseReason\":\"escape-hotkey\"}"
            ),
            "COMPUTER_USE_MANUAL_CONTROL"
        );
        assert_eq!(
            super::worker_error_code("COMPUTER_USE_DESKTOP_DISCONNECTED"),
            "COMPUTER_USE_DESKTOP_DISCONNECTED"
        );
        assert_eq!(
            super::worker_error_code("COMPUTER_USE_ABORTED"),
            "COMPUTER_USE_ABORTED"
        );
        assert_eq!(
            super::worker_error_code("COMPUTER_USE_ PRIVATE_TEXT"),
            "COMPUTER_USE_UU_ERROR"
        );
    }
}
