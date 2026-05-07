pub mod circuit;
pub mod classify;
pub mod error;
pub mod guard;
pub mod hindsight;
pub mod prefetch;
pub mod resilient;
pub mod retain_queue;
pub mod status;

pub use classify::ErrorKind;
pub use error::MemoryError;
pub use resilient::{ResilientError, ResilientHindsight};
pub use status::MemoryStatus;

pub mod alert_types {
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const CLIENT_FLOOD: &str = "client_flood";
}
