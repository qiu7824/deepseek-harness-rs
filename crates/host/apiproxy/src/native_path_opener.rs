//! Cross-platform native path and text-document openers used by the local
//! GUI carrier. Rust port of
//! `packages/host/apiproxy/src/native-path-opener.ts`.
//!
//! The default intent prefers the default browser for documents it renders
//! when the platform can name one, then falls back to the default
//! application. WSL translates every path for the Windows desktop instead of
//! assuming a Linux GUI. The text-editor intent never consults the browser.

use std::sync::Arc;

use dsh_native_command::{NativeCommandAbort, NativeCommandFailure, NativeCommandOutput};
use futures::future::BoxFuture;

/// Testable command boundary; native implementations never invoke a shell.
pub type PathOpenerRunner = Arc<
    dyn Fn(
            &str,
            Vec<String>,
            Option<NativeCommandAbort>,
        ) -> BoxFuture<'static, Result<NativeCommandOutput, NativeCommandFailure>>
        + Send
        + Sync,
>;

/// Injectable platform facts for deterministic adapter tests.
#[derive(Clone)]
pub struct PathOpenerInternals {
    pub platform: Option<&'static str>,
    /// Kernel release override used to distinguish WSL from desktop Linux.
    pub os_release: Option<String>,
    /// Environment used for WSL markers and the desktop Linux browser
    /// convention.
    pub env: Option<std::collections::HashMap<String, String>>,
    pub run: Option<PathOpenerRunner>,
}

impl Default for PathOpenerInternals {
    fn default() -> Self {
        Self {
            platform: None,
            os_release: None,
            env: None,
            run: None,
        }
    }
}

impl PathOpenerInternals {
    fn platform(&self) -> &'static str {
        self.platform.unwrap_or_else(current_platform)
    }
}

/// The current platform in Node `process.platform` vocabulary.
pub fn current_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// The production runner (the TS `runNativeCommand` default).
fn native_runner() -> PathOpenerRunner {
    Arc::new(
        |command: &str, args: Vec<String>, signal: Option<NativeCommandAbort>| {
            let command = command.to_string();
            Box::pin(async move {
                dsh_native_command::run_native_command(&command, &args, signal).await
            })
        },
    )
}

/// Documents a browser renders, as opposed to ones an editor merely edits.
fn is_browser_document(path: &str) -> bool {
    const BROWSER_DOCUMENTS: [&str; 4] = [".html", ".htm", ".xhtml", ".svg"];
    let lower = path.to_lowercase();
    BROWSER_DOCUMENTS
        .iter()
        .any(|extension| lower.ends_with(extension))
}

/// The macOS bundle registered for `https` — the default browser, as
/// LaunchServices records it. The TS plist regex collapse is approximated
/// by a direct block scan (the nested-version stripping only matters for
/// plists whose preferred-version dict precedes the scheme block).
fn mac_bundle_for_https(plist: &str) -> Option<String> {
    let Some(block_start) = plist.find("LSHandlerURLScheme") else {
        return None;
    };
    // Walk back to the enclosing { and forward to the matching }.
    let before = &plist[..block_start];
    let open = before.rfind('{')?;
    let rest = &plist[open..];
    let close = rest.find('}')? + open;
    let block = &plist[open..close];
    let role = block.find("LSHandlerRoleAll")?;
    let after_role = &block[role + "LSHandlerRoleAll".len()..];
    let equals = after_role.find('=')?;
    let after_equals = &after_role[equals + 1..];
    let quoted = after_equals.trim().trim_start_matches('"');
    let end = quoted.find('"')?;
    Some(quoted[..end].to_string())
}

