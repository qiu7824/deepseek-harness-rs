//! SPA dist server over the webserver fallback seat. Rust port of
//! `packages/host/frontend-static`.

pub mod index;
pub mod invariant;

pub use index::{
    Config, FrontendStaticPlugin, NAME, apply, decode_uri_path, resolve_static_target,
    serve_static,
};
