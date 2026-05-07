#![warn(unreachable_pub)]

pub(crate) mod circuit;
pub(crate) mod classify;
pub(crate) mod error;
pub mod hindsight;
pub mod prefetch;
pub mod resilient;
pub mod retain_queue;
pub(crate) mod status;

pub use classify::ErrorKind;
pub use error::MemoryError;
pub use resilient::{ResilientError, ResilientHindsight};
pub use status::MemoryStatus;

pub mod alert_types {
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const CLIENT_FLOOD: &str = "client_flood";
}
