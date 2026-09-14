//! Image generation and editing through existing, account-owned connections.
use super::{provider_auth::AccountAuth, task_models::TaskModels};
use base64::Engine;
use cordis::Context;
use dsh_attachment::{
    AttachmentStore, ImageAttachmentRef, ImageMediaType, SaveImageAttachment, StoredImageAttachment,
};
use dsh_fs::{FileSystem, ResolveOptions};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const MAX_BODY: usize = 64 * 1024 * 1024;
fn failure(message: impl Into<String>) -> ToolBodyError {
    let message = message.into();
    let code = if message.starts_with("NATIVE_TOOL_UNSUPPORTED:") {
        "NATIVE_TOOL_UNSUPPORTED"
    } else { "IMAGE_GENERATION_FAILED" };
    ToolBodyError::coded(
        message,
        "ImageGenerationError",
        code,
    )
}
fn image_ref(value: &Value, id: &str) -> Option<ImageAttachmentRef> {
    dsh_attachment::find_image_reference(value, id)
}
fn sniff(bytes: &[u8]) -> Result<ImageMediaType, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(ImageMediaType::Png)
    } else if bytes.starts_with(&[255, 216, 255]) {
        Ok(ImageMediaType::Jpeg)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Ok(ImageMediaType::Webp)
    } else {
        Err("图像数据不是PNG、JPEG或WebP".into())
    }
}
pub(crate) async fn read_references(
    ctx: &Context,
    execution: &ToolExecution,
    values: &Value,
) -> Result<Vec<StoredImageAttachment>, String> {
    let Some(values) = values.as_array() else {
        return Ok(vec![]);
    };
    if values.len() > 16 {
        return Err("最多使用16张参考图".into());
    }
    let agent = execution.agent.as_ref().ok_or("图像操作需要当前会话")?;
    let store = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .ok_or("附件存储不可用")?;
    let signal = execution.signal.lock().clone();
    let mut result = vec![];
    let mut total = 0;
    for value in values {
        if signal() {
            return Err("图像读取已取消".into());
        }
        let path = value.as_str().ok_or("参考图必须是附件标识或工作区路径")?;
        let reference = agent
            .session()
            .with_events(|events| events.iter().find_map(|event| image_ref(&event.data, path)));
        if path.starts_with("sha256:") && reference.is_none() {
            return Err("参考图不属于当前会话".into());
        }
        let image = if let Some(reference) = reference {
            store
                .read_image(&reference, Some(&signal))
                .await
                .map_err(|e| e.to_string())?
        } else {
            let fs = ctx
                .get_typed::<Arc<dyn FileSystem>>("fs", false)
                .ok_or("文件系统不可用")?;
            let cwd = agent
                .session()
                .header()
                .cwd
                .clone()
                .ok_or("参考图需要工作区")?;
            let options = ResolveOptions {
                cwd: Some(cwd.clone()),
                signal: Some(signal.clone()),
            };
            let root = fs
                .resolve(&cwd, Some(&options))
                .await
                .map_err(|e| e.to_string())?;
            let target = fs
                .resolve(path, Some(&options))
                .await
                .map_err(|e| e.to_string())?;
            if !fs.contains(&root, &target) {
                return Err("工作区之外的参考图请先上传到当前对话".into());
            }
            let bytes = fs
                .read_bytes(
                    &target,
                    Some(signal.clone()),
                    store.image_limits().max_image_bytes,
                )
                .await
                .map_err(|e| e.to_string())?;
            let reference = store
                .save_image(&SaveImageAttachment {
                    media_type: sniff(&bytes)?,
                    data: bytes.clone(),
                    name: std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned()),
                })
                .await
                .map_err(|e| e.to_string())?;
            StoredImageAttachment {
                reference,
                data: bytes,
            }
        };
        total += image.data.len();
        if total > 32 * 1024 * 1024 {
            return Err("参考图总大小超过32MiB，请减少图片或尺寸".into());
        }
        result.push(image);
    }
    Ok(result)
}

