pub mod circuit;
pub mod classify;
pub mod error;
pub mod guard;
pub mod hindsight;
pub mod prefetch;
pub mod resilient;
pub mod retain_queue;
pub mod status;
pub mod store;

pub use right_db::{open_connection, open_db};

pub use classify::ErrorKind;
pub use error::MemoryError;
pub use resilient::{ResilientError, ResilientHindsight};
pub use status::MemoryStatus;

/// Dedup keys for rows in the `memory_alerts` table.
///
/// These strings appear in SQL queries across `memory_alerts.rs`, `doctor.rs`,
/// and their tests. Keeping them here prevents silent drift that would break
/// dedup (a typo makes the same alert fire twice).
pub mod alert_types {
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const CLIENT_FLOOD: &str = "client_flood";
}
