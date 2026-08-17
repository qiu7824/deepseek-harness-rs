//! Composition-layer `agentPreset.*` over the real fetch carrier: the
//! roster, blank-session switching, authoring, and the native-opener
//! handoff.

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxTarget,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use dsh_session::{Session, SessionId, SessionStore, UserMessage, session_id};

fn run<F: std::future::Future>(future: F) -> F::Output {
    // Multi-thread: the preset mount path drives loader row fibers whose
    // settles interleave across spawned tasks (the agent-presets mount
    // suite runs on the same runtime shape).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// The TS `contribute` fixture row: loads and registers nothing.
struct ContributePlugin;

#[async_trait::async_trait]
impl Plugin for ContributePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("contribute")
    }

    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

/// A mountable composition naming the fixture plugin registered above.
const VALID: &str = "- id: alpha\n  name: contribute\n  config:\n    tool: alpha\n";

fn temp_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "dsh-apiproxy-preset-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

async fn register_agent(registry: &AgentRegistry, agent: &Arc<dyn Agent>) {
    registry.register(&registry.ctx, agent.clone());
    let id = agent.id().clone();
    for _ in 0..10_000 {
        if registry.get(&id).is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("agent never became live");
}

struct Harness {
    ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    agents: Arc<AgentRegistry>,
    _system_root: std::path::PathBuf,
    _user_root: std::path::PathBuf,
}

impl Harness {
    /// Boot the runtime with a one-preset roster: `standard` under a system
    /// root (default) and an empty user root for authoring.
    async fn new(defaults: ApiProxyDefaults) -> Self {
        let ctx = Context::root();
        let loader_fiber = ctx.plugin(dsh_cordis_loader::plugin(), cordis::arc(()));
        loader_fiber.settle().await.expect("loader loads");
        let loader = ctx
            .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", true)
            .expect("loader service")
            .as_ref()
            .clone();
        loader.core.register("contribute", Arc::new(ContributePlugin));
        SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let system_root = temp_dir("system");
        let user_root = temp_dir("user");
        std::fs::create_dir_all(system_root.join("standard")).expect("system preset dir");
        std::fs::write(
            system_root
                .join("standard")
                .join(dsh_agent_presets::COMPOSITION_FILE),
            VALID,
        )
        .expect("composition");
        dsh_agent_presets::AgentPresets::install(
            &ctx,
            dsh_agent_presets::Config {
                default: "standard".to_string(),
                roots: vec![
                    dsh_agent_presets::PresetRoot {
                        path: system_root.to_string_lossy().into_owned(),
                        trust: dsh_agent_presets::PresetTrust::System,
                    },
                    dsh_agent_presets::PresetRoot {
                        path: user_root.to_string_lossy().into_owned(),
                        trust: dsh_agent_presets::PresetTrust::User,
                    },
                ],
                include_user_root: false,
            },
            Arc::new(|_| None),
        )
        .expect("presets install");
        let service = ApiProxyService::install(&ctx, defaults);
        let handler = to_fetch_handler(service);
        Self {
            ctx,
            handler,
            agents,
            _system_root: system_root,
            _user_root: user_root,
        }
    }

    async fn post(&self, method: &str, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": method,
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: format!("/api/{method}"),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        serde_json::from_slice(&bytes).expect("json")
    }

    /// A live agent whose context carries a scope key (what recompose
    /// joins to the standing composition through).
    fn blank_agent(&self, id: &str) -> Arc<dyn Agent> {
        let key = ScopeKey::new();
        let scope = create_scope(&self.ctx, key.clone(), &CreateScopeOptions::default());
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(StubAgent {
            id,
            session,
            inbox,
            ctx: scope.ctx.clone(),
            scope_key: key,
        })
    }
}

#[test]
fn list_answers_an_empty_roster_without_the_service() {
    run(async {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        AgentRegistry::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "agentPreset.list",
            "payload": {},
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/agentPreset.list".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let listed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(listed["result"]["ok"], true, "{listed}");
        assert_eq!(listed["result"]["value"]["presets"].as_array().unwrap().len(), 0);
        assert_eq!(listed["result"]["value"]["authorable"], false);
        assert_eq!(listed["result"]["value"]["hasDocument"], false);

        // The authoring calls refuse the same deployment with one shared code.
        for (method, payload) in [
            ("agentPreset.read", serde_json::json!({ "agentPreset": "x" })),
            ("agentPreset.copy", serde_json::json!({ "from": "standard", "agentPreset": "x" })),
            ("agentPreset.remove", serde_json::json!({ "agentPreset": "x" })),
            (
                "agentPreset.openDocument",
                serde_json::json!({ "agentPreset": "x" }),
            ),
        ] {
            let body = serde_json::to_string(&serde_json::json!({
                "type": "client-request",
                "rpcId": "r1",
                "method": method,
                "payload": payload,
            }))
            .expect("envelope");
            let response = handler
                .handle(CarrierRequest {
                    method: http::Method::POST,
                    path: format!("/api/{method}"),
                    query: vec![],
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: Some(body.into_bytes()),
                })
                .await;
            let Body::Bytes(bytes) = response.into_body() else {
                panic!("byte body");
            };
            let refused: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(refused["result"]["ok"], false, "{method}: {refused}");
            assert_eq!(
                refused["result"]["error"]["code"],
                "agent-preset-not-found",
                "{method}: {refused}"
            );
            assert_eq!(
                refused["result"]["error"]["message"],
                "this deployment composes no agent presets"
            );
        }
    });
}

#[test]
fn list_reports_the_roster_trust_default_and_authoring() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults {
            can_open_path: Some(Arc::new(|| true)),
            ..Default::default()
        })
        .await;
        let listed = harness.post("agentPreset.list", serde_json::json!({})).await;
        assert_eq!(listed["result"]["ok"], true, "{listed}");
        let presets = listed["result"]["value"]["presets"].as_array().expect("presets");
        assert_eq!(presets.len(), 1, "{presets:?}");
        assert_eq!(presets[0]["id"], "standard");
        assert_eq!(presets[0]["trust"], "system");
        assert_eq!(presets[0]["isDefault"], true);
        assert_eq!(listed["result"]["value"]["authorable"], true);
        assert_eq!(listed["result"]["value"]["hasDocument"], true);
    });
}