pub(crate) struct ImageGeneration {
    ctx: Context,
    tasks: Arc<TaskModels>,
    auth: Arc<AccountAuth>,
    gate: tokio::sync::Semaphore,
}
impl ImageGeneration {
    pub fn install(
        ctx: &Context,
        tasks: Arc<TaskModels>,
        auth: Arc<AccountAuth>,
    ) -> Result<Arc<Self>, String> {
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            tasks,
            auth,
            gate: tokio::sync::Semaphore::new(1),
        });
        let tool_service = service.clone();
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .ok_or("工具运行时不可用")?;
        tools.register(ctx,ToolDefinition{
            name:"generate_image".into(),description:"Generate an actual image or edit existing images using the current conversation connection and existing credentials, or an optional dedicated task model when configured. Task assignments are not required. Use this for image requests instead of drawing substitutes with code. For edits, pass reference_images as image attachment IDs from this conversation or workspace image paths; optionally pass a PNG mask. Results are displayed as images in this conversation with preview and download. Return image IDs may be reused for further edits. Use consult_model task=vision to review images if a vision route is configured.".into(),
            parameters:json!({"type":"object","additionalProperties":false,"properties":{"prompt":{"type":"string","minLength":1,"maxLength":32000},"reference_images":{"type":"array","maxItems":16,"items":{"type":"string"}},"mask":{"type":"string"},"size":{"type":"string"},"quality":{"type":"string","enum":["auto","low","medium","high","xhigh","max"]},"n":{"type":"integer","minimum":1,"maximum":4}},"required":["prompt"]}),
            output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:Some(Arc::new(|_,value|{let mut meta=value.clone();meta["kind"]=json!("image-generation");Ok(meta)}))},
            timeout_ms:Some(600000),is_concurrency_safe:Some(Arc::new(|_|false)),finalize_content:None,present_call:Some(Arc::new(|args|Some(dsh_tools::ToolCallView::Generic{title:if args["reference_images"].as_array().is_some_and(|v|!v.is_empty()){"编辑图片"}else{"生成图片"}.into(),kind:None,raw_input:None,content:None,locations:None}))),present_result:None,
            execute:Arc::new(move|args,run|{let service=tool_service.clone();let args=args.clone();let execution=run.execution.clone();Box::pin(async move{service.run(&args,&execution).await.map_err(failure)})})
        })?;
        Ok(service)
    }
    async fn run(&self, args: &Value, execution: &ToolExecution) -> Result<Value, String> {
        execution.agent.as_ref().ok_or("图像操作需要当前会话")?;
        let signal = execution.signal.lock().clone();
        if signal() {
            return Err("图片生成已取消".into());
        }
        let wait = self.gate.acquire();
        tokio::pin!(wait);
        let _permit = loop {
            tokio::select! {permit=&mut wait=>break permit.map_err(|_|"生图服务已关闭")?,_=tokio::time::sleep(Duration::from_millis(50))=>if signal(){return Err("图片生成已取消".into())}}
        };
        let route = self.tasks.route("image", execution).await?;
        let provider = route["provider"].as_str().ok_or("未选择生图连接")?;
        let model = route["model"].as_str().ok_or("未选择生图模型")?;
        let (profile, key) = self.auth.image_connection(provider).await?;
        if key.is_none() && profile["keyless"] != true {
            return Err("此模型连接尚未配置可用凭据，请在已有连接中检查账号或API密钥".into());
        }
        let base = profile["baseURL"]
            .as_str()
            .ok_or("模型连接未提供服务地址")?;
        let base_url = reqwest::Url::parse(base).map_err(|_| "模型服务地址无效")?;
        let local = matches!(
            base_url.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        );
        if (base_url.scheme() != "https" && !(base_url.scheme() == "http" && local))
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err("生图连接要求HTTPS服务地址，本机测试可用HTTP".into());
        }
        let codex = profile["authProvider"] == "openai-codex";
        if !codex
            && !matches!(
                profile["api"].as_str(),
                Some("openai-completions" | "openai-responses")
            )
        {
            return Err("此连接尚未支持OpenAI Images兼容接口".into());
        }
        let images = read_references(&self.ctx, execution, &args["reference_images"]).await?;
        let masks = if let Some(mask) = args["mask"].as_str() {
            if images.is_empty() {
                return Err("蒙版编辑需要参考图".into());
            }
            read_references(&self.ctx, execution, &json!([mask])).await?
        } else {
            vec![]
        };
        if let Some(mask) = masks.first() {
            if mask.reference.media_type != ImageMediaType::Png
                || mask.reference.width != images[0].reference.width
                || mask.reference.height != images[0].reference.height
            {
                return Err("蒙版必须为与第一张参考图尺寸一致的PNG".into());
            }
        }
        let client = dsh_http_proxy::builder()?
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(540))
            .build()
            .map_err(|_| "无法创建生图连接")?;
        let mut headers = super::validated_discovery_headers(
            provider,
            &serde_json::from_value(profile.get("headers").cloned().unwrap_or(json!({})))
                .map_err(|_| "连接请求头无效")?,
        )?;
        for name in ["authorization", "host", "content-type", "content-length"] {
            headers.remove(name);
        }
        if let Some(key) = &key {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {key}")
                    .parse()
                    .map_err(|_| "连接凭据格式无效")?,
            );
        }
        let prompt = args["prompt"].as_str().unwrap_or("");
        let size = args["size"].as_str().unwrap_or("auto");
        let quality = args["quality"].as_str().unwrap_or("auto");
        let count = args["n"]
            .as_u64()
            .or_else(|| {
                args["n"]
                    .as_f64()
                    .filter(|n| n.fract() == 0.0 && (1.0..=4.0).contains(n))
                    .map(|n| n as u64)
            })
            .unwrap_or(1);
        let mut driver = None;
        let request = if codex {
            let models = self
                .tasks
                .llm
                .list_models(provider)
                .await
                .map_err(|e| e.to_string())?;
            let caller = execution
                .agent
                .as_ref()
                .filter(|a| a.options().provider.as_deref() == Some(provider))
                .and_then(|a| a.options().model.clone());
            let selected = caller
                .or_else(|| {
                    models
                        .into_iter()
                        .find(|m| !m.id.starts_with("gpt-image"))
                        .map(|m| m.id)
                })
                .ok_or("此订阅没有可用于图像工具调用的对话模型")?;
            driver = Some(selected.clone());
            let mut input = vec![
                json!({"type":"input_text","text":format!("Generate {count} image(s). {prompt}")}),
            ];
            for image in &images {
                input.push(json!({"type":"input_image","image_url":format!("data:{};base64,{}",image.reference.media_type.as_str(),base64::engine::general_purpose::STANDARD.encode(&image.data))}));
            }
            let mut tool = json!({"type":"image_generation","model":model,"output_format":"png","quality":quality,"size":size});
            if let Some(mask) = masks.first() {
                tool["input_image_mask"] = json!({"image_url":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(&mask.data))});
            }
            let endpoint = if base.trim_end_matches('/').ends_with("/responses") {
                base.to_string()
            } else {
                format!("{}/responses", base.trim_end_matches('/'))
            };
            client.post(endpoint).headers(headers).json(&json!({"model":selected,"instructions":"Use image_generation to fulfill the image request. Do not draw a substitute with code. Return generated images.","input":[{"role":"user","content":input}],"tools":[tool],"tool_choice":{"type":"image_generation"},"stream":true,"store":false}))
        } else if images.is_empty() {
            let mut body = json!({"model":model,"prompt":prompt,"n":count,"size":size});
            if !model.starts_with("gpt-image") {
                body["response_format"] = json!("b64_json");
            }
            if quality != "auto" {
                body["quality"] = json!(quality);
            }
            client
                .post(image_endpoint(base, "generations"))
                .headers(headers)
                .json(&body)
        } else {
            let mut form = reqwest::multipart::Form::new()
                .text("model", model.to_string())
                .text("prompt", prompt.to_string())
                .text("n", count.to_string())
                .text("size", size.to_string());
            if quality != "auto" {
                form = form.text("quality", quality.to_string());
            }
            for (index, image) in images.iter().enumerate() {
                form = form.part(
                    "image[]",
                    reqwest::multipart::Part::bytes(image.data.clone())
                        .file_name(format!(
                            "reference-{index}.{}",
                            extension(image.reference.media_type)
                        ))
                        .mime_str(image.reference.media_type.as_str())
                        .map_err(|_| "参考图格式无效")?,
                );
            }
            if let Some(mask) = masks.first() {
                form = form.part(
                    "mask",
                    reqwest::multipart::Part::bytes(mask.data.clone())
                        .file_name("mask.png")
                        .mime_str("image/png")
                        .unwrap(),
                );
            }
            client
                .post(image_endpoint(base, "edits"))
                .headers(headers)
                .multipart(form)
        };
        let pending = async {
            let mut response = request
                .send()
                .await
                .map_err(|_| "图像服务连接失败；未自动重试，避免重复生成和扣费".to_string())?;
            let status = response.status();
            let mut bytes = Vec::new();
            let mut events = ImageStream::default();
            let mut total = 0;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "图像响应中断".to_string())?
            {
                total += chunk.len();
                if total > MAX_BODY {
                    return Err("图像响应超过64MiB，请减少数量或尺寸".into());
                }
                if codex && status.is_success() {
                    events.feed(&chunk)?;
                    if events.ended {
                        break;
                    }
                } else {
                    bytes.extend_from_slice(&chunk);
                }
            }
            if !status.is_success() {
                return Err(safe_error(status.as_u16(), &bytes, key.as_deref()));
            }
            if codex {
                events.finish()
            } else {
                let mut value: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "图像服务返回无效JSON")?;
                drop(bytes);
                let rows = value["data"]
                    .as_array_mut()
                    .ok_or("图像服务没有返回data列表")?;
                if rows.is_empty() || rows.len() > 4 {
                    return Err("图像服务未返回1至4张图片".into());
                }
                for row in rows {
                    if row["b64_json"].as_str().is_none() {
                        let url = row["url"].as_str().ok_or("图像服务缺少图片内容")?;
                        let bytes = download_image(url, &base_url).await?;
                        row["b64_json"] =
                            json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                        row.as_object_mut().unwrap().remove("url");
                    }
                }
                extract_image_values(value)
            }
        };
        tokio::pin!(pending);
        let encoded = loop {
            tokio::select! {value=&mut pending=>break value?,_=tokio::time::sleep(Duration::from_millis(50))=>if signal(){return Err("图片生成已取消".into())}}
        };
        let store = self
            .ctx
            .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
            .ok_or("附件存储不可用")?;
        let mut inputs = vec![];
        let mut total = 0;
        for (index, b64) in encoded.iter().enumerate() {
            if b64.len() > 24 * 1024 * 1024 {
                return Err("单张生成图超过存储限制".into());
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| "图像服务返回无效base64")?;
            total += data.len();
            if total > 48 * 1024 * 1024 {
                return Err("生成图片总大小超过48MiB".into());
            }
            let media_type = sniff(&data)?;
            inputs.push(SaveImageAttachment {
                data,
                media_type,
                name: Some(format!(
                    "generated-{}-{index}.{}",
                    uuid::Uuid::new_v4(),
                    extension(media_type)
                )),
            });
        }
        if signal() {
            return Err("图片生成已取消".into());
        }
        let saved = store
            .save_images(&inputs)
            .await
            .map_err(|e| e.to_string())?;
        let refs = saved
            .into_iter()
            .map(|attachment| json!({"type":"image","attachment":attachment}))
            .collect::<Vec<_>>();
        Ok(
            json!({"images":refs,"sourceImages":images.iter().chain(masks.iter()).map(|image|json!({"type":"image","attachment":image.reference})).collect::<Vec<_>>(),"provider":provider,"model":model,"driverModel":driver,"prompt":prompt,"size":size,"quality":quality,"edited":!images.is_empty()}),
        )
    }
}
fn extension(media: ImageMediaType) -> &'static str {
    match media {
        ImageMediaType::Png => "png",
        ImageMediaType::Jpeg => "jpg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Gif => "gif",
    }
}
fn image_endpoint(base: &str, operation: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base
        .strip_suffix("/chat/completions")
        .or_else(|| base.strip_suffix("/responses"))
        .unwrap_or(base);
    format!("{base}/images/{operation}")
}
fn safe_error(status: u16, bytes: &[u8], secret: Option<&str>) -> String {
    let mut message = serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "图像服务拒绝请求".into());
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        message = message.replace(secret, "[redacted]");
    }
    format!(
        "图像服务HTTP {status}: {}",
        message.chars().take(1000).collect::<String>()
    )
}
fn extract_image_values(mut value: Value) -> Result<Vec<String>, String> {
    let rows = value
        .as_object_mut()
        .and_then(|object| object.remove("data"))
        .ok_or("图像服务没有返回data列表")?;
    let Value::Array(rows) = rows else {
        return Err("图像data必须是列表".into());
    };
    if rows.is_empty() || rows.len() > 4 {
        return Err("图像服务未返回1至4张图片".into());
    }
    rows.into_iter()
        .map(|mut row| match row.get_mut("b64_json").map(Value::take) {
            Some(Value::String(image)) => Ok(image),
            _ => Err("图像服务缺少base64图片内容".into()),
        })
        .collect()
}
#[cfg(test)]
fn parse_images(bytes: &[u8], stream: bool) -> Result<Vec<String>, String> {
    if stream {
        let mut parser = ImageStream::default();
        parser.feed(bytes)?;
        parser.finish()
    } else {
        extract_image_values(serde_json::from_slice(bytes).map_err(|_| "图像服务返回无效JSON")?)
    }
}

