//! OpenShell Provider gRPC + CLI wrappers.
//!
//! This module is the SOLE owner of the OpenShell Provider client.
//! All Provider RPCs and `openshell provider` / `openshell sandbox provider`
//! CLI invocations go through here (see ARCHITECTURE.md).

#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::process::Stdio;

use thiserror::Error;
#[allow(unused_imports)]
use tokio::process::Command;

/// All provider operation errors. Each is FAIL FAST — never swallowed.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider gateway unreachable: {0:#}")]
    GatewayUnreachable(miette::ErrReport),
    #[error("openshell gRPC: {0:#}")]
    Grpc(String),
    #[error("openshell CLI {cmd:?} exited {status}: {stderr}")]
    Cli {
        cmd: String,
        status: i32,
        stderr: String,
    },
    #[error("provider \"{0}\" not found")]
    NotFound(String),
    #[error("providers_v2_enabled is not on; run `right up` to enable")]
    V2NotEnabled,
    #[error("invalid provider: {0}")]
    Invalid(String),
}

/// Input for create/update.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub name: String,
    pub type_: String, // raw slug
    pub credentials: HashMap<String, String>,
    pub config: HashMap<String, String>,
}

/// Output of get/list. Credentials field is INTENTIONALLY OMITTED — the
/// gateway returns them, but Right never reads or stores them on host.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub type_: String,
    pub config: HashMap<String, String>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Profile entry surfaced by `/provider-types` to the dashboard.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    pub type_slug: String,
    pub env_var: String,
    pub display_name: String,
    pub category: ProviderCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCategory {
    Inference,
    Agent,
    SourceControl,
    Messaging,
    Other,
}
