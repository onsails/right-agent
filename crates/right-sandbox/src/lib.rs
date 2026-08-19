//! Agent Sandbox backend.
//!
//! Owns every microVM interaction: lifecycle, streaming execution, guest
//! filesystem, Egress Mode, and Provider secret bindings. This is the only
//! crate in the workspace that depends on the microsandbox SDK.
//!
//! Stage 1 of the migration establishes only the pinned dependency and the
//! live-microVM assumption probes in `tests/`. The API lands in stage 2.

/// The exact microsandbox SDK version this crate is built against.
///
/// The SDK pins its own `msb` runtime and `libkrunfw`, so Right maintains no
/// separate version floor. Bumping this constant is a deliberate act that
/// requires the `ci_msb_` contract suite to pass first.
pub const PINNED_SDK_VERSION: &str = "0.6.10";
