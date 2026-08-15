//! Rust port of the TS `render.spec.ts` suite for `dsh-shell`: the
//! `[exit code: N]` / `[killed by signal: X]` marker parse contract.

use dsh_shell::{ParsedExitStatus, parse_exit_status};

#[test]
fn recovers_a_clean_exit_zero_with_the_body_verbatim_when_no_marker_is_present() {
    assert_eq!(
        parse_exit_status("hi\n\n"),
        ParsedExitStatus::Exit {
            body: "hi\n\n".to_string(),
            exit_code: 0,
        }
    );
    assert_eq!(
        parse_exit_status(""),
        ParsedExitStatus::Exit {
            body: String::new(),
            exit_code: 0,
        }
    );
}

#[test]
fn recovers_a_nonzero_exit_and_strips_only_its_marker_from_the_body() {
    assert_eq!(
        parse_exit_status("oops\n[exit code: 3]"),
        ParsedExitStatus::Exit {
            body: "oops".to_string(),
            exit_code: 3,
        }
    );
    // The marker needs the leading newline and the end of the string, so a
    // clean result whose output merely ENDS in marker-like text is not read
    // as a failure and the text stays in the body.
    assert_eq!(
        parse_exit_status("[exit code: 5]"),
        ParsedExitStatus::Exit {
            body: "[exit code: 5]".to_string(),
            exit_code: 0,
        }
    );
}

#[test]
fn recovers_a_signal_kill_ahead_of_any_nonzero_exit_marker() {
    assert_eq!(
        parse_exit_status("gone\n[killed by signal: SIGKILL]"),
        ParsedExitStatus::Signal {
            body: "gone".to_string(),
            signal: "SIGKILL".to_string(),
        }
    );
    // A fake signal marker with no leading newline is output, not a kill.
    assert_eq!(
        parse_exit_status("[killed by signal: SIGKILL]"),
        ParsedExitStatus::Exit {
            body: "[killed by signal: SIGKILL]".to_string(),
            exit_code: 0,
        }
    );
}

#[test]
fn keeps_markers_no_pill_shows_in_the_body() {
    assert_eq!(
        parse_exit_status("slow\n[timed out after 100ms]\n[exit code: 143]"),
        ParsedExitStatus::Exit {
            body: "slow\n[timed out after 100ms]".to_string(),
            exit_code: 143,
        }
    );
}
