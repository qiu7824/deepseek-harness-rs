//! Rust port of the core `tool-todo.spec.ts` + `projection.spec.ts`
//! behaviors, driven through the real `ToolRuntime` pipeline: registration
//! shape, whole-list append semantics, trimming, single/parallel active
//! policy, input validation, non-agent rejection, presentation, disposal,
//! and the `todos` projection fold.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_agent::{Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget};
use dsh_llm::ToolSchema;
use dsh_llm::{ContentBlock, call_id};
use dsh_scope::ScopeKey;
use dsh_session::{
    AgentCancelCause, Session, SessionId, SessionStore, TodoItem, TodoStatus, session_id,
};
use dsh_system_prompt::SystemPrompt;
use dsh_tool_todo::{Config, NAME, ToolTodoPlugin, describe, to_todo_list};
use dsh_tools::{ToolExecutionInput, ToolRuntime};

struct ProbeAgent {
    id: SessionId,
    session: Session,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Arc::new(Self { id, session })
    }
}

impl Agent for ProbeAgent {
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
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
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

    fn send(&self, _message: dsh_session::UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

async fn setup(allow_parallel: bool) -> (Context, Arc<ToolRuntime>, cordis::Disposer) {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let tools = ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    // apply() registers immediately; the returned disposer is for REMOVAL
    // (run it in teardown, never before execution).
    let disposer = dsh_tool_todo::apply(
        &ctx,
        &Config {
            allow_parallel_in_progress: allow_parallel,
        },
    )
    .expect("apply");
    (ctx, tools, disposer)
}

fn input(agent: Option<Arc<ProbeAgent>>, args: serde_json::Value) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id("call-todo"),
        root_call_id: None,
        name: "todo_write".to_string(),
        arguments: args,
        agent: agent.map(|agent| agent as Arc<dyn Agent>),
        parent: None,
        signal: Arc::new(|| false),
    }
}

fn text(result: &dsh_tools::ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            _ => String::new(),
        })
        .collect()
}

fn last_todos(session: &Session) -> Vec<TodoItem> {
    let events = session.events();
    let event = events
        .iter()
        .rev()
        .find(|event| event.type_ == "todo/write")
        .expect("todo/write");
    serde_json::from_value(event.data.get("todos").cloned().expect("todos")).expect("todo items")
}

fn item(content: &str, status: &str) -> serde_json::Value {
    serde_json::json!({ "content": content, "status": status })
}

#[test]
fn description_varies_only_the_active_status_clause() {
    let single = describe(false);
    assert!(
        single.contains("Keep AT MOST ONE todo `in_progress`"),
        "{single}"
    );
    assert!(!single.contains("several at once"));
    let parallel = describe(true);
    assert!(
        parallel.contains("several at once when work genuinely runs in parallel"),
        "{parallel}"
    );
    assert!(!parallel.contains("AT MOST ONE"));
}

#[test]
fn value_constraints_normalize_and_fail_loud() {
    let trimmed = to_todo_list(
        &[serde_json::json!({"content": "  plan  ", "status": "pending"})],
        false,
    )
    .expect("trim");
    assert_eq!(
        trimmed,
        vec![TodoItem {
            content: "plan".to_string(),
            status: TodoStatus::Pending
        }]
    );
    assert!(
        to_todo_list(&[item("   ", "pending")], false)
            .unwrap_err()
            .contains("non-empty")
    );
    assert!(
        to_todo_list(&[item("dup", "pending"), item("dup", "completed")], false)
            .unwrap_err()
            .contains("duplicate")
    );
    assert!(
        to_todo_list(&[item("a", "in_progress"), item("b", "in_progress")], false)
            .unwrap_err()
            .contains("at most one task may be in_progress")
    );
    to_todo_list(&[item("a", "in_progress"), item("b", "in_progress")], true).expect("parallel");
}

