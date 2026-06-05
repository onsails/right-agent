//! OpenShell gRPC, CLI wrappers, sandbox exec, and live-test support.

#![warn(unreachable_pub)]

pub mod diagnosis;
pub mod managed_profiles;
pub mod openshell;
pub mod providers;
// Generated tonic/prost code: `large_enum_variant` is inherent to the proto
// shape, and `doc_lazy_continuation` fires on upstream proto comments whose
// list formatting we don't control.
#[allow(clippy::large_enum_variant, clippy::doc_lazy_continuation)]
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
pub mod preflight;
pub mod sandbox_exec;
#[cfg(unix)]
pub mod test_cleanup;
#[cfg(test)]
mod test_mock_server;
#[cfg(all(unix, any(test, feature = "test-support")))]
pub mod test_support;