/// PowerShell single-quoted literal (doubles embedded quotes).
fn powershell_literal(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

/// Whether one environment marker is set to a non-empty value.
fn present(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// Distinguish WSL from desktop Linux using its process and kernel markers.
fn is_wsl(internals: &PathOpenerInternals) -> bool {
    let env = internals.env.as_ref();
    if present(env.and_then(|env| env.get("WSL_DISTRO_NAME").map(String::as_str)))
        || present(env.and_then(|env| env.get("WSL_INTEROP").map(String::as_str)))
    {
        return true;
    }
    let release = internals.os_release.clone().unwrap_or_else(|| {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default()
    });
    release.to_lowercase().contains("microsoft")
}

/// Open one Windows-resolvable path through its registered desktop
/// application.
async fn open_windows_path(
    path: &str,
    run: &PathOpenerRunner,
    signal: Option<NativeCommandAbort>,
) -> Result<(), NativeCommandFailure> {
    run(
        "powershell.exe",
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            format!("Invoke-Item -LiteralPath {}", powershell_literal(path)),
        ],
        signal,
    )
    .await
    .map(|_| ())
}

/// Translate a WSL path before handing it to the Windows desktop.
async fn open_wsl_path(
    path: &str,
    run: &PathOpenerRunner,
    signal: Option<NativeCommandAbort>,
) -> Result<(), NativeCommandFailure> {
    let translated = run(
        "wslpath",
        vec!["-w".to_string(), path.to_string()],
        signal.clone(),
    )
    .await?;
    let windows_path = translated.stdout.trim_end_matches(['\r', '\n']);
    if windows_path.is_empty() {
        return Err(NativeCommandFailure {
            message: "wslpath returned no Windows path".to_string(),
            code: None,
            stdout: translated.stdout,
            stderr: translated.stderr,
        });
    }
    open_windows_path(windows_path, run, signal).await
}

/// Dispatch one shell-free platform command for the requested open intent.
async fn open_native_path_with_intent(
    path: &str,
    signal: Option<NativeCommandAbort>,
    intent: PathOpenIntent,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    let platform = internals.platform();
    let run = internals.run.clone().unwrap_or_else(native_runner);
    let wsl = platform == "linux" && is_wsl(internals);

    if !wsl && intent == PathOpenIntent::Default && is_browser_document(path) {
        let browser_took = open_in_browser(path, signal.clone(), platform, &run, internals).await?;
        if browser_took {
            return Ok(());
        }
    }

    match platform {
        "darwin" => run(
            "open",
            match intent {
                PathOpenIntent::TextEditor => vec!["-t".to_string(), path.to_string()],
                PathOpenIntent::Default => vec![path.to_string()],
            },
            signal,
        )
        .await
        .map(|_| ()),
        "win32" => open_windows_path(path, &run, signal).await,
        "linux" => {
            if wsl {
                open_wsl_path(path, &run, signal).await
            } else {
                run("xdg-open", vec![path.to_string()], signal)
                    .await
                    .map(|_| ())
            }
        }
        other => Err(NativeCommandFailure {
            message: format!("native path opener is unsupported on {other}"),
            code: None,
            stdout: String::new(),
            stderr: String::new(),
        }),
    }
}

/// Native path-open intent; macOS distinguishes text editing from file
/// association.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOpenIntent {
    Default,
    TextEditor,
}

/// Open one browser-renderable document with the default browser. Returns
/// true when a browser took it; false when this platform cannot name one —
/// the caller then uses the default application.
async fn open_in_browser(
    path: &str,
    signal: Option<NativeCommandAbort>,
    platform: &str,
    run: &PathOpenerRunner,
    internals: &PathOpenerInternals,
) -> Result<bool, NativeCommandFailure> {
    match platform {
        "darwin" => {
            let bundle = match run(
                "defaults",
                vec![
                    "read".to_string(),
                    "com.apple.LaunchServices/com.apple.launchservices.secure".to_string(),
                ],
                signal.clone(),
            )
            .await
            {
                Ok(output) => mac_bundle_for_https(&output.stdout),
                Err(_) => {
                    // No LaunchServices record: the content-type handler is
                    // then the system's own choice anyway.
                    return Ok(false);
                }
            };
            let Some(bundle) = bundle else {
                return Ok(false);
            };
            run(
                "open",
                vec!["-b".to_string(), bundle, path.to_string()],
                signal,
            )
            .await?;
            Ok(true)
        }
        "linux" => {
            // $BROWSER is the portable convention.
            let browser: Option<String> = internals
                .env
                .as_ref()
                .and_then(|env| env.get("BROWSER").map(|value| value.to_string()))
                .or_else(|| std::env::var("BROWSER").ok());
            let Some(browser) = browser else {
                return Ok(false);
            };
            if browser.is_empty() {
                return Ok(false);
            }
            run(&browser, vec![path.to_string()], signal).await?;
            Ok(true)
        }
        // Windows names no browser without reading the UserChoice registry,
        // and its .html association is the browser in the ordinary case.
        _ => Ok(false),
    }
}

/// Whether `open_native_path` plausibly reaches a desktop on this host.
pub fn can_open_native_path(internals: &PathOpenerInternals) -> bool {
    let platform = internals.platform();
    match platform {
        "darwin" | "win32" => true,
        "linux" => {
            is_wsl(internals)
                || internals
                    .env
                    .as_ref()
                    .is_some_and(|env| present(env.get("DISPLAY").map(String::as_str)))
                || internals
                    .env
                    .as_ref()
                    .is_some_and(|env| present(env.get("WAYLAND_DISPLAY").map(String::as_str)))
        }
        _ => false,
    }
}

/// Open a filesystem path with the operating system's default application,
/// or with the default browser when the path names a document a browser
/// renders.
pub async fn open_native_path(
    path: &str,
    signal: Option<NativeCommandAbort>,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    open_native_path_with_intent(path, signal, PathOpenIntent::Default, internals).await
}

/// Open a text document for editing; macOS bypasses the file-type
/// association.
pub async fn open_native_text_file(
    path: &str,
    signal: Option<NativeCommandAbort>,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    open_native_path_with_intent(path, signal, PathOpenIntent::TextEditor, internals).await
}
