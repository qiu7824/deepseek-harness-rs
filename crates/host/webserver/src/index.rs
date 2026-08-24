//! `@deepseek-ai/dsh-host-webserver` — Web route-registration plugin: a
//! hyper/HTTP server plus the `webServer` service (HTTP and upgrade route
//! registries, index transform taps, and the single fallback seat for
//! everything no route claims). Knows no harness concepts and serves no files;
//! the composing application's frontend plugin owns dist serving through the
//! fallback hook.
//!
//! # Deviations
//!
//! - Node's raw `http.Server` response object is collapsed to a returned
//!   [`WebResponse`]; streaming handlers return an axum [`Body`] stream.
//! - An unmatched upgrade request receives a 400 response instead of a bare
//!   socket destroy. A matched upgrade receives the HTTP 101 response from
//!   hyper after [`hyper::upgrade::on`] is claimed, matching the observable
//!   node contract.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use cordis::{ArcValue, Context, Plugin, PluginError, Service, arc, downcast, make_disposer};
use futures::FutureExt;
use futures::future::BoxFuture;
use http::header;
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Cordis plugin name (the Rust static-registry equivalent of the TS package
/// entry name `@deepseek-ai/dsh-host-webserver`).
pub const NAME: &str = "host-webserver";

/// The service name under which the running server is exposed.
pub const SERVICE_NAME: &str = "webServer";

/// Route match kind: `Exact` matches the pathname verbatim; `Prefix` matches
/// `p` and `p/<anything>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRouteKind {
    Exact,
    Prefix,
}

/// One named HTTP route registration.
#[derive(Clone)]
pub struct WebRoute {
    pub kind: WebRouteKind,
    /// Absolute pathname, no trailing slash.
    pub path: String,
    /// Owns the full response lifecycle (may hold the response open, e.g. SSE).
    pub handler: WebRouteHandler,
}

/// One exact-path HTTP upgrade registration.
#[derive(Clone)]
pub struct WebUpgradeRoute {
    /// Absolute pathname, no trailing slash.
    pub path: String,
    /// Owns protocol negotiation and the upgraded socket after dispatch.
    pub handler: WebUpgradeHandler,
}

/// The configured bind host (loopback or all interfaces).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Loopback,
    AllInterfaces,
}

impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "127.0.0.1",
            Self::AllInterfaces => "0.0.0.0",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "127.0.0.1" => Some(Self::Loopback),
            "0.0.0.0" => Some(Self::AllInterfaces),
            _ => None,
        }
    }
}

/// Gateway config: the listen address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub host: Host,
    /// Listen port; zero requests an OS-assigned port.
    pub port: u16,
}

impl Config {
    /// Parse the loader/JSON config shape (`{ host, port }`).
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "host-webserver: config must be an object".to_string())?;
        let host = object
            .get("host")
            .and_then(serde_json::Value::as_str)
            .and_then(Host::parse)
            .ok_or_else(|| {
                "host-webserver: host must be either \"127.0.0.1\" or \"0.0.0.0\"".to_string()
            })?;
        let port = object
            .get("port")
            .and_then(serde_json::Value::as_u64)
            .filter(|port| *port <= u64::from(u16::MAX))
            .map(|port| port as u16)
            .ok_or_else(|| {
                "host-webserver: port must be a non-negative integer no greater than 65535"
                    .to_string()
            })?;
        Ok(Self { host, port })
    }

    fn from_arcvalue(value: &ArcValue) -> Result<Self, String> {
        if let Some(config) = downcast::<Config>(value).cloned() {
            return Ok(config);
        }
        if let Some(raw) = downcast::<serde_json::Value>(value) {
            return Self::from_value(raw);
        }
        Err("host-webserver: config is not an object".to_string())
    }
}

/// Failure returned by a route/fallback/upgrade handler.
#[derive(Debug, Clone)]
pub struct WebHandlerError {
    pub message: String,
}

impl WebHandlerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WebHandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WebHandlerError {}

