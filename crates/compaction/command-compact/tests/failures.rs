use dsh_command_compact::expected_failure_text;
use dsh_compaction::ManualCompactionErrorCode;

#[test]
fn maps_every_node_manual_failure_message() {
    let cases = [
        (
            ManualCompactionErrorCode::Busy,
            "Compaction is unavailable because this process has an active compaction, or the agent is not idle.",
        ),
        (
            ManualCompactionErrorCode::Cancelled,
            "Compaction cancelled.",
        ),
        (
            ManualCompactionErrorCode::Changed,
            "The history selected for compaction changed before it could be replaced. The conversation is unchanged; the attempt is recorded in the session log.",
        ),
        (
            ManualCompactionErrorCode::Summary,
            "Compaction could not produce a useful summary. The conversation is unchanged; the attempt is recorded in the session log.",
        ),
        (
            ManualCompactionErrorCode::Commit,
            "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying.",
        ),
        (
            ManualCompactionErrorCode::Persistence,
            "Compaction finished, but the session could not be saved.",
        ),
    ];
    for (code, text) in cases {
        assert_eq!(expected_failure_text(code), text);
    }
}
