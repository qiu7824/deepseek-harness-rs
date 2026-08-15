//! The DeepSeek Harness Host boot spine (M6): compose the ported service
//! stack — sessions, agents, system prompt, tools, JSONL persistence,
//! SQLite FTS5 session search, schedule, commands, and user questions —
//! run the package-owned invariant companions, and expose a boot report
//! with a real end-to-end durability + search probe.
//!
//! This is the composition half of the TS `apps/host` boot; the webserver,
//! apiproxy, and CLI front end build on it in later milestones. It is also
//! the seam the `dsh-host` binary and the integration tests share.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_agent::AgentRegistry;
use dsh_commands::CommandRuntime;
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_session::{SessionStore, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use dsh_session_query::{SessionQueryEngine, SessionSearchRequest};
use dsh_session_query_sqlite::{Config as SqliteSearchConfig, SqliteSearch};
use dsh_system_prompt::SystemPrompt;
use dsh_tools::ToolRuntime;
use dsh_user_questions::UserQuestionService;

/// One booted host spine: the root context plus its registered services and
/// the disposable data directories owned by this boot.
pub struct HostSpine {
    pub ctx: Context,
    pub sessions: Arc<SessionStore>,
    pub agents: Arc<AgentRegistry>,
    pub tools: Arc<ToolRuntime>,
    pub system_prompt: Arc<SystemPrompt>,
    pub commands: Arc<CommandRuntime>,
    pub questions: Arc<UserQuestionService>,
    pub persistence: Arc<JsonlSessionPersistence>,
    pub search: Arc<SqliteSearch>,
    pub query: Arc<SessionQueryEngine>,
    data_root: std::path::PathBuf,
}

impl Drop for HostSpine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_root);
    }
}

/// Compose the M6 host spine synchronously (the async service bindings
/// settle through their own fibers).
pub fn compose_host(ctx: &Context) -> Result<HostSpine, String> {
    // Package-owned invariant companions run first so every later append is
    // validated.
    let _invariants = InvariantRegistry::new(
        ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let data_root = std::env::temp_dir().join(format!("dsh-host-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data_root).map_err(|error| format!("data root: {error}"))?;
    let sessions_root = data_root.join("sessions");
    let search_path = data_root.join("search.db");

    let sessions = SessionStore::install(ctx);
    let agents = AgentRegistry::install(ctx);
    let system_prompt = SystemPrompt::install(ctx, dsh_system_prompt::Config::default())
        .map_err(|error| format!("systemPrompt: {error}"))?;
    let tools = ToolRuntime::install(
        ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .map_err(|error| format!("tools: {error}"))?;
    let commands = CommandRuntime::install(ctx);
    let questions = UserQuestionService::install(ctx);
    let persistence = JsonlSessionPersistence::install(
        ctx,
        JsonlConfig {
            root: sessions_root.to_string_lossy().to_string(),
            pack_chunks: true,
            compression: JsonlCompression::Zstd,
            prepared_session_cache_size: 5,
            write_batch_max_delay_ms: 200,
        },
    )
    .map_err(|error| format!("sessionPersistence: {error}"))?;
    let search = SqliteSearch::install(
        ctx,
        &SqliteSearchConfig {
            path: search_path.to_string_lossy().to_string(),
            ..Default::default()
        },
    )
    .map_err(|error| format!("sessionQuery: {}", error.message))?;
    let query = ctx
        .get_typed::<Arc<SessionQueryEngine>>("sessionQuery", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "sessionQuery service missing".to_string())?;
    dsh_schedule::apply(ctx);
    Ok(HostSpine {
        ctx: ctx.clone(),
        sessions,
        agents,
        tools,
        system_prompt,
        commands,
        questions,
        persistence,
        search,
        query,
        data_root,
    })
}

/// The service inventory plus a real durability-and-search probe — the
/// observable boot report shared by the binary and the integration test.
pub async fn boot_report(spine: &HostSpine) -> Result<serde_json::Value, String> {
    // Live path: a store-attached session, a user message, and a durability
    // flush through the JSONL coordinator.
    let session = spine
        .sessions
        .create(
            &spine.ctx,
            Some(session_id("host-boot")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .map_err(|error| format!("session create: {error}"))?;
    let starter = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "host boot live needle".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(&starter).map_err(|error| error.to_string())?,
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .map_err(|error| format!("append: {error}"))?;
    let flushed = spine
        .sessions
        .flush(&session)
        .await
        .map_err(|error| format!("flush: {error}"))?;

    // Persisted-only path: an independent durable log the search index must
    // reconcile through the erased persistence service.
    let durable_header = dsh_session::SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id("host-persisted"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let durable_event = dsh_session::SessionEvent {
        type_: "user/message".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::to_value(&dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "host persisted needle".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ))
        .expect("message"),
        ignorable: None,
        surface_op: Some(dsh_session::SurfaceOp::Append),
        source_event_seqs: None,
    };
    spine
        .persistence
        .create(durable_header.clone())
        .await
        .map_err(|error| format!("persisted create: {error}"))?;
    spine
        .persistence
        .append(&durable_header.id, &[durable_event])
        .await
        .map_err(|error| format!("persisted append: {error}"))?;
    let snapshots = spine
        .persistence
        .list_snapshots()
        .await
        .map_err(|error| format!("snapshots: {error}"))?;

    // The FTS5 index must find both the live and the persisted log.
    let live_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "live needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("live search: {}", error.message))?;
    let persisted_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("persisted search: {}", error.message))?;

    Ok(serde_json::json!({
        "services": [
            "invariants",
            "sessions",
            "agents",
            "systemPrompt",
            "tools",
            "commands",
            "userQuestions",
            "sessionPersistence",
            "sessionQuery",
            "schedule",
        ],
        "session": {
            "id": session.id().as_str(),
            "seq": session.seq(),
            "toolCount": spine.tools.schemas(None).len(),
        },
        "probe": {
            "flushAcknowledged": flushed,
            "persistedSnapshotCount": snapshots.len(),
            "liveSearchHits": live_hits.items.len(),
            "persistedSearchHits": persisted_hits.items.len(),
        },
    }))
}

/// Mount the package-owned invariant companions onto a composed spine.
pub fn mount_companions(spine: &HostSpine) {
    let _ = futures::executor::block_on(dsh_session::invariant::apply(&spine.ctx));
    let _schedule = dsh_schedule::invariant::apply(&spine.ctx);
    let _query_sqlite = dsh_session_query_sqlite::invariant::apply(&spine.ctx);
    let _ = spine
        .ctx
        .plugin(Arc::new(dsh_llm::LlmInvariantPlugin), arc(()));
}

// Re-exported anchors for compositions.
pub use dsh_agent::AgentRegistry as AgentRegistryType;
pub use dsh_session::SessionStore as SessionStoreType;
