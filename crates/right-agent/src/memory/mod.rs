//! Re-export shim. Real definitions live in the `right-memory` crate.
//! `open_db` / `open_connection` are sourced from `right-db`. Removed in Stage F.

pub use right_db::{open_connection, open_db};
pub use right_memory::*;