#[derive(Default)]
struct ImageStream {
    buffer: Vec<u8>,
    data: Vec<u8>,
    images: Vec<String>,
    ids: std::collections::HashSet<String>,
    ended: bool,
}
impl ImageStream {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        self.buffer.extend_from_slice(bytes);
        let mut consumed = 0;
        while let Some(end) = self.buffer[consumed..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + consumed)
        {
            let mut line = &self.buffer[consumed..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1]
            }
            if line.is_empty() {
                let data = std::mem::take(&mut self.data);
                if !data.is_empty() {
                    self.event(&data)?;
                }
            } else if let Some(line) = line.strip_prefix(b"data:") {
                if !self.data.is_empty() {
                    self.data.push(b'\n')
                };
                self.data
                    .extend_from_slice(line.strip_prefix(b" ").unwrap_or(line));
            }
            consumed = end + 1;
            if self.ended {
                break;
            }
        }
        self.buffer.drain(..consumed);
        if self.buffer.len() + self.data.len() > MAX_BODY {
            return Err("单条生图事件过大".into());
        }
        Ok(())
    }
    fn event(&mut self, data: &[u8]) -> Result<(), String> {
        if data == b"[DONE]" {
            self.ended = true;
            return Ok(());
        }
        let value: Value = serde_json::from_slice(data).map_err(|_| "生图事件格式无效")?;
        if value["type"] == "error"
            || value["type"] == "response.failed"
            || value["type"] == "response.incomplete"
        {
            return Err("订阅图像请求失败或未完成，请检查所选模型权限；未自动切换连接".into());
        }
        let mut items = vec![];
        if let Some(item) = value.get("item") {
            items.push(item)
        }
        if let Some(output) = value.pointer("/response/output").and_then(Value::as_array) {
            items.extend(output);
        }
        for item in items {
            if item["type"] == "image_generation_call" && item["status"] == "completed" {
                if let Some(result) = item["result"].as_str() {
                    use sha2::Digest;
                    let id = item["id"].as_str().map(str::to_owned).unwrap_or_else(|| {
                        format!("{:x}", sha2::Sha256::digest(result.as_bytes()))
                    });
                    if self.ids.insert(id) {
                        if self.images.len() >= 4 {
                            return Err("图像服务返回超过4张图片".into());
                        }
                        self.images.push(result.to_string());
                    }
                }
            }
        }
        if matches!(
            value["type"].as_str(),
            Some("response.completed" | "response.done")
        ) {
            self.ended = true;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<Vec<String>, String> {
        if !self.ended && (!self.buffer.is_empty() || !self.data.is_empty()) {
            self.feed(b"\n\n")?;
        }
        if !self.ended || self.images.is_empty() {
            return Err("服务没有完整返回生成图片，未用文字或代码绘图替代".into());
        }
        Ok(self.images)
    }
}

fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let o = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && o[0] != 0
                && o[0] < 240
                && !(o[0] == 100 && (64..=127).contains(&o[1]))
        }
        std::net::IpAddr::V6(ip) => ip.to_ipv4().map(|ip| public_ip(ip.into())).unwrap_or(
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_stream_is_incremental_deduplicated_and_stops_at_completion() {
        let item = json!({"type":"image_generation_call","id":"ig-1","status":"completed","result":"aW1hZ2U="});
        let bytes = format!(
            "data: {}\r\n\r\ndata: {}\r\n\r\n",
            json!({"type":"response.output_item.done","item":item}),
            json!({"type":"response.completed","response":{"output":[item]}})
        );
        let mut parser = ImageStream::default();
        for byte in bytes.as_bytes() {
            parser.feed(&[*byte]).unwrap();
        }
        assert!(parser.ended);
        parser.feed(b"not-json-after-completion\n\n").unwrap();
        assert_eq!(parser.finish().unwrap(), ["aW1hZ2U="]);
    }
    #[test]
    fn incomplete_text_only_and_error_responses_are_not_successful_images() {
        assert!(parse_images(br#"{"data":[]}"#, false).is_err());
        assert!(parse_images(br#"{"text":"draw it with code"}"#, false).is_err());
        assert!(parse_images(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"id\":\"x\",\"status\":\"completed\",\"result\":\"aQ==\"}}\n\n",true).is_err());
        assert!(parse_images(b"data: {\"type\":\"response.failed\"}\n\n", true).is_err());
    }
    #[test]
    fn image_endpoints_reuse_connection_roots_and_full_chat_urls() {
        for base in [
            "https://example.com/v1",
            "https://example.com/v1/",
            "https://example.com/v1/chat/completions",
            "https://example.com/v1/responses",
        ] {
            assert_eq!(
                image_endpoint(base, "generations"),
                "https://example.com/v1/images/generations"
            );
            assert_eq!(
                image_endpoint(base, "edits"),
                "https://example.com/v1/images/edits"
            );
        }
    }
    #[test]
    fn credentials_are_redacted_and_cross_origin_private_downloads_are_disallowed() {
        let error = safe_error(
            401,
            br#"{"error":{"message":"credential sensitive-fixture was rejected"}}"#,
            Some("sensitive-fixture"),
        );
        assert!(!error.contains("sensitive-fixture"));
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
    }
}
async fn download_image(value: &str, base: &reqwest::Url) -> Result<Vec<u8>, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "图片下载地址无效")?;
    let same = url.origin() == base.origin();
    if (url.scheme() != "https" && !(same && url.scheme() == "http"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("图片下载地址不安全".into());
    }
    let host = url.host_str().ok_or("图片下载地址缺少主机")?;
    let port = url.port_or_known_default().ok_or("图片下载端口无效")?;
    let mut builder = dsh_http_proxy::builder()?
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60));
    if !same {
        let addresses = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| "图片下载地址解析超时")?
        .map_err(|_| "图片下载地址解析失败")?
        .collect::<Vec<_>>();
        if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err("不允许下载其他内网地址的图像".into());
        }
        builder = builder.resolve_to_addrs(host, &addresses);
    }
    let mut response = builder
        .build()
        .map_err(|_| "图片下载连接初始化失败")?
        .get(url)
        .send()
        .await
        .map_err(|_| "图片下载失败")?;
    if !response.status().is_success() {
        return Err(format!("图片下载HTTP {}", response.status().as_u16()));
    }
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await.map_err(|_| "图片下载中断")? {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err("下载图片超过16MiB".into());
        }
        bytes.extend_from_slice(&chunk)
    }
    sniff(&bytes)?;
    Ok(bytes)
}
