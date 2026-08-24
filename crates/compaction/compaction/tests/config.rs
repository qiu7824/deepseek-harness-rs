use dsh_compaction::basic::{
    BasicCompactionConfig, ModelCompactPolicyConfig, RetentionConfig, resolve_compact_spec,
    resolve_config, resolve_target_policy,
};

#[test]
fn resolves_node_defaults_and_exact_model_policy() {
    let config = resolve_config(BasicCompactionConfig {
        model_policies: vec![ModelCompactPolicyConfig {
            provider: "deepseek".into(),
            model: "chat".into(),
            threshold_ratio: Some(0.7),
            retention: Some(RetentionConfig::Tokens(123)),
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(config.threshold_ratio, 0.8);
    assert_eq!(config.retention, RetentionConfig::Ratio(0.16));
    assert_eq!(config.max_tokens, 8192);
    assert_eq!(config.compaction_retries, 1);
    assert_eq!(config.max_overflow_retries, 1);
    assert!(config.auto);

    let policy = resolve_target_policy(&config, "deepseek", "chat");
    assert_eq!(policy.threshold_ratio, 0.7);
    assert_eq!(policy.retention, RetentionConfig::Tokens(123));
    let spec = resolve_compact_spec(&policy, 10_000).unwrap();
    assert_eq!(spec.threshold_tokens, 7_000);
    assert_eq!(spec.retain_tokens, 123);
}

#[test]
fn rejects_node_configuration_conflicts() {
    let duplicate = ModelCompactPolicyConfig {
        provider: "p".into(),
        model: "m".into(),
        ..Default::default()
    };
    assert!(
        resolve_config(BasicCompactionConfig {
            model_policies: vec![duplicate.clone(), duplicate],
            ..Default::default()
        })
        .unwrap_err()
        .contains("duplicate model policy for p/m")
    );
    assert!(
        resolve_config(BasicCompactionConfig {
            threshold_ratio: Some(0.2),
            retention: Some(RetentionConfig::Ratio(0.2)),
            ..Default::default()
        })
        .unwrap_err()
        .contains("must be less than")
    );
}
