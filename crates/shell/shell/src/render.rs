//! Shared rendering helpers for the shell tools (`dsh-tool-bash`,
//! `dsh-tool-pwsh`): the exit-status marker contract the tools' renderers
//! emit and the presentation layer parses back. Rust port of
//! `packages/shell/shell/src/render.ts`.

use std::sync::OnceLock;

use regex::Regex;

/// The exit status recovered from a rendered result, with the output body
/// that status was split off from (TS `ParsedExitStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedExitStatus {
    /// A recovered exit code (absent any marker means a clean exit 0).
    Exit { body: String, exit_code: i32 },
    /// A recovered signal kill.
    Signal { body: String, signal: String },
}

fn signal_marker() -> &'static Regex {
    static MARKER: OnceLock<Regex> = OnceLock::new();
    MARKER.get_or_init(|| Regex::new(r"\n\[killed by signal: ([^\]\n]+)\]$").expect("valid regex"))
}

fn exit_marker() -> &'static Regex {
    static MARKER: OnceLock<Regex> = OnceLock::new();
    MARKER.get_or_init(|| Regex::new(r"\n\[exit code: (\d+)\]$").expect("valid regex"))
}

/// Split a rendered shell-tool result string into its output body and the
/// structured exit status — the inverse of the `[exit code: N]` /
/// `[killed by signal: X]` markers the shell tools' renderers append. A
/// killed marker yields `signal`; otherwise a non-zero marker yields
/// `exit_code`; absent both means a clean exit 0.
///
/// The consumed marker is removed from `body` because a terminal
/// presentation shows the exit status as its own pill: leaving the marker in
/// the output would render the exit twice. Other markers (timeout, sandbox
/// denial) carry facts no pill shows, so they stay in the body (TS
/// `parseExitStatus`).
pub fn parse_exit_status(text: &str) -> ParsedExitStatus {
    if let Some(captures) = signal_marker().captures(text) {
        let matched = captures.get(0).expect("full match");
        return ParsedExitStatus::Signal {
            body: text[..matched.start()].to_string(),
            signal: captures.get(1).expect("signal group").as_str().to_string(),
        };
    }
    if let Some(captures) = exit_marker().captures(text) {
        let matched = captures.get(0).expect("full match");
        return ParsedExitStatus::Exit {
            body: text[..matched.start()].to_string(),
            exit_code: captures
                .get(1)
                .and_then(|digits| digits.as_str().parse().ok())
                .unwrap_or(0),
        };
    }
    ParsedExitStatus::Exit {
        body: text.to_string(),
        exit_code: 0,
    }
}