#[test]
fn select_switches_a_blank_session_and_logs_the_event() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults::default()).await;
        let agent = harness.blank_agent("live-1");
        register_agent(&harness.agents, &agent).await;
        let selected = harness
            .post(
                "agentPreset.select",
                serde_json::json!({ "sessionId": "live-1", "agentPreset": "standard" }),
            )
            .await;
        assert_eq!(selected["result"]["ok"], true, "{selected}");
        assert_eq!(selected["result"]["value"]["agentPreset"], "standard");
        // The log states what the agent runs.
        let logged = agent
            .session()
            .events()
            .iter()
            .any(|event| {
                event.type_ == dsh_agent_presets::AGENT_PRESET_SELECTED
                    && event.data["agentPreset"] == "standard"
            });
        assert!(logged, "selected event logged");
    });
}

#[test]
fn select_refuses_a_started_session_as_locked() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults::default()).await;
        let agent = harness.blank_agent("started-1");
        agent
            .session()
            .append("turn/start", serde_json::json!({ "turn": 1, "reason": "completed" }), None)
            .expect("started event");
        register_agent(&harness.agents, &agent).await;
        let selected = harness
            .post(
                "agentPreset.select",
                serde_json::json!({ "sessionId": "started-1", "agentPreset": "standard" }),
            )
            .await;
        assert_eq!(selected["result"]["ok"], false, "{selected}");
        assert_eq!(selected["result"]["error"]["code"], "agent-preset-locked");
        assert_eq!(selected["result"]["error"]["details"]["sessionId"], "started-1");
    });
}

#[test]
fn read_serves_the_composition_and_unknown_ids_as_not_found() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults::default()).await;
        let read = harness
            .post(
                "agentPreset.read",
                serde_json::json!({ "agentPreset": "standard" }),
            )
            .await;
        assert_eq!(read["result"]["ok"], true, "{read}");
        assert_eq!(read["result"]["value"]["agentPreset"], "standard");
        assert_eq!(read["result"]["value"]["trust"], "system");
        assert_eq!(read["result"]["value"]["content"], VALID);

        let refused = harness
            .post("agentPreset.read", serde_json::json!({ "agentPreset": "ghost" }))
            .await;
        assert_eq!(refused["result"]["ok"], false, "{refused}");
        assert_eq!(refused["result"]["error"]["code"], "agent-preset-not-found");
        assert_eq!(refused["result"]["error"]["details"]["agentPreset"], "ghost");
        assert_eq!(
            refused["result"]["error"]["details"]["available"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "standard"),
            true
        );
    });
}

