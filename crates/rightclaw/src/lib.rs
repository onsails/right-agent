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
pub mod sandbox_exec;
pub mod runtime;
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
