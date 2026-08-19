//! `@deepseek-ai/dsh-host-frontend-static` — SPA dist server over the
//! webserver fallback seat: traversal outside the dist root is 403, any miss
//! falls back to index.html with 200 (SPA routing), unknown extensions ship as
//! `application/octet-stream`, non-GET/HEAD is 405, and every index response
//! runs through the webserver's registered index taps.
//!
//! # Deviations
//!
//! - The node `ServerResponse` write side is collapsed to a returned
//!   [`dsh_host_webserver::WebResponse`].
//! - `decodeURIComponent` is replaced by a strict percent decoder; malformed
//!   escapes still surface as the webserver's per-request 400.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc, downcast, make_disposer};
use dsh_host_webserver::{WebHandlerError, WebRequest, WebResponse, WebRouteHandler, WebServer};
use futures::future::BoxFuture;
use http::header;
use http::{Method, Response, StatusCode};

/// Stable Cordis plugin name.
pub const NAME: &str = "frontend-static";

/// Service required before the fallback seat can be claimed.
pub const INJECT: [&str; 1] = ["webServer"];

/// Plugin config: the dist anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Absolute path of index.html inside the dist root.
    pub dist_index: String,
}

impl Config {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "frontend-static: config must be an object".to_string())?;
        let dist_index = object
            .get("distIndex")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "frontend-static: distIndex must be a string".to_string())?;
        Ok(Self { dist_index })
    }

    fn from_arcvalue(value: &ArcValue) -> Result<Self, String> {
        if let Some(config) = downcast::<Config>(value).cloned() {
            return Ok(config);
        }
        if let Some(raw) = downcast::<serde_json::Value>(value) {
            return Self::from_value(raw);
        }
        Err("frontend-static: config is not an object".to_string())
    }
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Decode a `decodeURIComponent`-shaped path. Malformed `%` escapes are an
/// error so the webserver answers 400.
pub fn decode_uri_path(path: &str) -> Result<String, WebHandlerError> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 2 >= bytes.len() {
            return Err(WebHandlerError::new("malformed percent escape"));
        }
        let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
        let value = u8::from_str_radix(hex, 16)
            .map_err(|_| WebHandlerError::new("malformed percent escape"))?;
        out.push(value);
        i += 3;
    }
    String::from_utf8(out).map_err(|_| WebHandlerError::new("invalid UTF-8 path"))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

fn absolute_normalized(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("frontend-static: failed to resolve cwd: {error}"))?
            .join(path)
    };
    Ok(normalize_path(&absolute))
}

/// Lexically resolve a decoded pathname under `dist_root` without touching the
/// filesystem. Returns `None` when the normalized target escapes the root.
pub fn resolve_static_target(dist_root: &Path, pathname: &str) -> Option<PathBuf> {
    let mut target = dist_root.to_path_buf();
    for segment in pathname.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                target.pop();
            }
            segment => target.push(segment),
        }
    }
    if target.as_path() == dist_root || target.starts_with(dist_root) {
        Some(target)
    } else {
        None
    }
}

fn empty_response(status: StatusCode) -> WebResponse {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("static response")
}

async fn serve_index_response(
    render_index: &(dyn Fn() -> BoxFuture<'static, Result<String, WebHandlerError>> + Send + Sync),
    is_head: bool,
) -> Result<WebResponse, WebHandlerError> {
    let body = if is_head {
        String::new()
    } else {
        render_index().await?
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_for(Path::new("index.html")))
        .body(Body::from(body))
        .expect("static response"))
}

/// Serve one GET/HEAD static request from the dist root.
pub async fn serve_static(
    pathname: &str,
    dist_root: &Path,
    dist_index: &Path,
    render_index: Arc<
        dyn Fn() -> BoxFuture<'static, Result<String, WebHandlerError>> + Send + Sync,
    >,
    is_head: bool,
) -> Result<WebResponse, WebHandlerError> {
    let Some(target) = resolve_static_target(dist_root, pathname) else {
        return Ok(empty_response(StatusCode::FORBIDDEN));
    };
    if target.as_path() == dist_root || target.as_path() == dist_index {
        return serve_index_response(render_index.as_ref(), is_head).await;
    }
    match tokio::fs::read(&target).await {
        Ok(bytes) => {
            let body = if is_head { Vec::new() } else { bytes };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_for(&target))
                .body(Body::from(body))
                .expect("static response"))
        }
        Err(_) => serve_index_response(render_index.as_ref(), is_head).await,
    }
}

fn render_index(
    server: &Arc<WebServer>,
    dist_index: &Path,
) -> BoxFuture<'static, Result<String, WebHandlerError>> {
    let server = server.clone();
    let dist_index = dist_index.to_path_buf();
    Box::pin(async move {
        let html = tokio::fs::read_to_string(&dist_index)
            .await
            .map_err(|error| {
                WebHandlerError::new(format!("frontend-static: failed to read index: {error}"))
            })?;
        Ok(server.apply_index_taps(&html))
    })
}

/// Claim the webserver fallback seat and serve the dist (TS `apply`).
pub fn apply(ctx: &Context, config: Config) -> Result<cordis::Disposer, String> {
    let server = ctx
        .get_typed::<Arc<WebServer>>("webServer", false)
        .ok_or_else(|| "frontend-static: the webServer service is not configured".to_string())?
        .as_ref()
        .clone();
    let dist_index = PathBuf::from(&config.dist_index);
    let dist_index = absolute_normalized(&dist_index)?;
    let dist_root = dist_index
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "frontend-static: distIndex has no parent directory".to_string())?;

    let handler_server = server.clone();
    let handler_index = dist_index.clone();
    let handler_root = dist_root.clone();
    let handler: WebRouteHandler = Arc::new(move |request: WebRequest| {
        let handler_server = handler_server.clone();
        let handler_index = handler_index.clone();
        let handler_root = handler_root.clone();
        Box::pin(async move {
            if request.method() != Method::GET && request.method() != Method::HEAD {
                return Ok(empty_response(StatusCode::METHOD_NOT_ALLOWED));
            }
            let pathname = decode_uri_path(request.uri().path())?;
            let render_server = handler_server.clone();
            let render_path = handler_index.clone();
            let render = Arc::new(move || render_index(&render_server, &render_path));
            serve_static(
                &pathname,
                &handler_root,
                &handler_index,
                render,
                request.method() == Method::HEAD,
            )
            .await
        })
    });

    let release = server.register_fallback(handler);
    let release_for_effect = release.clone();
    let disposer = ctx.effect(
        "frontend-static: fallback seat",
        Box::pin(async move {
            Some(make_disposer(move || {
                let release = release_for_effect.clone();
                Box::pin(async move {
                    release();
                })
            }))
        }),
    );
    Ok(disposer)
}

/// The Cordis plugin form.
pub struct FrontendStaticPlugin;

#[async_trait]
impl Plugin for FrontendStaticPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config =
            Config::from_arcvalue(&config).map_err(|error| PluginError::new(arc(error)))?;
        apply(ctx, config)
            .map(|_| ())
            .map_err(|error| PluginError::new(arc(error)))
    }
}