#[tokio::test(flavor = "current_thread")]
async fn registers_the_todo_write_schema() {
    let (ctx, tools, _disposer) = setup(true).await;
    let schemas: Vec<ToolSchema> = tools.schemas(None);
    let schema = schemas
        .iter()
        .find(|schema| schema.name == "todo_write")
        .expect("todo_write schema");
    let properties = schema.parameters["properties"].as_object().expect("props");
    assert_eq!(properties.keys().collect::<Vec<_>>(), vec!["todos"]);
    let todos = &properties["todos"];
    assert_eq!(todos["type"], "array");
    let item_props = todos["items"]["properties"]
        .as_object()
        .expect("item props");
    let mut keys: Vec<&String> = item_props.keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["content", "status"]);
    assert_eq!(
        item_props["status"]["enum"],
        serde_json::json!(["pending", "in_progress", "completed"])
    );
    assert_eq!(NAME, "tool-todo");
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn appends_the_whole_list_and_reports_counts() {
    let (ctx, tools, _disposer) = setup(true).await;
    let agent = ProbeAgent::new("writer");
    let todos = serde_json::json!([
        { "content": "plan", "status": "in_progress" },
        { "content": "build", "status": "pending" }
    ]);
    let result = tools
        .execute(input(
            Some(agent.clone()),
            serde_json::json!({ "todos": todos }),
        ))
        .await;
    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(
        result.value.clone().expect("value"),
        serde_json::json!({
            "todos": [
                { "content": "plan", "status": "in_progress" },
                { "content": "build", "status": "pending" }
            ],
            "counts": { "pending": 1, "inProgress": 1, "completed": 0 }
        })
    );
    assert!(
        text(&result).contains("1 pending, 1 in progress, 0 completed"),
        "{}",
        text(&result)
    );
    assert_eq!(
        last_todos(agent.session()),
        vec![
            TodoItem {
                content: "plan".to_string(),
                status: TodoStatus::InProgress
            },
            TodoItem {
                content: "build".to_string(),
                status: TodoStatus::Pending
            },
        ]
    );
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn stores_trimmed_content_and_replaces_on_a_second_call() {
    let (ctx, tools, _disposer) = setup(true).await;
    let agent = ProbeAgent::new("trim");
    let result = tools
        .execute(input(
            Some(agent.clone()),
            serde_json::json!({ "todos": [{ "content": "  plan the work  ", "status": "pending" }] }),
        ))
        .await;
    assert!(!result.is_error);
    assert_eq!(
        last_todos(agent.session()),
        vec![TodoItem {
            content: "plan the work".to_string(),
            status: TodoStatus::Pending
        }]
    );

    let result = tools
        .execute(input(
            Some(agent.clone()),
            serde_json::json!({ "todos": [
                { "content": "plan the work", "status": "completed" },
                { "content": "b", "status": "in_progress" }
            ] }),
        ))
        .await;
    assert!(!result.is_error);
    assert_eq!(
        last_todos(agent.session()),
        vec![
            TodoItem {
                content: "plan the work".to_string(),
                status: TodoStatus::Completed
            },
            TodoItem {
                content: "b".to_string(),
                status: TodoStatus::InProgress
            },
        ]
    );
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_schema_level_and_value_level_violations() {
    let (ctx, tools, _disposer) = setup(true).await;

    // Malformed status.
    let result = tools
        .execute(input(
            None,
            serde_json::json!({ "todos": [{ "content": "x", "status": "doing" }] }),
        ))
        .await;
    assert!(result.is_error, "{}", text(&result));

    // Non-array todos.
    let result = tools
        .execute(input(None, serde_json::json!({ "todos": "nope" })))
        .await;
    assert!(result.is_error, "{}", text(&result));

    // Unknown item keys (additionalProperties: false).
    let result = tools
        .execute(input(
            None,
            serde_json::json!({ "todos": [{ "content": "a", "status": "pending", "children": [] }] }),
        ))
        .await;
    assert!(result.is_error);
    assert!(
        text(&result).contains("not a declared property"),
        "{}",
        text(&result)
    );

    // Empty and duplicate content.
    for (args, fragment) in [
        (
            serde_json::json!({ "todos": [{ "content": "   ", "status": "pending" }] }),
            "non-empty",
        ),
        (
            serde_json::json!({ "todos": [
                { "content": "dup", "status": "pending" },
                { "content": "dup", "status": "completed" }
            ] }),
            "duplicate",
        ),
    ] {
        let result = tools.execute(input(None, args)).await;
        assert!(result.is_error);
        assert!(text(&result).contains(fragment), "{}", text(&result));
    }
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn single_active_policy_rejects_parallel_lists_without_touching_the_log() {
    let (ctx, tools, _disposer) = setup(false).await;
    let agent = ProbeAgent::new("single-active");
    let result = tools
        .execute(input(
            Some(agent.clone()),
            serde_json::json!({ "todos": [
                { "content": "run subagent a", "status": "in_progress" },
                { "content": "run subagent b", "status": "in_progress" }
            ] }),
        ))
        .await;
    assert!(result.is_error);
    assert!(
        text(&result).contains("at most one task may be in_progress"),
        "{}",
        text(&result)
    );
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "todo/write"),
        "a rejected call must not reach the durable log"
    );

    // One active item is accepted.
    let result = tools
        .execute(input(
            Some(agent.clone()),
            serde_json::json!({ "todos": [
                { "content": "run subagent a", "status": "in_progress" },
                { "content": "run subagent b", "status": "pending" }
            ] }),
        ))
        .await;
    assert!(!result.is_error, "{}", text(&result));
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_non_agent_caller() {
    let (ctx, tools, _disposer) = setup(true).await;
    let result = tools
        .execute(input(
            None,
            serde_json::json!({ "todos": [{ "content": "a", "status": "pending" }] }),
        ))
        .await;
    assert!(result.is_error);
    assert!(
        text(&result).contains("owning agent session"),
        "{}",
        text(&result)
    );
    let _ = ctx;
}

#[test]
fn plugin_metadata_matches_the_ts_exports() {
    assert_eq!(NAME, "tool-todo");
    let _plugin = ToolTodoPlugin;
}

#[tokio::test(flavor = "current_thread")]
async fn presents_via_the_registered_definition_and_disposes_on_unload() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let tools = ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    let fiber = ctx.plugin(
        Arc::new(ToolTodoPlugin),
        arc(Config {
            allow_parallel_in_progress: true,
        }),
    );
    fiber.settle().await.expect("settle");

    let definition = tools.get("todo_write", None).expect("registered");
    let view = (definition.present_call.as_ref().expect("presentCall"))(
        &serde_json::json!({ "todos": [{ "content": "a", "status": "pending" }] }),
    )
    .expect("view");
    match view {
        dsh_tools::ToolCallView::Generic {
            title, raw_input, ..
        } => {
            assert_eq!(title, "Update todo list");
            assert_eq!(
                raw_input,
                Some(serde_json::json!([{ "content": "a", "status": "pending" }]))
            );
        }
        other => panic!("generic view expected, got {other:?}"),
    }
    assert!(
        tools
            .schemas(None)
            .iter()
            .any(|schema| schema.name == "todo_write")
    );

    fiber.dispose().await;
    assert!(
        !tools
            .schemas(None)
            .iter()
            .any(|schema| schema.name == "todo_write")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn todos_projection_folds_whole_lists_and_resets_at_turn_start() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    let registry = dsh_session_projection::SessionProjectionRegistry::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(ToolTodoPlugin),
        arc(Config {
            allow_parallel_in_progress: true,
        }),
    );
    fiber.settle().await.expect("settle");

    let session = store
        .create(
            &ctx,
            Some(session_id("projection-session")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    let read = |session: &Session| -> Option<Vec<TodoItem>> {
        let snapshot = registry.snapshot(session);
        snapshot
            .values
            .get("todos")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    };
    assert_eq!(read(&session), None);

    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    session
        .append(
            "todo/write",
            dsh_session::todo_write_data(&[TodoItem {
                content: "plan".to_string(),
                status: TodoStatus::InProgress,
            }]),
            None,
        )
        .expect("todo/write");
    assert_eq!(
        read(&session),
        Some(vec![TodoItem {
            content: "plan".to_string(),
            status: TodoStatus::InProgress
        }])
    );

    // A later turn/start clears the standing list.
    session
        .append("turn/start", serde_json::json!({ "turn": 2 }), None)
        .expect("turn/start");
    assert_eq!(read(&session), None);

    fiber.dispose().await;
}

#[tokio::test(flavor = "current_thread")]
async fn invariant_companion_installs_and_rejects_malformed_snapshots() {
    // Pure checker: the append veto of the TS internal/dispatch path is
    // contained in this port, so the companion's failure is observable
    // through the checker instead.
    assert!(dsh_todo_todo_invariant_validate(&serde_json::json!("nope")).is_err());
    assert!(
        dsh_todo_todo_invariant_validate(&serde_json::json!([
            { "content": "   ", "status": "pending" }
        ]))
        .is_err()
    );
    assert!(
        dsh_todo_todo_invariant_validate(&serde_json::json!([
            { "content": "a", "status": "pending" },
            { "content": "a", "status": "completed" }
        ]))
        .is_err()
    );
    assert!(
        dsh_todo_todo_invariant_validate(&serde_json::json!([
            { "content": "a", "status": "doing" }
        ]))
        .is_err()
    );
    assert!(
        dsh_todo_todo_invariant_validate(&serde_json::json!([
            { "content": "a", "status": "pending" }
        ]))
        .is_ok()
    );

    // Companion registration with the registry.
    let ctx = Context::root();
    let _store = SessionStore::install(&ctx);
    let _registry = dsh_invariants::InvariantRegistry::new(
        &ctx,
        dsh_invariants::InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(
        Arc::new(dsh_tool_todo::invariant::ToolTodoInvariantPlugin),
        arc(()),
    );
    fiber.settle().await.expect("settle");
    let session = _store
        .create(
            &ctx,
            Some(session_id("todo-invariant")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append(
            "todo/write",
            dsh_session::todo_write_data(&[TodoItem {
                content: "ok".to_string(),
                status: TodoStatus::Pending,
            }]),
            None,
        )
        .expect("valid snapshot commits");
    fiber.dispose().await;
}

/// Pure-checker shim mirroring the companion's event validator.
fn dsh_todo_todo_invariant_validate(value: &serde_json::Value) -> Result<(), String> {
    dsh_tool_todo::invariant::validate_todos(value)
}
