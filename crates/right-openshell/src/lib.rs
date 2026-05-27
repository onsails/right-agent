//! OpenShell gRPC, CLI wrappers, sandbox exec, and live-test support.

#![warn(unreachable_pub)]

pub mod openshell;
pub mod providers;
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
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