impl From<&str> for WebHandlerError {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for WebHandlerError {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// An incoming HTTP request (hyper request body, before it is read).
pub type WebRequest = Request<Incoming>;

/// The response a route or fallback handler returns.
pub type WebResponse = Response<Body>;

/// Route/fallback handler. Returning `Err` or panicking is contained by the
/// webserver as a 400 response (the last-resort guard in the TS source).
pub type WebRouteHandler = Arc<
    dyn Fn(WebRequest) -> BoxFuture<'static, Result<WebResponse, WebHandlerError>> + Send + Sync,
>;

/// An index.html transform applied by the fallback owner.
pub type WebIndexTap = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The upgraded socket handed to an upgrade route handler. Wrapped in the tokio
/// I/O adapter so handlers can use the tokio `AsyncRead`/`AsyncWrite` traits.
pub type WebUpgraded = TokioIo<Upgraded>;

/// Upgrade route handler. Returning `Err` or panicking destroys the socket.
pub type WebUpgradeHandler = Arc<
    dyn Fn(WebRequest, WebUpgraded) -> BoxFuture<'static, Result<(), WebHandlerError>>
        + Send
        + Sync,
>;

/// Synchronous route disposer (the TS register methods return `() => void`).
pub type RouteDisposer = Arc<dyn Fn() + Send + Sync>;

/// The browser HTTP carrier service.
pub struct WebServer {
    config: Config,
    exact: Arc<Mutex<HashMap<String, WebRoute>>>,
    prefixes: Arc<Mutex<HashMap<String, WebRoute>>>,
    upgrades: Arc<Mutex<HashMap<String, WebUpgradeRoute>>>,
    fallback: Arc<Mutex<Option<WebRouteHandler>>>,
    index_taps: Arc<Mutex<Vec<WebIndexTap>>>,
    bound_addr: Mutex<Option<SocketAddr>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    upgrade_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    stopping: AtomicBool,
    logger: cordis::Logger,
}

impl Service for WebServer {
    fn service_name(&self) -> &'static str {
        SERVICE_NAME
    }
}

impl WebServer {
    /// Construct an unregistered server (test hook); `install` binds, registers
    /// the `webServer` service, and attaches the teardown effect.
    pub fn new(ctx: &Context, config: Config) -> Arc<Self> {
        Arc::new(Self {
            config,
            exact: Arc::new(Mutex::new(HashMap::new())),
            prefixes: Arc::new(Mutex::new(HashMap::new())),
            upgrades: Arc::new(Mutex::new(HashMap::new())),
            fallback: Arc::new(Mutex::new(None)),
            index_taps: Arc::new(Mutex::new(Vec::new())),
            bound_addr: Mutex::new(None),
            accept_task: Mutex::new(None),
            connections: Arc::new(Mutex::new(Vec::new())),
            upgrade_tasks: Arc::new(Mutex::new(Vec::new())),
            stopping: AtomicBool::new(false),
            logger: ctx.named_logger(Some(NAME)),
        })
    }

