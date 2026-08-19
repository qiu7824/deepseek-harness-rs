//! Rust port of `packages/host/frontend-static/tests/frontend-static.spec.ts`:
//! real HTTP assertions over the webserver fallback seat — asset serving,
//! MIME mapping, SPA index fallback with index taps, traversal rejection,
//! method gating, and seat release on fiber disposal.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use bytes::Bytes;
use cordis::{Context, FiberCore, arc};
use dsh_host_frontend_static::FrontendStaticPlugin;
use dsh_host_webserver::{WebServer, WebServerPlugin};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

static NEXT_TMP: AtomicU64 = AtomicU64::new(1);

struct Booted {
    ctx: Context,
    webserver_fiber: Arc<FiberCore>,
    frontend_fiber: Arc<FiberCore>,
    server: Arc<WebServer>,
    root: PathBuf,
}

async fn write(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .expect("create parent");
    }
    tokio::fs::write(path, body).await.expect("write fixture");
}

async fn boot() -> Booted {
    let sequence = NEXT_TMP.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "dsh-frontend-static-{}-{}",
        std::process::id(),
        sequence
    ));
    let dist = root.join("dist");
    let dist_index = dist.join("index.html");
    write(&dist_index, "<head></head><body>shell</body>").await;
    write(&dist.join("app.js"), "export {}").await;
    write(&dist.join("blob.bin"), "BLOB").await;
    write(&dist.join("manifest.webmanifest"), "{}").await;

    let ctx = Context::root();
    let webserver_plugin = Arc::new(WebServerPlugin);
    let webserver_fiber = ctx.plugin(
        webserver_plugin,
        arc(serde_json::json!({
            "host": "127.0.0.1",
            "port": 0,
        })),
    );
    webserver_fiber.settle().await.expect("webserver loads");

    let frontend_plugin = Arc::new(FrontendStaticPlugin);
    let frontend_fiber = ctx.plugin(
        frontend_plugin,
        arc(serde_json::json!({
            "distIndex": dist_index.to_string_lossy().to_string(),
        })),
    );
    frontend_fiber
        .settle()
        .await
        .expect("frontend-static loads");

    let server = ctx
        .get_typed::<Arc<WebServer>>("webServer", false)
        .expect("webServer service is registered")
        .as_ref()
        .clone();

    Booted {
        ctx,
        webserver_fiber,
        frontend_fiber,
        server,
        root,
    }
}

fn text_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(body.to_string()))
        .expect("static response")
}

async fn request(port: u16, path: &str, method: Method) -> (u16, Option<String>, String) {
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("server accepts TCP");
    let io = TokioIo::new(stream);
    let (mut sender, connection) = http1::handshake(io).await.expect("http1 handshake");
    tokio::spawn(connection);
    let uri = format!("http://127.0.0.1:{port}{path}");
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Full::new(Bytes::new()))
        .expect("valid request");
    let response = sender.send_request(request).await.expect("response");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        content_type,
        String::from_utf8_lossy(&body).to_string(),
    )
}

async fn get(port: u16, path: &str) -> (u16, Option<String>, String) {
    request(port, path, Method::GET).await
}

#[tokio::test]
async fn serves_dist_with_spa_fallback_taps_traversal_and_method_gating() {
    let booted = boot().await;
    let port = booted.server.port();

    // Real assets with their MIME types; a live rebuild is served on next read.
    let app = get(port, "/app.js").await;
    assert_eq!(app.0, 200);
    assert_eq!(app.1.as_deref(), Some("text/javascript; charset=utf-8"));
    assert_eq!(app.2, "export {}");

    let manifest = get(port, "/manifest.webmanifest").await;
    assert_eq!(manifest.0, 200);
    assert_eq!(manifest.1.as_deref(), Some("application/manifest+json"));
    assert_eq!(manifest.2, "{}");

    write(
        &booted.root.join("dist/app.js"),
        "export const rebuilt = true",
    )
    .await;
    assert_eq!(get(port, "/app.js").await.2, "export const rebuilt = true");

    // Unknown extension ships as octet-stream.
    let blob = get(port, "/blob.bin").await;
    assert_eq!(blob.0, 200);
    assert_eq!(blob.1.as_deref(), Some("application/octet-stream"));
    assert_eq!(blob.2, "BLOB");

    // `/`, the index path, and any miss all render index.html through taps.
    let untap = booted.server.tap_index(Arc::new(|html| {
        html.replace("<head>", "<head><script>window.__T__=1</script>")
    }));
    for path in ["/", "/index.html", "/no/such/route"] {
        let got = get(port, path).await;
        assert_eq!(got.0, 200, "{path}");
        assert!(got.2.contains("__T__"), "{path}: {}", got.2);
        assert!(got.2.contains("shell"), "{path}: {}", got.2);
    }
    untap();
    assert!(!get(port, "/").await.2.contains("__T__"));

    // Traversal outside the dist root is 403; non-GET/HEAD is 405.
    assert_eq!(get(port, "/..%2f..%2fetc%2fpasswd").await.0, 403);
    assert_eq!(request(port, "/nowhere", Method::POST).await.0, 405);

    // HMR safety: disposing the frontend row releases the fallback seat; the
    // unclaimed webserver answers 404 and the seat is claimable again.
    booted.frontend_fiber.dispose().await;
    assert_eq!(get(port, "/no/such/route").await.0, 404);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = booted.server.register_fallback(Arc::new(|_request| {
                Box::pin(async { Ok(text_response(StatusCode::OK, "SECOND")) })
            }));
        }))
        .is_ok()
    );

    booted.webserver_fiber.dispose().await;
    let _ = booted.ctx.fiber.state();
    let _ = tokio::fs::remove_dir_all(&booted.root).await;
}
