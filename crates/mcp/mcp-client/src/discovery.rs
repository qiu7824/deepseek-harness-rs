use super::*;
pub(crate) fn negotiated_version(initialized: &Value) -> Result<String, McpClientError> {
    let version = initialized["protocolVersion"]
        .as_str()
        .ok_or_else(|| error("MCP initialize requires protocolVersion"))?;
    if ![PROTOCOL_VERSION, "2025-06-18", "2025-03-26", "2024-11-05"].contains(&version) {
        return Err(error("MCP server selected an unsupported protocol version"));
    }
    if !initialized["capabilities"].is_object() {
        return Err(error("MCP initialize requires capabilities"));
    }
    Ok(version.into())
}
pub(crate) async fn catalog(
    client: &dyn McpTransport,
    initialized: &Value,
) -> Result<Value, McpClientError> {
    let mut listed = json!({"tools":[],"resourcesSupported":initialized["capabilities"].get("resources").is_some()});
    if initialized["capabilities"].get("tools").is_none() {
        return Ok(listed);
    }
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = client
            .request(
                "tools/list",
                cursor
                    .as_ref()
                    .map(|c| json!({"cursor":c}))
                    .unwrap_or(json!({})),
            )
            .await?;
        let rows = page["tools"]
            .as_array()
            .ok_or_else(|| error("MCP tools/list requires tools"))?;
        listed["tools"]
            .as_array_mut()
            .unwrap()
            .extend(rows.iter().cloned());
        if listed["tools"].as_array().unwrap().len() > 10000
            || serde_json::to_vec(&listed)
                .map_err(|_| error("invalid MCP tools JSON"))?
                .len()
                > 8 * 1024 * 1024
        {
            return Err(error("MCP tool catalog exceeds the size limit"));
        }
        match page.get("nextCursor") {
            None => break,
            Some(value) => {
                let next = value
                    .as_str()
                    .filter(|s| {
                        !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control)
                    })
                    .ok_or_else(|| error("invalid MCP tools cursor"))?;
                if !seen.insert(next.to_string()) || seen.len() > 100 {
                    return Err(error("MCP tools pagination did not terminate"));
                }
                cursor = Some(next.to_string());
            }
        }
    }
    validate_catalog(&listed)?;
    Ok(listed)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Pages(parking_lot::Mutex<Vec<Value>>);
    impl McpTransport for Pages {
        fn request(&self, _: &'static str, _: Value) -> RequestFuture<'_> {
            Box::pin(async { Ok(self.0.lock().remove(0)) })
        }
    }
    #[tokio::test]
    async fn discovery_handles_resources_only_pagination_and_cycles() {
        let init = json!({"protocolVersion":"2025-06-18","capabilities":{"resources":{}}});
        assert_eq!(negotiated_version(&init).unwrap(), "2025-06-18");
        assert_eq!(
            catalog(&Pages(Default::default()), &init).await.unwrap()["tools"],
            json!([])
        );
        let init = json!({"capabilities":{"tools":{}}});
        let row = json!({"name":"echo","inputSchema":{"type":"object"}});
        let client = Pages(parking_lot::Mutex::new(vec![
            json!({"tools":[],"nextCursor":"next"}),
            json!({"tools":[row]}),
        ]));
        assert_eq!(
            catalog(&client, &init).await.unwrap()["tools"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let client = Pages(parking_lot::Mutex::new(vec![
            json!({"tools":[],"nextCursor":"loop"}),
            json!({"tools":[],"nextCursor":"loop"}),
        ]));
        assert!(catalog(&client, &init).await.is_err());
        assert!(
            negotiated_version(&json!({"protocolVersion":"2099-01-01","capabilities":{}})).is_err()
        );
    }
}
