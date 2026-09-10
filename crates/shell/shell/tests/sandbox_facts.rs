use dsh_sandbox::{ConfinedArgv, RunnerFailureRule, SandboxEnforcement, SandboxMode};
use dsh_shell::ShellSandboxInfo;

#[test]
fn command_exit_does_not_masquerade_as_runner_failure_or_bypass_allowed_exit_rules() {
    let confined = ConfinedArgv {
        startup: None,
        argv: vec![],
        enforcement: SandboxEnforcement::Full,
        denial_signatures: vec!["access is denied".into()],
        runner_failure_rules: vec![RunnerFailureRule {
            allowed_exit_codes: Some(vec![125]),
            fatal_signatures: vec!["runner:".into()],
            informational_lines: Some(vec!["runner: ready".into()]),
        }],
    };
    let observe = |exit, text| {
        ShellSandboxInfo::observe(SandboxMode::WorkspaceWrite, &confined, Some(exit), text)
    };
    assert_eq!(observe(125, "command exited").runner_failed, Some(false));
    assert_eq!(observe(1, "runner: failed").runner_failed, Some(false));
    assert_eq!(observe(125, "runner: ready").runner_failed, Some(false));
    assert_eq!(
        observe(125, "RUNNER: cannot start").runner_failed,
        Some(true)
    );
    assert!(!observe(125, "runner: access is denied").denied);
    assert!(observe(1, "Access is denied").denied);
    assert!(!observe(0, "access is denied").denied);
    assert!(!observe(1, "[sandbox-cleanup] access is denied\ncommand failed").denied);
}