#[test]
fn copy_creates_a_user_preset_and_refuses_an_occupied_id() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults::default()).await;
        let copied = harness
            .post(
                "agentPreset.copy",
                serde_json::json!({ "from": "standard", "agentPreset": "mine", "name": "Mine" }),
            )
            .await;
        assert_eq!(copied["result"]["ok"], true, "{copied}");
        assert_eq!(copied["result"]["value"]["agentPreset"], "mine");

        // The roster now carries the copy under the user root.
        let listed = harness.post("agentPreset.list", serde_json::json!({})).await;
        let ids: Vec<&str> = listed["result"]["value"]["presets"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["id"].as_str())
            .collect();
        assert!(ids.contains(&"standard"), "{ids:?}");
        assert!(ids.contains(&"mine"), "{ids:?}");
        let mine = listed["result"]["value"]["presets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == "mine")
            .expect("mine entry");
        assert_eq!(mine["trust"], "user");
        assert_eq!(mine["name"], "Mine");

        // A copy never overwrites.
        let refused = harness
            .post(
                "agentPreset.copy",
                serde_json::json!({ "from": "standard", "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(refused["result"]["ok"], false, "{refused}");
        assert_eq!(refused["result"]["error"]["code"], "agent-preset-invalid");
        assert_eq!(refused["result"]["error"]["details"]["agentPreset"], "mine");
    });
}

#[test]
fn remove_deletes_a_user_preset_and_refuses_a_shipped_one() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults::default()).await;
        let copied = harness
            .post(
                "agentPreset.copy",
                serde_json::json!({ "from": "standard", "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(copied["result"]["ok"], true, "{copied}");

        let removed = harness
            .post("agentPreset.remove", serde_json::json!({ "agentPreset": "mine" }))
            .await;
        assert_eq!(removed["result"]["ok"], true, "{removed}");

        // Shipped presets are not the user's to manage.
        let refused = harness
            .post(
                "agentPreset.remove",
                serde_json::json!({ "agentPreset": "standard" }),
            )
            .await;
        assert_eq!(refused["result"]["ok"], false, "{refused}");
        assert_eq!(refused["result"]["error"]["code"], "agent-preset-read-only");
        assert_eq!(refused["result"]["error"]["details"]["agentPreset"], "standard");
    });
}

#[test]
fn open_document_hands_a_user_directory_to_the_opener_and_refuses_shipped() {
    run(async {
        let opened: Arc<parking_lot::Mutex<Vec<String>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let opened_for_fn = opened.clone();
        let harness = Harness::new(ApiProxyDefaults {
            can_open_path: Some(Arc::new(|| true)),
            open_path: Some(Arc::new(move |path, _signal| {
                let opened = opened_for_fn.clone();
                Box::pin(async move {
                    opened.lock().push(path);
                    Ok(())
                })
            })),
            ..Default::default()
        })
        .await;
        let copied = harness
            .post(
                "agentPreset.copy",
                serde_json::json!({ "from": "standard", "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(copied["result"]["ok"], true, "{copied}");

        let opened_res = harness
            .post(
                "agentPreset.openDocument",
                serde_json::json!({ "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(opened_res["result"]["ok"], true, "{opened_res}");
        assert_eq!(opened_res["result"]["value"]["opened"], true);
        // The opener received the preset's DIRECTORY, Host-resolved.
        let calls = opened.lock().clone();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].ends_with("mine"), "directory not file: {}", calls[0]);
        assert!(!calls[0].ends_with(dsh_agent_presets::COMPOSITION_FILE));

        // Shipped presets are refused before any opener runs.
        let refused = harness
            .post(
                "agentPreset.openDocument",
                serde_json::json!({ "agentPreset": "standard" }),
            )
            .await;
        assert_eq!(refused["result"]["ok"], false, "{refused}");
        assert_eq!(refused["result"]["error"]["code"], "agent-preset-read-only");
    });
}

#[test]
fn open_document_reports_the_path_when_no_opener_can_run() {
    run(async {
        let harness = Harness::new(ApiProxyDefaults {
            can_open_path: Some(Arc::new(|| false)),
            ..Default::default()
        })
        .await;
        let copied = harness
            .post(
                "agentPreset.copy",
                serde_json::json!({ "from": "standard", "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(copied["result"]["ok"], true, "{copied}");
        let opened = harness
            .post(
                "agentPreset.openDocument",
                serde_json::json!({ "agentPreset": "mine" }),
            )
            .await;
        assert_eq!(opened["result"]["ok"], true, "{opened}");
        assert_eq!(opened["result"]["value"]["opened"], false);
        let path = opened["result"]["value"]["path"].as_str().expect("path");
        assert!(path.ends_with("mine"), "{path}");
    });
}
