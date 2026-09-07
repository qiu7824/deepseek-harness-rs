//! The settings schema mirrors the executable Rust compatibility contract.
use dsh_schemastery::{Data, Schema};
pub(crate) fn schema() -> Schema {
    Schema::object(indexmap::IndexMap::from([
        (
            "thinkingTokenBudgetField".into(),
            Schema::union(
                [
                    "thinking_token_budget",
                    "thinking_budget",
                    "thinking_budget_tokens",
                ]
                .into_iter()
                .map(|field| Schema::constant(Data::String(field.into())))
                .collect(),
            ),
        ),
        ("supportsThinkingTokenBudget".into(), Schema::boolean()),
        (
            "thinkingBudgets".into(),
            Schema::dict(Schema::number().min(1.0).max(1_000_000.0).step(1.0), None)
                .default(Data::Undefined),
        ),
        ("supportsReasoningEffort".into(), Schema::boolean()),
        (
            "vllmPriority".into(),
            Schema::number()
                .min(i32::MIN as f64)
                .max(i32::MAX as f64)
                .step(1.0),
        ),
        ("supportsMaxOutputTokens".into(), Schema::boolean()),
    ]))
    .default(Data::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_budgets_are_not_materialized_and_invalid_values_survive_to_validation() {
        let data = super::super::json_to_settings_data(&serde_json::json!({"vllmPriority":1}));
        let value = Schema::validate(&schema(), data)
            .unwrap()
            .to_json()
            .unwrap();
        let mut value = value;
        super::super::model_capabilities::normalize_capacity_numbers(&mut value);
        let config: dsh_llm_deepseek::ProviderCompatibility =
            serde_json::from_value(value).unwrap();
        assert!(config.thinking_budgets.is_none());
        assert!(config.validate("openai-completions").is_ok());
        let data = super::super::json_to_settings_data(&serde_json::json!({"vllmPriority":null}));
        let value = Schema::validate(&schema(), data)
            .unwrap()
            .to_json()
            .unwrap();
        assert!(serde_json::from_value::<dsh_llm_deepseek::ProviderCompatibility>(value).is_err());
    }
}
