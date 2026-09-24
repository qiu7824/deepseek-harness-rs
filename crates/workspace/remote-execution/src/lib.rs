//! Local-agent routing into a separately authorized remote execution world.
pub mod helper;
pub mod protocol;
pub mod routing;
pub mod transport;
mod worker_spawn;
pub use protocol::{Connection, Handshake, PROTOCOL_VERSION};
pub use routing::install_routes;
pub use transport::RemoteRuntime;
