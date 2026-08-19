//! Rust port of `packages/host/webserver/tests/webserver.spec.ts`: real HTTP
//! routing precedence, index taps, fallback-seat semantics, per-request error
//! containment, upgrade routing, and fail-loud bind errors.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use cordis::{Context, FiberCore, arc};
use dsh_host_webserver::{
    WebHandlerError, WebRoute, WebRouteKind, WebServer, WebServerPlugin, WebUpgradeRoute,
};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

type Booted = (Context, Arc<FiberCore>, Arc<WebServer>);

async fn boot(port: u16) -> Booted {
    let ctx = Context::root();
    let plugin = Arc::new(WebServerPlugin);
    let fiber = ctx.plugin(
        plugin,
        arc(serde_json::json!({
            "host": "127.0.0.1",
            "port": port,
        })),
    );
    fiber.settle().await.expect("webserver loads");
    let server = ctx
        .get_typed::<Arc<WebServer>>("webServer", false)
        .expect("webServer service is registered")
        .as_ref()
        .clone();
    (ctx, fiber, server)
}

fn text_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(body.to_string()))
        .expect("static response")
}

async fn request(port: u16, path: &str, method: Method) -> (u16, String) {
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
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn get(port: u16, path: &str) -> (u16, String) {
    request(port, path, Method::GET).await
}

/// Reject a malformed percent escape (the fallback static server's decoder).
fn decode_pathname(path: &str) -> Result<String, WebHandlerError> {
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

async fn open_upgrade(port: u16, path: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("upgrade TCP connect");
    stream
        .write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: dsh-test\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("upgrade request written");
    let mut head = [0_u8; 512];
    let read = stream.read(&mut head).await.expect("upgrade response head");
    assert!(read > 0);
    let text = String::from_utf8_lossy(&head[..read]);
    assert!(text.contains("101 Switching Protocols"), "{text}");
    stream
}

#[tokio::test]
async fn routes_fallback_taps_and_upgrades() {
    let (_ctx, fiber, server) = boot(0).await;
    let port = server.port();
    assert!(port > 0);
    assert_eq!(server.host(), "127.0.0.1");

    // Routing precedence: exact beats prefix, longest prefix wins, a prefix
    // route answers its own path, and methods belong to the route.
    let dispose_exact = server.register(WebRoute {
        kind: WebRouteKind::Exact,
        path: "/probe".to_string(),
        handler: Arc::new(|_request| {
            Box::pin(async { Ok(text_response(StatusCode::OK, "EXACT")) })
        }),
    });
    let _dispose_api = server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/api".to_string(),
        handler: Arc::new(|_request| Box::pin(async { Ok(text_response(StatusCode::OK, "API")) })),
    });
    let _dispose_deep = server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/api/deep".to_string(),
        handler: Arc::new(|_request| Box::pin(async { Ok(text_response(StatusCode::OK, "DEEP")) })),
    });
    assert_eq!(get(port, "/probe").await, (200, "EXACT".to_string()));
    assert_eq!(get(port, "/api/anything").await, (200, "API".to_string()));
    assert_eq!(get(port, "/api/deep/leaf").await, (200, "DEEP".to_string()));
    assert_eq!(get(port, "/api").await, (200, "API".to_string()));
    assert_eq!(
        request(port, "/api/anything", Method::POST).await,
        (200, "API".to_string())
    );

    // Fallback seat: 404 while unclaimed, then one owner answers unmatched
    // requests and applies the registered index taps.
    assert_eq!(get(port, "/no/such/route").await.0, 404);
    let untap = server.tap_index(Arc::new(|html| {
        html.replace("<head>", "<head><script>window.__T__=1</script>")
    }));
    assert!(server.apply_index_taps("<head></head>").contains("__T__"));
    let fallback_server = server.clone();
    let release_fallback = server.register_fallback(Arc::new(move |request| {
        let fallback_server = fallback_server.clone();
        Box::pin(async move {
            let path = decode_pathname(request.uri().path())?;
            let _ = path;
            let html = fallback_server.apply_index_taps("<head></head><body>shell</body>");
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html")
                .body(Body::from(html))
                .expect("valid response");
            Ok(response)
        })
    }));
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = server.register_fallback(Arc::new(|_request| {
            Box::pin(async { Ok(text_response(StatusCode::OK, "NOPE")) })
        }));
    }));
    assert!(duplicate.is_err());
    let (status, body) = get(port, "/no/such/route").await;
    assert_eq!(status, 200);
    assert!(body.contains("__T__"));
    assert!(body.contains("shell"));
    untap();
    let (_, body) = get(port, "/no/such/route").await;
    assert!(!body.contains("__T__"));
    assert!(body.contains("shell"));

    // Per-request error containment: malformed %-escape answers 400 and the
    // server keeps serving afterwards.
    assert_eq!(get(port, "/%zz").await.0, 400);
    assert_eq!(get(port, "/probe").await, (200, "EXACT".to_string()));

    // Duplicate routes throw; disposers restore registrability.
    let duplicate_route = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = server.register(WebRoute {
            kind: WebRouteKind::Exact,
            path: "/probe".to_string(),
            handler: Arc::new(|_request| {
                Box::pin(async { Ok(text_response(StatusCode::OK, "BAD")) })
            }),
        });
    }));
    assert!(duplicate_route.is_err());
    dispose_exact();
    assert_eq!(get(port, "/probe").await.0, 200); // now answered by fallback
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = server.register(WebRoute {
                kind: WebRouteKind::Exact,
                path: "/probe".to_string(),
                handler: Arc::new(|_request| {
                    Box::pin(async { Ok(text_response(StatusCode::OK, "EXACT")) })
                }),
            });
        }))
        .is_ok()
    );

    // Releasing the fallback restores unclaimed 404 and registrability.
    release_fallback();
    assert_eq!(get(port, "/no/such/route").await.0, 404);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = server.register_fallback(Arc::new(|_request| {
                Box::pin(async { Ok(text_response(StatusCode::OK, "SECOND")) })
            }));
        }))
        .is_ok()
    );

    // Upgrade routes match exact pathnames and reject duplicate ownership.
    let dispose_upgrade = server.register_upgrade(WebUpgradeRoute {
        path: "/events".to_string(),
        handler: Arc::new(|_request, mut socket| {
            Box::pin(async move {
                let mut buffer = [0_u8; 256];
                let _ = socket.read(&mut buffer).await;
                Ok(())
            })
        }),
    });
    let duplicate_upgrade = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = server.register_upgrade(WebUpgradeRoute {
            path: "/events".to_string(),
            handler: Arc::new(|_request, _socket| Box::pin(async { Ok(()) })),
        });
    }));
    assert!(duplicate_upgrade.is_err());
    let mut upgraded = open_upgrade(port, "/events?stream=mux").await;
    dispose_upgrade();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = server.register_upgrade(WebUpgradeRoute {
                path: "/events".to_string(),
                handler: Arc::new(|_request, _socket| Box::pin(async { Ok(()) })),
            });
        }))
        .is_ok()
    );

    // A failing upgrade handler must not take the server down.
    let _ = server.register_upgrade(WebUpgradeRoute {
        path: "/upgrade-error".to_string(),
        handler: Arc::new(|_request, _socket| {
            Box::pin(async { Err(WebHandlerError::new("test upgrade transport failure")) })
        }),
    });
    {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("upgrade-error TCP connect");
        stream
            .write_all(
                format!(
                    "GET /upgrade-error HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: dsh-test\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("upgrade-error request written");
        let mut buffer = Vec::new();
        let read_to_end =
            tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buffer)).await;
        assert!(read_to_end.is_ok(), "upgrade-error socket closes");
        assert!(String::from_utf8_lossy(&buffer).contains("101 Switching Protocols"));
    }
    assert_eq!(get(port, "/probe").await, (200, "EXACT".to_string()));

    // Fiber teardown closes the accepted upgrade socket and the listener.
    fiber.dispose().await;
    let mut buffer = [0_u8; 32];
    let closed = tokio::time::timeout(Duration::from_secs(3), upgraded.read(&mut buffer)).await;
    match closed {
        Ok(Ok(0)) | Ok(Err(_)) => {}
        other => panic!("upgraded socket did not close during teardown: {other:?}"),
    }
}

#[tokio::test]
async fn bind_failure_is_fail_loud() {
    let (_first_ctx, first_fiber, first_server) = boot(0).await;
    let taken_port = first_server.port();

    let ctx = Context::root();
    let plugin = Arc::new(WebServerPlugin);
    let fiber = ctx.plugin(
        plugin,
        arc(serde_json::json!({
            "host": "127.0.0.1",
            "port": taken_port,
        })),
    );
    let failure = fiber.settle().await.expect_err("second bind fails");
    assert!(!failure.message().is_empty());
    fiber.dispose().await;

    first_fiber.dispose().await;
}
