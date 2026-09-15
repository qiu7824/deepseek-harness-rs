//! Caller-scoped resource routing. A URI is protocol data for its configured
//! server, never a host filesystem path or an HTTP destination.
use super::*;
use dsh_scope::{ScopeKey, scope_chain_of, scope_of};

const MAX_RESOURCE_BYTES: usize = 2 * 1024 * 1024;
struct Route {
    server: String,
    scope: Option<ScopeKey>,
    client: Arc<dyn McpTransport>,
}
pub(crate) struct ResourceHub {
    routes: parking_lot::Mutex<Vec<(u64, Route)>>,
    next: AtomicU64,
}
impl cordis::Service for ResourceHub {
    fn service_name(&self) -> &'static str {
        "mcpResources"
    }
}
impl ResourceHub {
    fn visible(&self, scope: Option<&ScopeKey>) -> Vec<(String, Arc<dyn McpTransport>)> {
        let chain = scope_chain_of(scope);
        let routes = self.routes.lock();
        let mut selected =
            std::collections::BTreeMap::<String, (usize, Arc<dyn McpTransport>)>::new();
        for (_, route) in routes.iter() {
            let rank = match &route.scope {
                None => chain.len(),
                Some(key) => match chain.iter().position(|s| s == key) {
                    Some(index) => index,
                    None => continue,
                },
            };
            if selected
                .get(&route.server)
                .is_none_or(|(old, _)| rank < *old)
            {
                selected.insert(route.server.clone(), (rank, route.client.clone()));
            }
        }
        selected
            .into_iter()
            .map(|(name, (_, client))| (name, client))
            .collect()
    }
    fn ensure(ctx: &Context) -> Result<Arc<Self>, McpClientError> {
        static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _gate = GATE
            .lock()
            .map_err(|_| error("MCP resource initialization failed"))?;
        let root = ctx.root_context();
        if let Some(hub) = root.get_typed::<Arc<Self>>("mcpResources", false) {
            return Ok(hub.as_ref().clone());
        }
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .ok_or_else(|| error("MCP resources require tools"))?;
        let hub = Arc::new(Self {
            routes: Default::default(),
            next: AtomicU64::new(1),
        });
        let mut prepared = Vec::new();
        for (name, method, key) in [
            ("list_mcp_resources", "resources/list", "resources"),
            (
                "list_mcp_resource_templates",
                "resources/templates/list",
                "resourceTemplates",
            ),
            ("read_mcp_resource", "resources/read", "contents"),
        ] {
            let read = method == "resources/read";
            let caller = Arc::downgrade(&hub);
            let definition = ToolDefinition {
                name: name.into(),
                description: if read {
                    "Read a URI from a configured MCP resource server. URI values are sent only to that server.".into()
                } else {
                    "List resources or URI templates from a configured MCP server. Omit server to discover server names visible in this scope. Pass nextCursor back as cursor to continue.".into()
                },
                parameters: json!({"type":"object","properties":{"server":{"type":"string"},if read {"uri"} else {"cursor"}:{"type":"string"}},"required":if read {vec!["server","uri"]} else {vec![]}}),
                output: ToolOutputDefinition {
                    schema: json!({"type":"object"}),
                    render: Arc::new(|_, value| {
                        Ok(vec![ContentBlock::Text {
                            text: serde_json::to_string(value).unwrap_or_default(),
                        }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: Some(Arc::new(|_| true)),
                execute: Arc::new(move |args, exec| {
                    let hub = caller.clone();
                    let args = args.clone();
                    let scope = exec.agent.as_ref().map(|agent| agent.scope_key().clone());
                    let signal = exec.signal.lock().clone();
                    Box::pin(async move {
                        let hub = hub
                            .upgrade()
                            .ok_or_else(|| ToolBodyError::plain("MCP resources are unavailable"))?;
                        let visible = hub.visible(scope.as_ref());
                        let Some(server) = args["server"].as_str() else {
                            return if read {
                                Err(ToolBodyError::plain("server is required"))
                            } else {
                                Ok(
                                    json!({"servers":visible.iter().map(|(name,_)|name).collect::<Vec<_>>()}),
                                )
                            };
                        };
                        let client = visible
                            .into_iter()
                            .find(|(name, _)| name == server)
                            .map(|(_, client)| client)
                            .ok_or_else(|| {
                                ToolBodyError::plain(
                                    "MCP resource server is unavailable in this agent scope",
                                )
                            })?;
                        let mut params = json!({});
                        let arg = if read { "uri" } else { "cursor" };
                        if let Some(value) = args.get(arg) {
                            let value = value
                                .as_str()
                                .filter(|s| {
                                    !s.is_empty()
                                        && s.len() <= 8192
                                        && !s.chars().any(char::is_control)
                                })
                                .ok_or_else(|| {
                                    ToolBodyError::plain("invalid resource URI or cursor")
                                })?;
                            params[arg] = json!(value);
                        } else if read {
                            return Err(ToolBodyError::plain("uri is required"));
                        }
                        let result = tokio::select! {
                            result=client.request(method,params)=>result.map_err(|e|ToolBodyError::plain(e.to_string()))?,
                            _=async {while !signal() {tokio::time::sleep(Duration::from_millis(15)).await;}}=>return Err(ToolBodyError::plain("MCP resource request cancelled")),
                        };
                        validate_resource_result(&result, key)
                            .map_err(|e| ToolBodyError::plain(e.to_string()))?;
                        Ok(json!({"server":server,"result":result}))
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            };
            prepared.push(
                tools
                    .prepare_register_arc(&root, Arc::new(definition))
                    .map_err(error)?,
            );
        }
        for registration in prepared {
            let _ = registration.commit(&root);
        }
        root.register_service(hub.clone());
        Ok(hub)
    }
}
pub(crate) fn register(
    ctx: &Context,
    server: &str,
    client: Arc<dyn McpTransport>,
) -> Result<Disposer, McpClientError> {
    let hub = ResourceHub::ensure(ctx)?;
    let scope = scope_of(ctx);
    let id = hub.next.fetch_add(1, Ordering::Relaxed);
    {
        let mut routes = hub.routes.lock();
        if routes
            .iter()
            .any(|(_, route)| route.server == server && route.scope == scope)
        {
            return Err(error("duplicate MCP resource server in this scope"));
        }
        routes.push((
            id,
            Route {
                server: server.into(),
                scope,
                client,
            },
        ));
    }
    Ok(cordis::make_disposer(move || {
        hub.routes.lock().retain(|(key, _)| *key != id);
        Box::pin(async {})
    }))
}
fn validate_resource_result(value: &Value, key: &str) -> Result<(), McpClientError> {
    let rows = value[key]
        .as_array()
        .ok_or_else(|| error(format!("MCP resource response requires {key}")))?;
    if rows.len() > 10000
        || serde_json::to_vec(value)
            .map_err(|_| error("invalid MCP resource JSON"))?
            .len()
            > MAX_RESOURCE_BYTES
    {
        return Err(error("MCP resource response exceeds the size limit"));
    }
    if let Some(cursor) = value.get("nextCursor") {
        if !cursor
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control))
        {
            return Err(error("invalid MCP resource cursor"));
        }
    }
    for row in rows {
        let field = if key == "resourceTemplates" {
            "uriTemplate"
        } else {
            "uri"
        };
        if !row[field]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control))
        {
            return Err(error("invalid MCP resource identity"));
        }
        if key == "contents" && !(row["text"].is_string() ^ row["blob"].is_string()) {
            return Err(error(
                "MCP resource content requires exactly one of text or blob",
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake;
    impl McpTransport for Fake {
        fn request(&self, _: &'static str, _: Value) -> RequestFuture<'_> {
            Box::pin(async { Ok(json!({})) })
        }
    }
    #[test]
    fn resource_validation_preserves_text_blob_and_templates() {
        for (key, value) in [
            (
                "resources",
                json!({"resources":[{"uri":"file:///remote/readme"}]}),
            ),
            (
                "resourceTemplates",
                json!({"resourceTemplates":[{"uriTemplate":"repo://{branch}/{path}"}]}),
            ),
            (
                "contents",
                json!({"contents":[{"uri":"repo://main/a","text":"content"},{"uri":"repo://main/b","blob":"AQID"}]}),
            ),
        ] {
            validate_resource_result(&value, key).unwrap();
        }
        assert!(
            validate_resource_result(
                &json!({"contents":[{"uri":"repo:a","text":"a","blob":"AQID"}]}),
                "contents"
            )
            .is_err()
        );
        assert!(
            validate_resource_result(
                &json!({"contents":[{"uri":"repo:a","text":"x".repeat(MAX_RESOURCE_BYTES)}]}),
                "contents"
            )
            .is_err()
        );
    }
    #[test]
    fn resource_server_scopes_cannot_cross_agents() {
        let a = ScopeKey::new();
        let b = ScopeKey::new();
        let child = ScopeKey::new();
        let _parent = dsh_scope::bind_scope_parent(&child, &a);
        let hub = ResourceHub {
            routes: parking_lot::Mutex::new(vec![(
                1,
                Route {
                    server: "private".into(),
                    scope: Some(a.clone()),
                    client: Arc::new(Fake),
                },
            )]),
            next: AtomicU64::new(2),
        };
        assert_eq!(hub.visible(Some(&a)).len(), 1);
        assert_eq!(hub.visible(Some(&child)).len(), 1);
        assert!(hub.visible(Some(&b)).is_empty());
        assert!(hub.visible(None).is_empty());
    }
}
