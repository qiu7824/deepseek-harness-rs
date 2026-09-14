impl BrowserSession {
    async fn upload_files(
        &self,
        selector: &str,
        files: &[String],
        signal: &AbortPredicate,
    ) -> Result<(), AdapterError> {
        let websocket = self.page_websocket.clone();
        let selector = selector.to_string();
        let files = files.to_vec();
        run_cancellable(
            async move {
                let (mut socket, _) =
                    tokio_tungstenite::connect_async(&websocket)
                        .await
                        .map_err(|e| {
                            AdapterError::new("COMPUTER_USE_DEVTOOLS_CONNECT", e.to_string())
                        })?;
                let root = socket_rpc(&mut socket, 1, "DOM.getDocument", json!({})).await?;
                let node = socket_rpc(
                    &mut socket,
                    2,
                    "DOM.querySelector",
                    json!({"nodeId":root["root"]["nodeId"],"selector":selector}),
                )
                .await?;
                if node["nodeId"].as_u64().unwrap_or(0) == 0 {
                    return Err(AdapterError::new(
                        "COMPUTER_USE_FILE_INPUT_NOT_FOUND",
                        "selector did not match a file input",
                    ));
                }
                socket_rpc(
                    &mut socket,
                    3,
                    "DOM.setFileInputFiles",
                    json!({"nodeId":node["nodeId"],"files":files}),
                )
                .await?;
                let _ = socket.close(None).await;
                Ok(())
            },
            signal.clone(),
            self.action_timeout,
        )
        .await
    }
    async fn tabs(&self, signal: &AbortPredicate) -> Result<Value, AdapterError> {
        let result = self.cdp("Target.getTargets", json!({}), signal).await?;
        Ok(json!(result["targetInfos"].as_array().into_iter().flatten()
            .filter(|t|t["type"]=="page").take(32)
            .map(|t|json!({"id":t["targetId"],"url":t["url"],"title":t["title"],"active":self.page_websocket.ends_with(&format!("/{}",t["targetId"].as_str().unwrap_or("")))})).collect::<Vec<_>>()))
    }
    fn page_address(&self, id: &str) -> String {
        let prefix = self
            .browser_websocket
            .split("/devtools/")
            .next()
            .unwrap_or("");
        format!("{prefix}/devtools/page/{id}")
    }
    async fn select_tab(&mut self, id: &str, signal: &AbortPredicate) -> Result<(), AdapterError> {
        if !self
            .tabs(signal)
            .await?
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id)
        {
            return Err(AdapterError::new(
                "COMPUTER_USE_TAB_NOT_FOUND",
                "tab does not belong to this browser session",
            ));
        }
        self.cdp("Target.activateTarget", json!({"targetId":id}), signal)
            .await?;
        self.page_websocket = self.page_address(id);
        Ok(())
    }
}

async fn socket_rpc(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, AdapterError> {
    socket
        .send(Message::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|e| AdapterError::new("COMPUTER_USE_DEVTOOLS_SEND", e.to_string()))?;
    while let Some(message) = socket.next().await {
        let message = message
            .map_err(|e| AdapterError::new("COMPUTER_USE_DEVTOOLS_RECEIVE", e.to_string()))?;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| AdapterError::new("COMPUTER_USE_DEVTOOLS_JSON", e.to_string()))?;
            if value["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(AdapterError::new(
                    "COMPUTER_USE_DEVTOOLS_PROTOCOL",
                    error.to_string(),
                ));
            }
            return Ok(value["result"].clone());
        }
    }
    Err(AdapterError::new(
        "COMPUTER_USE_DEVTOOLS_CLOSED",
        "browser connection closed during upload",
    ))
}

async fn upload_paths(request: &AdapterRequest) -> Result<Vec<String>, AdapterError> {
    let fail = |message: &str| AdapterError::new("COMPUTER_USE_UPLOAD_INVALID", message);
    let paths = request.arguments["files"]
        .as_array()
        .ok_or_else(|| fail("files must be an array"))?;
    if paths.len() > 16 {
        return Err(fail("at most 16 files may be uploaded"));
    }
    let root = request.workspace_root.as_ref().ok_or_else(|| {
        fail("upload requires a Host-owned workspace; attach the files to a workspace first")
    })?;
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| fail("workspace cannot be resolved"))?;
    let mut total = 0u64;
    let mut output = Vec::new();
    for item in paths {
        let path = Path::new(
            item.as_str()
                .ok_or_else(|| fail("each file must be a path"))?,
        );
        let resolved = tokio::fs::canonicalize(if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        })
        .await
        .map_err(|_| fail("upload file cannot be resolved"))?;
        if !resolved.starts_with(&root) {
            return Err(fail("upload files must stay inside the workspace"));
        }
        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|_| fail("upload file is unavailable"))?;
        total = total.saturating_add(meta.len());
        if !meta.is_file() || meta.len() > 16 * 1024 * 1024 || total > 64 * 1024 * 1024 {
            return Err(fail("upload exceeds the regular-file or size limits"));
        }
        // Chromium accepts canonical Windows paths, including extended path syntax.
        output.push(resolved.to_string_lossy().into_owned());
    }
    Ok(output)
}
