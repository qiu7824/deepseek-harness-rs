#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
fn main() {
    eprintln!("Native desktop controller requires Windows");
    std::process::exit(69)
}

#[cfg(windows)]
fn respond(value: &serde_json::Value) -> bool {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "DSH_DESKTOP_RESPONSE={value}")
        .and_then(|_| out.flush())
        .is_ok()
}

#[cfg(windows)]
fn main() {
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
                    "takeover"
                        | "resume_agent"
                        | "focus_window"
                        | "mouse_move"
                        | "mouse_down"
                        | "mouse_up"
                        | "key_down"
                        | "key_up"
                        | "release_inputs"
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
                    pause_reader.store(true, Ordering::SeqCst)
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
        pause_reader.store(true, Ordering::SeqCst);
        for flag in cancellation_reader.lock().unwrap().values() {
            flag.store(true, Ordering::SeqCst)
        }
    });
    let mut media = None;
    let mut engine: Option<native::Engine> = None;
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
                if let Some(engine) = &engine {
                    engine.check_start_target(args)?;
                }
                if engine.is_none() {
                    if media.is_none() {
                        media = Some(platform::MediaPlatform::new()?);
                    }
                    owner = requested.to_string();
                    paused.store(human, Ordering::SeqCst);
                    engine = Some(native::Engine::open(
                        args,
                        paused.clone(),
                        command.cancelled.clone(),
                        input_generation.clone(),
                    )?);
                }
                return Ok(json!({"state":engine.as_ref().unwrap().state()}));
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
                let code = if message.starts_with("COMPUTER_USE_") {
                    message.clone()
                } else {
                    "COMPUTER_USE_DESKTOP_ERROR".into()
                };
                let diagnostics = native::control_diagnostics();
                let message = if code == "COMPUTER_USE_MANUAL_CONTROL" {
                    format!("{message}; controlDiagnostics={diagnostics}")
                } else {
                    message
                };
                json!({"id":id,"ok":false,"error":{"code":code,"message":message,"controlDiagnostics":diagnostics}})
            }
        };
        if !respond(&response) {
            break;
        }
    }
    drop(engine);
}
