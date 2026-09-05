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
pub type PathLaunchRunner =
    Arc<dyn Fn(&str, Vec<String>) -> Result<(), NativeCommandFailure> + Send + Sync>;

/// Injectable platform facts for deterministic adapter tests.
#[derive(Clone, Default)]
pub struct PathOpenerInternals {
    pub platform: Option<&'static str>,
    /// Kernel release override used to distinguish WSL from desktop Linux.
    pub os_release: Option<String>,
    /// Environment used for WSL markers and the desktop Linux browser
    /// convention.
    pub env: Option<std::collections::HashMap<String, String>>,
    pub run: Option<PathOpenerRunner>,
    pub launch: Option<PathLaunchRunner>,
}

/// A desktop application's lifetime must not become the lifetime of the HTTP action.
async fn launch_gui(
    command: &str,
    args: Vec<String>,
    signal: Option<NativeCommandAbort>,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(NativeCommandFailure {
            message: "打开操作已取消".into(),
            code: Some("ABORT_ERR".into()),
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    if let Some(launch) = &internals.launch {
        return launch(command, args);
    }
    // Preserve the injectable command seam for deterministic callers without launching an OS app.
    if let Some(run) = &internals.run {
        return run(command, args, signal).await.map(|_| ());
    }
    let mut process = tokio::process::Command::new(command);
    process
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let mut child = process.spawn().map_err(|error| NativeCommandFailure {
        message: format!("无法启动本地应用：{error}"),
        code: error.raw_os_error().map(|code| code.to_string()),
        stdout: String::new(),
        stderr: String::new(),
    })?;
    // Reap the process later; neither cancellation nor Host shutdown kills the user's editor.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
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
    let block_start = plist.find("LSHandlerURLScheme")?;
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
        "win32" => match intent {
            PathOpenIntent::TextEditor => {
                launch_gui(
                    "notepad.exe",
                    vec![display_native_path(path)],
                    signal,
                    internals,
                )
                .await
            }
            PathOpenIntent::Default => open_windows_path(path, &run, signal).await,
        },
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

/// Present Windows verbatim paths without changing their filesystem identity.
pub fn display_native_path(path: &str) -> String {
    if let Some(tail) = path.get(8..).filter(|_| {
        path.get(..8)
            .is_some_and(|head| head.eq_ignore_ascii_case(r"\\?\UNC\"))
    }) {
        format!(r"\\{tail}")
    } else if let Some(tail) = path.strip_prefix(r"\\?\") {
        tail.to_string()
    } else {
        path.to_string()
    }
}

/// Reveal the selected file, using argv rather than a command-line interpolation.
pub async fn reveal_native_path(
    path: &str,
    signal: Option<NativeCommandAbort>,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    let run = internals.run.clone().unwrap_or_else(native_runner);
    let mut path = display_native_path(path);
    let mut platform = internals.platform();
    if platform == "linux" && is_wsl(internals) {
        path = run("wslpath", vec!["-w".into(), path], signal.clone())
            .await?
            .stdout
            .trim_end_matches(['\r', '\n'])
            .to_string();
        platform = "win32";
    }
    let (command, args) = match platform {
        "win32" => (
            "explorer.exe",
            if std::path::Path::new(&path).is_dir() {
                vec![path]
            } else {
                vec![format!("/select,{path}")]
            },
        ),
        "darwin" => ("open", vec!["-R".into(), path]),
        "linux" => (
            "xdg-open",
            vec![
                std::path::Path::new(&path)
                    .parent()
                    .unwrap_or(std::path::Path::new(&path))
                    .to_string_lossy()
                    .into_owned(),
            ],
        ),
        _ => {
            return Err(NativeCommandFailure {
                message: "系统不支持定位文件".into(),
                code: None,
                stdout: String::new(),
                stderr: String::new(),
            });
        }
    };
    if platform == "win32" {
        launch_gui(command, args, signal, internals).await
    } else {
        run(command, args, signal).await.map(|_| ())
    }
}

/// Office artifacts are opened through WPS, independently from document associations.
pub async fn open_native_office_file(
    path: &str,
    signal: Option<NativeCommandAbort>,
    internals: &PathOpenerInternals,
) -> Result<(), NativeCommandFailure> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let program = match extension.as_str() {
        "xls" | "xlsx" | "xlsm" | "csv" => "et",
        "ppt" | "pptx" | "pps" | "ppsx" => "wpp",
        _ => "wps",
    };
    let run = internals.run.clone().unwrap_or_else(native_runner);
    match internals.platform() {
        "win32" => {
            let executable = format!("{program}.exe");
            let script = format!(
                "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $p=(Get-Command '{executable}' -ErrorAction SilentlyContinue).Source; if (!$p) {{ foreach ($k in @('HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{executable}','HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{executable}','HKLM:\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{executable}')) {{ if (Test-Path -LiteralPath $k) {{ $p=(Get-Item -LiteralPath $k).GetValue(''); if ($p) {{ break }} }} }} }}; if ($p) {{ [Console]::Out.Write($p) }} else {{ exit 2 }}"
            );
            let found = run(
                "powershell.exe",
                vec!["-NoProfile".into(), "-Command".into(), script],
                signal.clone(),
            )
            .await?;
            let command = found.stdout.trim().trim_matches('"');
            if command.is_empty() {
                return Err(NativeCommandFailure {
                    message: "未找到 WPS Office，请安装或配置 WPS".into(),
                    code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            launch_gui(command, vec![display_native_path(path)], signal, internals).await
        }
        "darwin" => run(
            "open",
            vec!["-a".into(), "WPS Office".into(), path.into()],
            signal,
        )
        .await
        .map(|_| ()),
        _ => launch_gui(program, vec![path.into()], signal, internals).await,
    }
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

#[cfg(test)]
mod file_action_tests {
    use super::*;
    #[test]
    fn display_keeps_unc_and_unicode_paths_readable() {
        assert_eq!(
            display_native_path(r"\\?\D:\资料\空 格\index.ts"),
            r"D:\资料\空 格\index.ts"
        );
        assert_eq!(
            display_native_path(r"\\?\UNC\server\share\文档.docx"),
            r"\\server\share\文档.docx"
        );
        assert_eq!(
            display_native_path(r"\\?\unc\server\share\file.txt"),
            r"\\server\share\file.txt"
        );
        assert_eq!(
            display_native_path("/home/project/index.ts"),
            "/home/project/index.ts"
        );
    }
    #[tokio::test]
    async fn explorer_selection_preserves_filename_as_one_argument() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = calls.clone();
        let internals = PathOpenerInternals {
            platform: Some("win32"),
            run: Some(Arc::new(move |command, args, _| {
                received.lock().unwrap().push((command.to_string(), args));
                Box::pin(async {
                    Ok(NativeCommandOutput {
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                })
            })),
            ..Default::default()
        };
        reveal_native_path(r"\\?\D:\资料\a & b.ts", None, &internals)
            .await
            .unwrap();
        assert_eq!(
            calls.lock().unwrap()[0],
            (
                "explorer.exe".into(),
                vec![r"/select,D:\资料\a & b.ts".into()]
            )
        );
    }
    #[tokio::test]
    async fn editor_launch_does_not_wait_for_the_gui_to_exit() {
        let launched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let recorded = launched.clone();
        let internals = PathOpenerInternals {
            platform: Some("win32"),
            run: Some(Arc::new(|_, _, _| Box::pin(std::future::pending()))),
            launch: Some(Arc::new(move |command, args| {
                assert_eq!(command, "notepad.exe");
                assert_eq!(args, [r"D:\example.txt"]);
                recorded.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            open_native_text_file(r"D:\example.txt", None, &internals),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(launched.load(std::sync::atomic::Ordering::SeqCst));
    }
}