    /// Bind the configured address, register the service, and attach teardown.
    /// A bind failure is returned (the loader fiber reports it fail-loud).
    pub async fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let server = Self::new(ctx, config);
        let unregister = ctx.register_service(server.clone());
        if let Err(error) = server.listen().await {
            unregister().await;
            return Err(error);
        }
        let weak = Arc::downgrade(&server);
        let _ = ctx.effect(
            "webServer.listen",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(server) = weak.upgrade() {
                            server.shutdown().await;
                        }
                    })
                }))
            }),
        );
        Ok(server)
    }

    /// The listening port (the OS-assigned value when `config.port` is 0).
    pub fn port(&self) -> u16 {
        self.bound_addr
            .lock()
            .map(|address| address.port())
            .expect("webServer: not listening (bind the service first)")
    }

    /// The address reported by the bound listener, including an OS-selected
    /// port when configuration requested port zero.
    pub fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
            .lock()
            .expect("webServer: not listening (bind the service first)")
    }

    /// The configured bind host literal.
    pub fn host(&self) -> &'static str {
        self.config.host.as_str()
    }

    /// Register a named route. Duplicate `(kind, path)` throws (route patterns
    /// are a composition-level contract).
    pub fn register(self: &Arc<Self>, route: WebRoute) -> RouteDisposer {
        let table = match route.kind {
            WebRouteKind::Exact => self.exact.clone(),
            WebRouteKind::Prefix => self.prefixes.clone(),
        };
        let mut routes = table.lock();
        if routes.contains_key(&route.path) {
            panic!(
                "webserver: duplicate {} route {:?}",
                route_kind_name(route.kind),
                route.path
            );
        }
        let path = route.path.clone();
        routes.insert(path.clone(), route);
        drop(routes);
        let weak = Arc::downgrade(self);
        Arc::new(move || {
            if weak.upgrade().is_some() {
                let _ = table.lock().remove(&path);
            }
        })
    }

    /// Register an exact-path HTTP upgrade route. Duplicate paths throw.
    pub fn register_upgrade(self: &Arc<Self>, route: WebUpgradeRoute) -> RouteDisposer {
        let mut routes = self.upgrades.lock();
        if routes.contains_key(&route.path) {
            panic!("webserver: duplicate upgrade route {:?}", route.path);
        }
        let path = route.path.clone();
        routes.insert(path.clone(), route);
        let weak = Arc::downgrade(self);
        Arc::new(move || {
            if let Some(server) = weak.upgrade() {
                let _ = server.upgrades.lock().remove(&path);
            }
        })
    }

    /// Claim the fallback seat. One owner only.
    pub fn register_fallback(self: &Arc<Self>, handler: WebRouteHandler) -> RouteDisposer {
        let mut fallback = self.fallback.lock();
        if fallback.is_some() {
            panic!("webserver: fallback already registered");
        }
        *fallback = Some(handler);
        let weak = Arc::downgrade(self);
        Arc::new(move || {
            if let Some(server) = weak.upgrade() {
                *server.fallback.lock() = None;
            }
        })
    }

    /// Register an index.html transform, applied in registration order.
    pub fn tap_index(
        self: &Arc<Self>,
        transform: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> RouteDisposer {
        let mut taps = self.index_taps.lock();
        taps.push(transform.clone());
        let weak = Arc::downgrade(self);
        Arc::new(move || {
            if let Some(server) = weak.upgrade() {
                let mut taps = server.index_taps.lock();
                if let Some(at) = taps.iter().position(|tap| Arc::ptr_eq(tap, &transform)) {
                    taps.remove(at);
                }
            }
        })
    }

    /// Run an index.html body through the registered taps in order.
    pub fn apply_index_taps(&self, html: &str) -> String {
        let taps: Vec<WebIndexTap> = self.index_taps.lock().clone();
        let mut out = html.to_string();
        for tap in taps {
            out = tap(&out);
        }
        out
    }

    async fn listen(self: &Arc<Self>) -> Result<u16, String> {
        if self.bound_addr.lock().is_some() {
            return Err("webServer: already listening".to_string());
        }
        let address = (self.config.host.as_str(), self.config.port);
        let listener = TcpListener::bind(address).await.map_err(|error| {
            format!(
                "webServer: failed to listen on {}:{}: {error}",
                address.0, address.1
            )
        })?;
        let bound_addr = listener
            .local_addr()
            .map_err(|error| format!("webServer: failed to read local address: {error}"))?;
        *self.bound_addr.lock() = Some(bound_addr);

        let weak = Arc::downgrade(self);
        let accept = tokio::spawn(async move {
            loop {
                if weak
                    .upgrade()
                    .map(|server| server.stopping.load(Ordering::SeqCst))
                    .unwrap_or(true)
                {
                    break;
                }
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let Some(server) = weak.upgrade() else {
                            break;
                        };
                        let io = TokioIo::new(stream);
                        let weak_conn = weak.clone();
                        let service = service_fn(move |request| {
                            let weak = weak_conn.clone();
                            async move {
                                let Some(server) = weak.upgrade() else {
                                    return Ok::<_, Infallible>(
                                        Response::builder()
                                            .status(StatusCode::SERVICE_UNAVAILABLE)
                                            .body(Body::empty())
                                            .expect("static response"),
                                    );
                                };
                                Ok::<_, Infallible>(server.dispatch(request).await)
                            }
                        });
                        let weak_track = weak.clone();
                        let connection = tokio::spawn(async move {
                            if let Err(error) = http1::Builder::new()
                                .serve_connection(io, service)
                                .with_upgrades()
                                .await
                                && let Some(server) = weak_track.upgrade()
                            {
                                server.logger.warn(vec![arc(format!(
                                    "webServer connection error: {error}"
                                ))]);
                            }
                        });
                        let mut connections = server.connections.lock();
                        connections.retain(|task| !task.is_finished());
                        connections.push(connection);
                    }
                    Err(error) => {
                        let stopping = weak
                            .upgrade()
                            .map(|server| server.stopping.load(Ordering::SeqCst))
                            .unwrap_or(true);
                        if !stopping && let Some(server) = weak.upgrade() {
                            server
                                .logger
                                .warn(vec![arc(format!("webServer accept error: {error}"))]);
                        }
                        break;
                    }
                }
            }
        });
        *self.accept_task.lock() = Some(accept);
        Ok(bound_addr.port())
    }

    async fn dispatch(self: &Arc<Self>, request: WebRequest) -> WebResponse {
        if is_upgrade_request(&request) {
            return self.dispatch_upgrade(request).await;
        }
        self.dispatch_http(request).await
    }

    async fn dispatch_http(&self, request: WebRequest) -> WebResponse {
        let path = request.uri().path().to_string();
        let handler = self
            .match_route(&path)
            .map(|route| route.handler.clone())
            .or_else(|| self.fallback.lock().clone());
        let Some(handler) = handler else {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .expect("static response");
        };
        match AssertUnwindSafe(handler(request)).catch_unwind().await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                self.logger
                    .warn(vec![arc(format!("webServer request failed: {error}"))]);
                self.bad_request()
            }
            Err(payload) => {
                let message = panic_payload_message(payload.as_ref());
                self.logger
                    .warn(vec![arc(format!("webServer handler panicked: {message}"))]);
                self.bad_request()
            }
        }
    }

    async fn dispatch_upgrade(self: &Arc<Self>, mut request: WebRequest) -> WebResponse {
        let path = request.uri().path().to_string();
        let route = self.upgrades.lock().get(&path).cloned();
        let Some(route) = route else {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("webserver: no upgrade route for this path"))
                .expect("static response");
        };
        let on_upgrade = hyper::upgrade::on(&mut request);
        let handler = route.handler.clone();
        let logger = self.logger.clone();
        let upgrade_value = request.headers().get(header::UPGRADE).cloned();
        let websocket_accept = request
            .headers()
            .get("sec-websocket-key")
            .and_then(|value| value.to_str().ok())
            .map(|key| tungstenite::handshake::derive_accept_key(key.as_bytes()));
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    let upgraded = TokioIo::new(upgraded);
                    match AssertUnwindSafe(handler(request, upgraded))
                        .catch_unwind()
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            logger.warn(vec![arc(format!(
                                "webServer upgrade handler failed: {error}"
                            ))]);
                        }
                        Err(payload) => {
                            let message = panic_payload_message(payload.as_ref());
                            logger.warn(vec![arc(format!(
                                "webServer upgrade handler panicked: {message}"
                            ))]);
                        }
                    }
                }
                Err(error) => {
                    logger.warn(vec![arc(format!(
                        "webServer upgrade handshake failed: {error}"
                    ))]);
                }
            }
            if let Some(server) = weak.upgrade() {
                let mut tasks = server.upgrade_tasks.lock();
                tasks.retain(|task| !task.is_finished());
            }
        });
        let mut tasks = self.upgrade_tasks.lock();
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);

        let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
        builder = builder.header(header::CONNECTION, "upgrade");
        if let Some(value) = upgrade_value {
            builder = builder.header(header::UPGRADE, value);
        }
        if let Some(accept) = websocket_accept {
            builder = builder.header("sec-websocket-accept", accept);
        }
        builder.body(Body::empty()).expect("static response")
    }

    /// Longest-prefix-wins over the prefix table after an exact-table miss.
    fn match_route(&self, pathname: &str) -> Option<WebRoute> {
        if let Some(route) = self.exact.lock().get(pathname).cloned() {
            return Some(route);
        }
        let mut best: Option<WebRoute> = None;
        for (prefix, route) in self.prefixes.lock().iter() {
            if pathname != prefix && !pathname.starts_with(&format!("{prefix}/")) {
                continue;
            }
            if best
                .as_ref()
                .map(|best| prefix.len() > best.path.len())
                .unwrap_or(true)
            {
                best = Some(route.clone());
            }
        }
        best
    }

    /// Synchronously request stop without waiting for task cancellation. This
    /// is the only safe work performed by owners that are dropped without an
    /// explicit async shutdown.
    pub fn request_shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(task) = self.accept_task.lock().as_ref() {
            task.abort();
        }
        for task in self.connections.lock().iter() {
            task.abort();
        }
        for task in self.upgrade_tasks.lock().iter() {
            task.abort();
        }
    }

    /// Stop accepting, abort active transports, and await every owned server
    /// task. Repeated calls are idempotent.
    pub async fn shutdown(&self) {
        self.request_shutdown();
        let accept_task = { self.accept_task.lock().take() };
        if let Some(task) = accept_task {
            let _ = task.await;
        }
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();
        tasks.extend(self.connections.lock().drain(..));
        tasks.extend(self.upgrade_tasks.lock().drain(..));
        for task in &tasks {
            task.abort();
        }
        futures::future::join_all(tasks.into_iter().map(|task| async move {
            let _ = task.await;
        }))
        .await;
        *self.bound_addr.lock() = None;
    }

    fn bad_request(&self) -> WebResponse {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .expect("static response")
    }
}

fn route_kind_name(kind: WebRouteKind) -> &'static str {
    match kind {
        WebRouteKind::Exact => "exact",
        WebRouteKind::Prefix => "prefix",
    }
}

fn is_upgrade_request(request: &WebRequest) -> bool {
    if !request.headers().contains_key(header::UPGRADE) {
        return false;
    }
    request
        .headers()
        .get(header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// The Cordis plugin form of the webserver.
pub struct WebServerPlugin;

#[async_trait]
impl Plugin for WebServerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config =
            Config::from_arcvalue(&config).map_err(|error| PluginError::new(arc(error)))?;
        WebServer::install(ctx, config)
            .await
            .map(|_| ())
            .map_err(|error| PluginError::new(arc(error)))
    }
}
