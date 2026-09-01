//! Web route-registration plugin: an HTTP server plus the `webServer` service
//! (HTTP and upgrade route registries, index transform taps, and the single
//! fallback seat). Rust port of `packages/host/webserver`.
//!
//! The crate knows no harness concepts and serves no files; the composing
//! application's frontend plugin owns dist serving through the fallback hook.

pub mod index;
pub mod invariant;

pub use index::{
    Config, Host, RouteDisposer, WebHandlerError, WebIndexTap, WebRequest, WebResponse, WebRoute,
    WebRouteHandler, WebRouteKind, WebServer, WebServerPlugin, WebUpgradeHandler, WebUpgradeRoute,
    WebUpgraded, accept_websocket,
};
