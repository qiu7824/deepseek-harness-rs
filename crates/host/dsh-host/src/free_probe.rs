//! Anonymous protocol checks. These results never substitute for a release binary test.
use serde_json::{Value, json};

const CHECK: &str = "Call connectivity_check exactly once with status ok. After receiving the tool result, reply with OK.";

async fn stream(
    client: &reqwest::Client,
    endpoint: &str,
    body: Value,
) -> Result<Vec<Value>, String> {
    let response = client
        .post(endpoint)
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "请求超时".to_string()
            } else {
                "无法连接免费模型服务".to_string()
            }
        })?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    if !response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"))
    {
        return Err("服务没有返回流式响应".into());
    }
    use futures::StreamExt;
    let mut chunks = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| "流式连接中断".to_string())?;
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("流式响应超过检测预算".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_events(&bytes)
}

fn parse_events(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "响应不是 UTF-8".to_string())?;
    let mut events = Vec::new();
    for line in text.lines().filter_map(|line| line.strip_prefix("data:")) {
        let value = line.trim();
        if value == "[DONE]" {
            events.push(json!({"done":true}));
        } else if !value.is_empty() {
            events.push(serde_json::from_str(value).map_err(|_| "流式 JSON 格式错误".to_string())?);
        }
    }
    if events.len() < 2 {
        return Err("未收到完整流式事件".into());
    }
    Ok(events)
}

fn completion(events: &[Value]) -> Result<(String, Vec<Value>), String> {
    let mut calls = std::collections::BTreeMap::<u64, Value>::new();
    let mut text = String::new();
    let mut finished = false;
    for event in events {
        if let Some(error) = event.get("error") {
            return Err(format!(
                "模型服务返回错误：{}",
                error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
        for choice in event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                if !matches!(reason, "stop" | "tool_calls") {
                    return Err(format!("响应未完整结束：{reason}"));
                }
                finished = true;
            }
            if let Some(part) = choice.pointer("/delta/content").and_then(Value::as_str) {
                text.push_str(part);
            }
            for call in choice
                .pointer("/delta/tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let slot = calls
                    .entry(call.get("index").and_then(Value::as_u64).unwrap_or(0))
                    .or_insert_with(
                        || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                    );
                for (source, target) in [
                    ("/id", "/id"),
                    ("/function/name", "/function/name"),
                    ("/function/arguments", "/function/arguments"),
                ] {
                    if let Some(value) = call
                        .pointer(source)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        let current = slot.pointer(target).and_then(Value::as_str).unwrap_or("");
                        let next = if source.ends_with("arguments")
                            || !current.is_empty() && source.ends_with("name") && current != value
                        {
                            format!("{current}{value}")
                        } else {
                            value.to_string()
                        };
                        *slot.pointer_mut(target).unwrap() = Value::String(next);
                    }
                }
            }
        }
    }
    if !finished
        || !events
            .iter()
            .any(|e| e.get("done") == Some(&Value::Bool(true)))
    {
        return Err("流式响应缺少结束事件".into());
    }
    Ok((text, calls.into_values().collect()))
}

fn responses(events: &[Value]) -> Result<(String, Vec<Value>), String> {
    if events.iter().any(|e| {
        matches!(
            e.get("type").and_then(Value::as_str),
            Some("response.failed" | "response.incomplete" | "error")
        )
    }) {
        return Err("Responses 未成功完成".into());
    }
    let response = events
        .iter()
        .find(|e| e.get("type").and_then(Value::as_str) == Some("response.completed"))
        .and_then(|e| e.get("response"))
        .ok_or("缺少 response.completed")?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            calls.push(item.clone());
        }
        for part in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(value) = part.get("text").and_then(Value::as_str) {
                text.push_str(value);
            }
        }
    }
    if text.is_empty() {
        for event in events {
            if event.get("type").and_then(Value::as_str) == Some("response.output_text.delta") {
                text.push_str(event.get("delta").and_then(Value::as_str).unwrap_or(""));
            }
        }
    }
    Ok((text, calls))
}

