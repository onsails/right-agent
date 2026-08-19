//! Agent Sandbox backend.
//!
//! Owns every microVM interaction: runtime install, sandbox lifecycle,
//! streaming execution, guest filesystem, egress policy, and Provider secret
//! bindings. This is the only crate in the workspace that depends on the
//! microsandbox SDK; nothing in the public API exposes a raw SDK type.
//!
//! The entry points:
//!
//! - [`ensure_runtime_installed`] + [`diagnose_host`] — bot startup preflight.
//! - [`sandbox_name`]/[`fit_sandbox_name`] — deterministic agent → sandbox
//!   naming within the SDK's 128-byte name space.
//! - [`SandboxSpec`] — everything a create needs, with Right's defaults.
//! - [`SandboxHandle::create_or_attach`]/[`SandboxHandle::attach`] — the
//!   unified handle replacing `resolved_sandbox` + `ssh_config_path`.
//! - [`SandboxError`]/[`SandboxCause`] — the error taxonomy the supervisor
//!   and Telegram UX match on.

mod egress;
mod error;
mod exec;
mod fs;
mod handle;
mod names;
mod phase;
mod resources;
mod runtime;
mod secrets;
mod spec;

pub use error::{SandboxCause, SandboxDiagnosis, SandboxError, SdkError};
pub use exec::{
    ChunkedStdin, ExecEvent, ExecOutcome, ExecRequest, ExecStream, PROTOCOL_FRAME_MAX_BYTES,
    STDIN_CHUNK_BYTES, Stdin,
};
pub use fs::{FsEntryInfo, FsEntryKind};
pub use handle::{DEFAULT_READY_TIMEOUT, SandboxHandle, SandboxHealthReport};
pub use names::{MAX_SANDBOX_NAME_BYTES, fit_sandbox_name, sandbox_name};
pub use phase::SandboxPhase;
pub use resources::{DEFAULT_CPUS, DEFAULT_MEMORY_MIB, DEFAULT_WRITABLE_LAYER_MIB, Resources};
pub use runtime::{diagnose_host, ensure_runtime_installed};
pub use secrets::{
    RotationDisposition, SecretBinding, SecretRotation, TLS_BYPASS_HOSTS, default_placeholder,
    tls_bypass_list,
};
pub use spec::SandboxSpec;

/// The exact microsandbox SDK version this crate is built against.
///
/// The SDK pins its own `msb` runtime and `libkrunfw`, so Right maintains no
/// separate version floor. Bumping this constant is a deliberate act that
/// requires the `ci_msb_` contract suite to pass first.
pub const PINNED_SDK_VERSION: &str = "0.6.10";
