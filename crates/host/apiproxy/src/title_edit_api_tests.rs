use super::idle_retirement_tests::control_admission_fixture;
use super::*;
use serde_json::{Value, json};

fn title_config() -> dsh_session_title::Config {
    dsh_session_title::Config {
        fallback_max_words: 8,
        fallback_max_bytes: 96,
        max_title_bytes: 256,
    }
}

async fn rename(service: &Arc<ApiProxyService>, payload: Value) -> Value {
    let response = crate::fetch::handler::to_fetch_handler(service.clone())
        .handle(crate::fetch::handler::CarrierRequest {
            method: http::Method::POST,
            path: "/api/session.rename".into(),
            query: vec![],
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(
                serde_json::to_vec(&json!({"type":"client-request","rpcId":"title-edit",
                "method":"session.rename","payload":payload}))
                .unwrap(),
            ),
        })
        .await;
    let Body::Bytes(bytes) = response.into_body() else {
        panic!("unary result required")
    };
    serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
}

#[tokio::test]
async fn title_edits_reject_stale_windows_and_aba_without_rejecting_unrelated_events() {
    let (service, agent, detach) = control_admission_fixture("title-edit-owner").await;
    let titles =
        dsh_session_title::SessionTitleService::install(&service.ctx, title_config()).unwrap();
    let first = titles.rename(agent.session(), "Original").unwrap();
    let base = json!({"value":first.title,"throughSeq":first.event_seq});
    agent
        .session()
        .append("plugin:unrelated", json!({}), None)
        .unwrap();
    let accepted = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"Window A","expectedTitle":base}),
    )
    .await;
    assert_eq!(accepted["ok"], true, "{accepted}");
    let count = agent.session().seq();
    let rejected = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"Window B draft","expectedTitle":base}),
    )
    .await;
    assert_eq!(rejected["error"]["code"], "title-conflict", "{rejected}");
    assert_eq!(rejected["error"]["details"]["title"], "Window A");
    assert_eq!(
        agent.session().seq(),
        count,
        "conflict cannot append an event"
    );
    titles.rename(agent.session(), "Original").unwrap();
    let aba = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"Stale draft","expectedTitle":base}),
    )
    .await;
    assert_eq!(aba["error"]["code"], "title-conflict");
    let current = titles.get(agent.session()).unwrap();
    let adopted = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"Window B draft",
        "expectedTitle":{"value":current.title,"throughSeq":current.event_seq}}),
    )
    .await;
    assert_eq!(adopted["ok"], true);
    assert_eq!(titles.get(agent.session()).unwrap().title, "Window B draft");
    detach().await;
}

#[tokio::test]
async fn title_edits_accept_absent_baselines_and_reject_invalid_watermarks() {
    let (service, agent, detach) = control_admission_fixture("untitled-edit-owner").await;
    let titles =
        dsh_session_title::SessionTitleService::install(&service.ctx, title_config()).unwrap();
    let invalid = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"Title",
        "expectedTitle":{"value":null,"throughSeq":-2}}),
    )
    .await;
    assert_eq!(invalid["error"]["code"], "title-invalid");
    assert!(titles.get(agent.session()).is_none());
    let valid = rename(
        &service,
        json!({"sessionId":agent.id(),"title":"  Title  ",
        "expectedTitle":{"value":null,"throughSeq":-1}}),
    )
    .await;
    assert_eq!(valid["value"]["title"], "Title");
    detach().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_title_writers_from_the_same_baseline_have_one_winner() {
    let (service, agent, detach) = control_admission_fixture("concurrent-title-owner").await;
    let titles =
        dsh_session_title::SessionTitleService::install(&service.ctx, title_config()).unwrap();
    let first = titles.rename(agent.session(), "Original").unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for name in ["Window A", "Window B"] {
        let titles = titles.clone();
        let session = agent.session().clone();
        let barrier = barrier.clone();
        workers.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            titles.rename_checked(
                &session,
                name,
                Some((Some("Original"), first.event_seq as i64)),
            )
        }));
    }
    let mut accepted = 0;
    let mut conflicts = 0;
    for worker in workers {
        match worker.await.unwrap() {
            Ok(_) => accepted += 1,
            Err(dsh_session_title::RenameFailure::Conflict(_)) => conflicts += 1,
            other => panic!("unexpected title result: {other:?}"),
        }
    }
    assert_eq!((accepted, conflicts), (1, 1));
    detach().await;
}
