pub mod agent;
pub mod codegen;
pub use right_core::config;
pub mod cron_spec;
pub mod doctor;
pub use right_core::error;
pub mod init;
pub mod mcp;
pub mod memory;
pub mod openshell;
pub mod platform_store;
#[cfg(unix)]
pub use right_core::process_group;
pub mod rebootstrap;
pub mod runtime;
pub mod sandbox_exec;
pub mod stt;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
pub mod tunnel;
pub use right_core::ui;
pub mod usage;

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
