pub mod agent;
pub mod codegen;
pub mod config;
pub mod cron_spec;
pub mod doctor;
pub mod error;
pub mod init;
pub mod mcp;
pub mod memory;
pub mod openshell;
pub mod platform_store;
#[cfg(unix)]
pub mod process_group;
pub mod runtime;
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
pub mod tunnel;

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
