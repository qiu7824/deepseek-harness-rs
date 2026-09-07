use dsh_agent::model_selection::model_switch_notice;
use dsh_llm::LlmCallConfig;
#[test]
fn switch_notice_ignores_effort_and_initial_selection() {
    let old = LlmCallConfig {
        provider: "one".into(),
        model: "model-a".into(),
        ..Default::default()
    };
    let mut selected = old.clone();
    assert!(model_switch_notice(None, &selected).is_none());
    selected.reasoning_effort = Some(dsh_llm::ReasoningEffortId::new("high"));
    assert!(model_switch_notice(Some(&old), &selected).is_none());
    selected.model = "model-b".into();
    let notice = model_switch_notice(Some(&old), &selected).unwrap();
    assert!(
        serde_json::to_string(&notice)
            .unwrap()
            .contains("one/model-a to one/model-b")
    );
    assert!(model_switch_notice(Some(&selected), &selected).is_none());
}
