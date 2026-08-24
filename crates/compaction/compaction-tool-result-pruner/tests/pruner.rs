use dsh_compaction_tool_result_pruner::{
    PRUNE_MARKER, ToolResultPruneConfig, ToolResultPruner, code_point_length, resolve_config,
};
use dsh_llm::{ContentBlock, call_id};

#[test]
fn resolves_node_defaults_and_rejects_invalid_budgets() {
    let defaults = resolve_config(ToolResultPruneConfig::default()).unwrap();
    assert_eq!(
        (
            defaults.threshold_chars,
            defaults.head_chars,
            defaults.tail_chars
        ),
        (8192, 4096, 1024)
    );
    assert!(
        resolve_config(ToolResultPruneConfig {
            threshold_chars: Some(50),
            head_chars: Some(20),
            tail_chars: Some(20)
        })
        .unwrap_err()
        .contains("headChars + marker + tailChars")
    );
}

#[test]
fn prunes_unicode_code_points_and_preserves_rich_block_order() {
    let pruner = ToolResultPruner::standalone(
        resolve_config(ToolResultPruneConfig {
            threshold_chars: Some(50),
            head_chars: Some(4),
            tail_chars: Some(3),
        })
        .unwrap(),
    );
    let rich = ContentBlock::ToolCall {
        id: call_id("nested"),
        name: "nested".into(),
        arguments: "{}".into(),
    };
    let output = pruner
        .prune_content(&[
            ContentBlock::Text {
                text: "😀".repeat(40),
            },
            rich.clone(),
            ContentBlock::Text {
                text: "B".repeat(60),
            },
        ])
        .unwrap();
    assert_eq!(
        output,
        vec![
            ContentBlock::Text {
                text: format!("{}{}", "😀".repeat(4), PRUNE_MARKER)
            },
            rich,
            ContentBlock::Text { text: "BBB".into() },
        ]
    );
    assert_eq!(code_point_length("a😀b"), 3);
    assert!(pruner.measure_content(&output) <= 50);
}
