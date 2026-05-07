//! Stable platform-foundation modules for `right`.
//!
//! Bottom-of-stack crate. Other crates depend on it; it depends on
//! nothing in this workspace. Modules here change rarely; incremental
//! edits to `right-codegen`, `right-memory`, `right-mcp`, or
//! `right-cc` should not invalidate this crate's build cache.

pub mod agent_types;
pub mod config;
pub mod error;
pub mod openshell;
/// Generated protobuf types for the OpenShell gRPC API.
#[allow(clippy::large_enum_variant)]
pub mod openshell_proto {
    pub mod openshell {
        pub mod v1 {
            tonic::include_proto!("openshell.v1");
        }
        pub mod datamodel {
            pub mod v1 {
                tonic::include_proto!("openshell.datamodel.v1");
            }
        }
        pub mod sandbox {
            pub mod v1 {
                tonic::include_proto!("openshell.sandbox.v1");
            }
        }
    }
}
#[cfg(unix)]
pub mod process_group;
pub mod platform_store;
pub mod sandbox_exec;
pub mod stt;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
pub mod time_constants;
pub mod ui;
