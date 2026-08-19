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
//! - [`agent_sandbox_spec`] — the one create-time spec every Right agent's
//!   sandbox is built from, shared by the bot and the CLI.
//! - [`SandboxSpec`] — everything a create needs, with Right's defaults.
//! - [`SandboxHandle::create_or_attach`]/[`SandboxHandle::attach`] — the
//!   unified handle replacing `resolved_sandbox` + `ssh_config_path`.
//! - [`SandboxError`]/[`SandboxCause`] — the error taxonomy the supervisor
//!   and Telegram UX match on.

mod agent;
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

pub use agent::{DEFAULT_SANDBOX_IMAGE, GUEST_HOME, GUEST_USER, agent_sandbox_spec, egress_for};
pub use egress::Egress;
pub use error::{SandboxCause, SandboxDiagnosis, SandboxError, SdkError};
pub use exec::{
    ChunkedStdin, ExecEvent, ExecOutcome, ExecRequest, ExecStream, PROTOCOL_FRAME_MAX_BYTES,
    STDIN_CHUNK_BYTES, Stdin,
};
pub use fs::{FsEntryInfo, FsEntryKind};
pub use handle::{DEFAULT_READY_TIMEOUT, SandboxHandle, SandboxHealthReport};
pub use names::{MAX_SANDBOX_NAME_BYTES, fit_sandbox_name, resolve_sandbox_name, sandbox_name};
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

#[cfg(test)]
mod api_surface_tests {
    //! Compile-time guard that the types behind public fields and methods are
    //! themselves nameable outside the crate. A `pub` type in a private module
    //! that is never re-exported compiles silently (no rustc warning) while
    //! being unusable downstream — this test names each such type so the
    //! omission becomes a compile error here, not a confusing failure in
    //! stage 4.

    #[test]
    fn public_field_types_are_exported() {
        // `SandboxSpec::egress` is a public field; `Egress` must be nameable.
        fn _assert(_: Option<crate::Egress>) {}
        // `SandboxSpec::resources` and `secrets` element types.
        fn _assert2(_: Option<crate::Resources>, _: Option<crate::SecretBinding>) {}
        let _ = (_assert, _assert2);
    }
}
