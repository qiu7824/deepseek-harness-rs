//! User-owned desktop applications opened from registered workspaces.
use axum::body::{Body, to_bytes};
use cordis::Context;
use dsh_host_webserver::{WebRequest, WebResponse, WebRoute, WebRouteKind, WebServer};
use http::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct App {
    id: &'static str,
    name: &'static str,
    executable: PathBuf,
    arguments: Vec<String>,
}
#[derive(Default)]
struct Catalog {
    cached: parking_lot::Mutex<Option<(Instant, Vec<App>)>>,
}
impl Catalog {
    fn list(&self) -> Vec<App> {
        let mut cached = self.cached.lock();
        if let Some((at, apps)) = &*cached {
            if at.elapsed() < Duration::from_secs(30) {
                return apps.clone();
            }
        }
        let apps = discover();
        *cached = Some((Instant::now(), apps.clone()));
        apps
    }
    fn invalidate(&self) {
        *self.cached.lock() = None;
    }
}
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|path| path.is_file())
    })
}
fn add(
    apps: &mut Vec<App>,
    id: &'static str,
    name: &'static str,
    paths: Vec<PathBuf>,
    arguments: &[&str],
) {
    if let Some(executable) = paths.into_iter().find(|path| path.is_file()) {
        apps.push(App {
            id,
            name,
            executable,
            arguments: arguments.iter().map(|s| s.to_string()).collect(),
        })
    }
}
fn discover() -> Vec<App> {
    let mut apps = Vec::new();
    #[cfg(windows)]
    {
        let windows = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:/Windows"));
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let program = std::env::var_os("ProgramFiles").map(PathBuf::from);
        let candidates = |relative: &str| {
            local
                .iter()
                .map(|base| base.join("Programs").join(relative))
                .chain(program.iter().map(|base| base.join(relative)))
                .collect()
        };
        add(
            &mut apps,
            "files",
            "文件资源管理器",
            vec![windows.join("explorer.exe")],
            &[],
        );
        add(
            &mut apps,
            "vscode",
            "Visual Studio Code",
            candidates("Microsoft VS Code/Code.exe"),
            &["--reuse-window"],
        );
        add(
            &mut apps,
            "cursor",
            "Cursor",
            candidates("cursor/Cursor.exe"),
            &["--reuse-window"],
        );
        add(
            &mut apps,
            "windsurf",
            "Windsurf",
            candidates("Windsurf/Windsurf.exe"),
            &["--reuse-window"],
        );
        add(&mut apps, "zed", "Zed", candidates("Zed/zed.exe"), &[]);
        add(
            &mut apps,
            "terminal",
            "Windows Terminal",
            local
                .iter()
                .map(|base| base.join("Microsoft/WindowsApps/wt.exe"))
                .collect(),
            &["-d"],
        );
        let shells = on_path("pwsh.exe")
            .into_iter()
            .chain(
                program
                    .iter()
                    .map(|base| base.join("PowerShell/7/pwsh.exe")),
            )
            .chain(std::iter::once(
                windows.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            ))
            .collect();
        add(&mut apps, "powershell", "PowerShell", shells, &["-NoExit"]);
    }
    #[cfg(target_os = "macos")]
    {
        let open = PathBuf::from("/usr/bin/open");
        if open.is_file() {
            apps.push(App {
                id: "files",
                name: "Finder",
                executable: open.clone(),
                arguments: vec![],
            });
            for (id, name, bundle) in [
                ("vscode", "Visual Studio Code", "Visual Studio Code.app"),
                ("cursor", "Cursor", "Cursor.app"),
                ("windsurf", "Windsurf", "Windsurf.app"),
                ("zed", "Zed", "Zed.app"),
                ("terminal", "Terminal", "Utilities/Terminal.app"),
            ] {
                let paths = [
                    PathBuf::from("/Applications").join(bundle),
                    PathBuf::from("/System/Applications").join(bundle),
                ];
                if let Some(bundle) = paths.into_iter().find(|path| path.is_dir()) {
                    apps.push(App {
                        id,
                        name,
                        executable: open.clone(),
                        arguments: vec!["-a".into(), bundle.to_string_lossy().into_owned()],
                    })
                }
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (id, name, program, arguments) in [
            ("files", "文件管理器", "xdg-open", vec![]),
            (
                "vscode",
                "Visual Studio Code",
                "code",
                vec!["--reuse-window"],
            ),
            ("cursor", "Cursor", "cursor", vec!["--reuse-window"]),
            ("windsurf", "Windsurf", "windsurf", vec!["--reuse-window"]),
            ("zed", "Zed", "zed", vec![]),
            (
                "gnome-terminal",
                "Terminal",
                "gnome-terminal",
                vec!["--working-directory"],
            ),
            ("konsole", "Konsole", "konsole", vec!["--workdir"]),
        ] {
            add(
                &mut apps,
                id,
                name,
                on_path(program).into_iter().collect(),
                &arguments,
            );
        }
    }
    apps
}
fn display_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{path}");
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return path.to_string();
        }
    }
    path.into_owned()
}
fn arguments(app: &App, path: &Path) -> Vec<String> {
    let mut arguments = app.arguments.clone();
    if app.id != "powershell" {
        arguments.push(display_path(path));
    }
    arguments
}
async fn launch(app: App, directory: PathBuf) -> Result<(), String> {
    let arguments = arguments(&app, &directory);
    let mut command = Command::new(&app.executable);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000200);
    }
    // These are user applications: relinquish ownership after startup, never put them
    // in an agent command's kill-on-close job or terminate them after a probe timeout.
    let mut child = tokio::process::Command::from(command)
        .spawn()
        .map_err(|error| format!("无法启动 {}：{error}", app.name))?;
    tokio::time::sleep(Duration::from_millis(120)).await;
    match child.try_wait().map_err(|error| error.to_string())? {
        Some(status) if !status.success() => Err(format!("{} 启动失败：{status}", app.name)),
        Some(_) => Ok(()),
        None => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            Ok(())
        }
    }
}
fn response(status: StatusCode, value: Value) -> WebResponse {
    http::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(value.to_string()))
        .expect("open app response")
}
fn error(status: StatusCode, code: &str, message: &str) -> WebResponse {
    response(status, json!({"error":code,"message":message}))
}
async fn handle(
    request: WebRequest,
    ctx: Context,
    catalog: Arc<Catalog>,
    allow_remote: bool,
) -> WebResponse {
    if !super::trusted_web_request(&request, allow_remote) {
        return error(StatusCode::FORBIDDEN, "forbidden", "请求来源不可信");
    }
    if request.method() != Method::POST {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "method-not-allowed",
            "仅支持 POST",
        );
    }
    let path = request.uri().path().to_string();
    let bytes = match to_bytes(Body::new(request.into_body()), 4096).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid-request", "请求过大"),
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid-request", "请求格式无效"),
    };
    let Some(workspace_id) = body["workspaceId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 200)
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "workspace-required",
            "请选择工作区",
        );
    };
    let Some(registry) =
        ctx.get_typed::<Arc<dsh_workspace::WorkspaceRegistry>>("workspaceRegistry", false)
    else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "workspace-unavailable",
            "工作区服务尚未就绪",
        );
    };
    let Some(workspace) = registry.get(&dsh_workspace::workspace_id(workspace_id)) else {
        return error(StatusCode::NOT_FOUND, "workspace-not-found", "工作区不存在");
    };
    let directory = match std::fs::canonicalize(workspace.path()) {
        Ok(path) if path.is_dir() => path,
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                "directory-not-found",
                "工作区目录已不存在",
            );
        }
    };
    let apps = catalog.list();
    if path == "/__dsh-open-in-app/meta" {
        return response(
            StatusCode::OK,
            json!({"apps":apps.iter().map(|app|json!({"id":app.id,"name":app.name})).collect::<Vec<_>>(),"path":display_path(&directory),"hostLocal":true}),
        );
    }
    if path != "/__dsh-open-in-app/open" {
        return error(StatusCode::NOT_FOUND, "not-found", "入口不存在");
    }
    let Some(app) = apps
        .into_iter()
        .find(|app| Some(app.id) == body["appId"].as_str())
    else {
        catalog.invalidate();
        return error(
            StatusCode::BAD_REQUEST,
            "app-unavailable",
            "应用未安装或已移除，请重新打开菜单",
        );
    };
    let id = app.id;
    match launch(app, directory).await {
        Ok(()) => response(StatusCode::OK, json!({"opened":true,"appId":id})),
        Err(message) => {
            catalog.invalidate();
            error(StatusCode::BAD_GATEWAY, "launch-failed", &message)
        }
    }
}
pub(super) fn register(server: &Arc<WebServer>, ctx: &Context, allow_remote: bool) {
    let catalog = Arc::new(Catalog::default());
    let ctx = ctx.clone();
    let _ = server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-open-in-app".into(),
        handler: Arc::new(move |request| {
            let ctx = ctx.clone();
            let catalog = catalog.clone();
            Box::pin(async move { Ok(handle(request, ctx, catalog, allow_remote).await) })
        }),
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_is_a_single_literal_argument() {
        let app = App {
            id: "vscode",
            name: "test",
            executable: "editor".into(),
            arguments: vec!["--reuse-window".into()],
        };
        let path = Path::new("C:/中文 project/$(literal) & folder");
        assert_eq!(
            arguments(&app, path),
            vec!["--reuse-window".to_string(), display_path(path)]
        );
    }
    #[test]
    fn powershell_uses_current_directory_without_shell_code() {
        let app = App {
            id: "powershell",
            name: "test",
            executable: "pwsh.exe".into(),
            arguments: vec!["-NoExit".into()],
        };
        assert_eq!(arguments(&app, Path::new("C:/用户目录")), vec!["-NoExit"]);
    }
}
