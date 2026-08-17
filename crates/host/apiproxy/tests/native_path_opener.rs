//! Rust port of the core `packages/host/apiproxy/tests/native-path-opener
//! .spec.ts` behaviors: command construction per platform, the browser
//! intent, WSL translation, and the desktop-reachability matrix.

use std::collections::HashMap;
use std::sync::Arc;

use dsh_host_apiproxy::native_path_opener::{
    PathOpenerInternals, PathOpenerRunner, can_open_native_path, open_native_path,
    open_native_text_file,
};
use dsh_native_command::{
    NativeCommandAbort, NativeCommandFailure, NativeCommandOutput,
};
use futures::future::BoxFuture;

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Recording runner: succeeds on every command.
struct RecordingRunner {
    calls: Arc<parking_lot::Mutex<Vec<(String, Vec<String>)>>>,
}

impl RecordingRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(parking_lot::Mutex::new(Vec::new())),
        })
    }

    fn runner(&self) -> PathOpenerRunner {
        let calls = self.calls.clone();
        Arc::new(
            move |command: &str,
                  args: Vec<String>,
                  _signal: Option<NativeCommandAbort>| {
                let calls = calls.clone();
                let command = command.to_string();
                Box::pin(async move {
                    calls.lock().push((command.clone(), args.clone()));
                    Ok(NativeCommandOutput {
                        stdout: match command.as_str() {
                            "wslpath" => "C:\\host\\file.txt\r\n".to_string(),
                            _ => String::new(),
                        },
                        stderr: String::new(),
                    })
                }) as BoxFuture<
                    'static,
                    Result<NativeCommandOutput, NativeCommandFailure>,
                >
            },
        )
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().clone()
    }
}

fn build_internals(
    platform: &'static str,
    runner: &RecordingRunner,
) -> PathOpenerInternals {
    PathOpenerInternals {
        platform: Some(platform),
        os_release: None,
        env: None,
        run: Some(runner.runner()),
    }
}

#[test]
fn windows_opens_through_powershell_invoke_item() {
    run(async {
        let recorder = RecordingRunner::new();
        let internals = build_internals("win32", &recorder);
        open_native_path(r"C:\dir\file.txt", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "powershell.exe");
        assert_eq!(calls[0].1[0], "-NoProfile");
        assert_eq!(calls[0].1[1], "-Command");
        assert_eq!(
            calls[0].1[2],
            "Invoke-Item -LiteralPath 'C:\\dir\\file.txt'"
        );
    });
}

#[test]
fn powershell_literals_double_embedded_quotes() {
    run(async {
        let recorder = RecordingRunner::new();
        let internals = build_internals("win32", &recorder);
        open_native_path(r"C:\dir\a'b.txt", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(
            calls[0].1[2],
            "Invoke-Item -LiteralPath 'C:\\dir\\a''b.txt'"
        );
    });
}

#[test]
fn linux_dispatches_xdg_open_and_uses_browser_for_html() {
    run(async {
        let recorder = RecordingRunner::new();
        let mut internals = build_internals("linux", &recorder);
        internals.env = Some(HashMap::from([(
            "BROWSER".to_string(),
            "firefox".to_string(),
        )]));

        // A browser document prefers $BROWSER.
        open_native_path("/tmp/page.html", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(calls[0].0, "firefox");
        assert_eq!(calls[0].1, vec!["/tmp/page.html".to_string()]);

        // A plain file uses xdg-open.
        open_native_path("/tmp/notes.txt", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(calls.last().unwrap().0, "xdg-open");
        assert_eq!(calls.last().unwrap().1, vec!["/tmp/notes.txt".to_string()]);

        // Without $BROWSER the document falls back to xdg-open.
        let recorder = RecordingRunner::new();
        let internals = build_internals("linux", &recorder);
        open_native_path("/tmp/page.html", None, &internals)
            .await
            .expect("open");
        assert_eq!(recorder.calls()[0].0, "xdg-open");
    });
}

#[test]
fn macos_uses_open_and_text_editor_distinguishes_intent() {
    run(async {
        let recorder = RecordingRunner::new();
        let internals = build_internals("darwin", &recorder);
        open_native_path("/tmp/doc.txt", None, &internals)
            .await
            .expect("open");
        assert_eq!(recorder.calls()[0].0, "open");
        assert_eq!(recorder.calls()[0].1, vec!["/tmp/doc.txt".to_string()]);

        open_native_text_file("/tmp/doc.txt", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(calls.last().unwrap().0, "open");
        assert_eq!(
            calls.last().unwrap().1,
            vec!["-t".to_string(), "/tmp/doc.txt".to_string()]
        );
    });
}

#[test]
fn wsl_translates_paths_before_the_windows_desktop() {
    run(async {
        let recorder = RecordingRunner::new();
        let mut internals = build_internals("linux", &recorder);
        internals.env = Some(HashMap::from([
            ("WSL_DISTRO_NAME".to_string(), "Ubuntu".to_string()),
        ]));
        open_native_path("/mnt/c/host/file.txt", None, &internals)
            .await
            .expect("open");
        let calls = recorder.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "wslpath");
        assert_eq!(calls[0].1, vec!["-w".to_string(), "/mnt/c/host/file.txt".to_string()]);
        assert_eq!(calls[1].0, "powershell.exe");
        assert_eq!(
            calls[1].1[2],
            "Invoke-Item -LiteralPath 'C:\\host\\file.txt'"
        );
    });
}

#[test]
fn desktop_reachability_follows_the_platform_matrix() {
    let desktop = |platform: &'static str, env: HashMap<String, String>| {
        PathOpenerInternals {
            platform: Some(platform),
            os_release: None,
            env: Some(env),
            run: None,
        }
    };
    assert!(can_open_native_path(&desktop("darwin", HashMap::new())));
    assert!(can_open_native_path(&desktop("win32", HashMap::new())));
    assert!(can_open_native_path(&desktop(
        "linux",
        HashMap::from([("DISPLAY".to_string(), ":0".to_string())]),
    )));
    assert!(can_open_native_path(&desktop(
        "linux",
        HashMap::from([("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string())]),
    )));
    assert!(!can_open_native_path(&desktop("linux", HashMap::new())));
    assert!(!can_open_native_path(&desktop("freebsd", HashMap::new())));
}
