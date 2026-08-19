//! Rust port of the core `user-questions.spec.ts` + `tool-ask-user.spec.ts`
//! behaviors: provider registration/disposal, the ask() validation ladder,
//! agent liveness/root checks, and the registered tool's request/answer
//! projection.

use std::sync::Arc;

use cordis::Context;
use dsh_user_questions::{
    AskUserQuestionAnswer, AskUserQuestionAnswerItem, AskUserQuestionIntent, AskUserQuestionItem,
    AskUserQuestionOption, AskUserQuestionRequest, UserQuestionError, UserQuestionService,
};

struct EchoProvider;

#[async_trait::async_trait]
impl dsh_user_questions::UserQuestionProvider for EchoProvider {
    async fn ask(
        &self,
        request: &AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, UserQuestionError> {
        Ok(AskUserQuestionAnswer {
            answers: request
                .questions
                .iter()
                .map(|question| AskUserQuestionAnswerItem {
                    id: question.id.clone(),
                    selected: question
                        .options
                        .as_deref()
                        .and_then(|options| options.first())
                        .map(|option| vec![option.label.clone()])
                        .unwrap_or_default(),
                    custom: None,
                })
                .collect(),
        })
    }
}

fn request(questions: Vec<AskUserQuestionItem>) -> AskUserQuestionRequest {
    AskUserQuestionRequest {
        questions,
        agent: None,
        signal: None,
    }
}

fn plain_question(id: &str) -> AskUserQuestionItem {
    AskUserQuestionItem {
        id: id.to_string(),
        question: "proceed?".to_string(),
        detail: None,
        header: None,
        options: Some(vec![AskUserQuestionOption {
            label: "yes".to_string(),
            description: None,
        }]),
        multi_select: None,
        intent: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn registers_one_provider_and_rejects_duplicates() {
    let ctx = Context::root();
    let service = UserQuestionService::install(&ctx);
    let outcome = service.ask(&request(vec![])).await;
    assert!(matches!(
        outcome,
        Err(UserQuestionError { code, .. }) if code == "EMPTY_QUESTIONS"
    ));
    let missing = service.ask(&request(vec![plain_question("q1")])).await;
    assert!(matches!(
        missing,
        Err(UserQuestionError { code, .. }) if code == "NO_PROVIDER"
    ));

    let disposer = service
        .register_provider(Arc::new(EchoProvider))
        .expect("register");
    let duplicate = service.register_provider(Arc::new(EchoProvider));
    assert!(matches!(
        duplicate,
        Err(UserQuestionError { code, .. }) if code == "DUPLICATE_PROVIDER"
    ));

    let answer = service
        .ask(&request(vec![plain_question("q1")]))
        .await
        .expect("ask");
    assert_eq!(answer.answers.len(), 1);
    assert_eq!(answer.answers[0].id, "q1");
    assert_eq!(answer.answers[0].selected, vec!["yes"]);

    (disposer)().await;
    let missing = service.ask(&request(vec![plain_question("q1")])).await;
    assert!(matches!(
        missing,
        Err(UserQuestionError { code, .. }) if code == "NO_PROVIDER"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn intent_validation_requires_the_approve_label_and_detail() {
    let ctx = Context::root();
    let service = UserQuestionService::install(&ctx);
    let _disposer = service
        .register_provider(Arc::new(EchoProvider))
        .expect("register");

    let bad_label = AskUserQuestionItem {
        intent: Some(AskUserQuestionIntent {
            kind: "plan-review".to_string(),
            approve: "approve".to_string(),
        }),
        detail: Some("plan".to_string()),
        ..plain_question("q1")
    };
    let outcome = service.ask(&request(vec![bad_label])).await;
    assert!(matches!(
        outcome,
        Err(UserQuestionError { code, .. }) if code == "BAD_INTENT"
    ));

    let missing_detail = AskUserQuestionItem {
        intent: Some(AskUserQuestionIntent {
            kind: "plan-review".to_string(),
            approve: "yes".to_string(),
        }),
        detail: None,
        ..plain_question("q1")
    };
    let outcome = service.ask(&request(vec![missing_detail])).await;
    assert!(matches!(
        outcome,
        Err(UserQuestionError { code, .. }) if code == "BAD_INTENT"
    ));

    let valid = AskUserQuestionItem {
        intent: Some(AskUserQuestionIntent {
            kind: "plan-review".to_string(),
            approve: "yes".to_string(),
        }),
        detail: Some("the plan".to_string()),
        ..plain_question("q1")
    };
    assert!(service.ask(&request(vec![valid])).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn aborted_requests_reject_before_the_provider() {
    let ctx = Context::root();
    let service = UserQuestionService::install(&ctx);
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag_for_signal = flag.clone();
    let signal: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || flag_for_signal.load(std::sync::atomic::Ordering::SeqCst));
    let mut request = request(vec![plain_question("q1")]);
    request.signal = Some(signal);
    let outcome = service.ask(&request).await;
    assert!(matches!(
        outcome,
        Err(UserQuestionError { code, .. }) if code == "ASK_ABORTED"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn tool_projects_questions_into_the_answer_shape() {
    let ctx = Context::root();
    let questions = UserQuestionService::install(&ctx);
    let _provider = questions
        .register_provider(Arc::new(EchoProvider))
        .expect("register");
    let system_prompt =
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let tools = dsh_tools::ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    let disposer = dsh_tool_ask_user::apply(&ctx).expect("apply");
    let _ = disposer;
    let _ = system_prompt;

    let input = dsh_tools::ToolExecutionInput {
        call_id: dsh_llm::call_id("ask-1"),
        root_call_id: None,
        name: "ask_user_question".to_string(),
        arguments: serde_json::json!({
            "questions": [{
                "id": "q1",
                "question": "proceed?",
                "header": "Confirm",
                "options": [{ "label": "yes" }, { "label": "no" }],
            }]
        }),
        agent: None,
        parent: None,
        signal: Arc::new(|| false),
    };
    let result = tools.execute(input).await;
    assert!(!result.is_error, "{}", {
        result
            .content
            .iter()
            .map(|block| match block {
                dsh_llm::ContentBlock::Text { text } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("|")
    });
    let value = result.value.clone().expect("value");
    assert_eq!(value["answers"][0]["id"], "q1");
    assert_eq!(value["answers"][0]["selected"], serde_json::json!(["yes"]));

    let schemas = tools.schemas(None);
    assert!(
        schemas
            .iter()
            .any(|schema| schema.name == "ask_user_question")
    );
}
