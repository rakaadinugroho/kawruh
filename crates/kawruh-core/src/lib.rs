//! Shared core of Kawruh: document model, markdown conversion, persistence
//! and the sync protocol. Used by the desktop app and the server.

pub mod markdown;
pub mod protocol;
pub mod schema;
pub mod store;
pub mod sync_client;
pub mod workspace;

pub use yrs;