fn checked_call(calls: &[Value], responses_api: bool) -> Result<&Value, String> {
    if calls.len() != 1 {
        return Err("检测要求一次工具调用，模型未按约定返回".into());
    }
    let call = &calls[0];
    let function = if responses_api {
        call
    } else {
        &call["function"]
    };
    if function["name"] != "connectivity_check" {
        return Err("工具名称不正确".into());
    }
    let args: Value = serde_json::from_str(function["arguments"].as_str().ok_or("工具参数缺失")?)
        .map_err(|_| "工具参数不是合法 JSON")?;
    if args != json!({"status":"ok"}) {
        return Err("工具参数未通过检查".into());
    }
    if call[if responses_api { "call_id" } else { "id" }]
        .as_str()
        .is_none_or(str::is_empty)
    {
        return Err("工具调用 ID 缺失".into());
    }
    Ok(call)
}

pub(super) async fn verify(
    client: &reqwest::Client,
    model: &str,
    api: &str,
) -> Result<Value, String> {
    let parameters = json!({"type":"object","properties":{"status":{"type":"string","enum":["ok"]}},"required":["status"],"additionalProperties":false});
    let text = if api == "openai-responses" {
        let endpoint = "https://opencode.ai/zen/v1/responses";
        let events = stream(client, endpoint, json!({"model":model,"stream":true,"store":false,"max_output_tokens":1024,"input":[{"role":"user","content":CHECK}],"tools":[{"type":"function","name":"connectivity_check","description":"Confirm the connection works","parameters":parameters}]})).await?;
        let (_, calls) = responses(&events)?;
        let call = checked_call(&calls, true)?;
        let input = json!([{"role":"user","content":CHECK},{"type":"function_call","call_id":call["call_id"],"name":call["name"],"arguments":call["arguments"]},{"type":"function_call_output","call_id":call["call_id"],"output":"{\"status\":\"ok\"}"}]);
        let (text, extra) = responses(&stream(client, endpoint, json!({"model":model,"stream":true,"store":false,"max_output_tokens":1024,"input":input})).await?)?;
        if !extra.is_empty() {
            return Err("工具结果续接后仍请求工具调用".into());
        }
        text
    } else if api == "openai-completions" {
        let endpoint = "https://opencode.ai/zen/v1/chat/completions";
        let events = stream(client, endpoint, json!({"model":model,"stream":true,"max_tokens":1024,"messages":[{"role":"user","content":CHECK}],"tools":[{"type":"function","function":{"name":"connectivity_check","description":"Confirm the connection works","parameters":parameters}}]})).await?;
        let (_, calls) = completion(&events)?;
        let call = checked_call(&calls, false)?;
        let messages = json!([{"role":"user","content":CHECK},{"role":"assistant","content":null,"tool_calls":[call]},{"role":"tool","tool_call_id":call["id"],"content":"{\"status\":\"ok\"}"}]);
        let (text, extra) = completion(
            &stream(
                client,
                endpoint,
                json!({"model":model,"stream":true,"max_tokens":1024,"messages":messages}),
            )
            .await?,
        )?;
        if !extra.is_empty() {
            return Err("工具结果续接后仍请求工具调用".into());
        }
        text
    } else {
        return Err("不支持的免费模型协议".into());
    };
    if !text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word == "OK")
    {
        return Err("工具结果续接未返回 OK".into());
    }
    Ok(
        json!({"inference":true,"streaming":true,"toolCall":true,"toolResult":true,"anonymous":true,"probeSource":"runtime-protocol"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_call_keeps_identity_and_requires_real_completion() {
        let events = vec![
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"connectivity_check","arguments":"{\"status\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","function":{"name":"","arguments":"\"ok\"}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"done":true}),
        ];
        let (_, calls) = completion(&events).unwrap();
        assert_eq!(checked_call(&calls, false).unwrap()["id"], "call-1");
        assert!(completion(&events[..2]).is_err());
    }
}
