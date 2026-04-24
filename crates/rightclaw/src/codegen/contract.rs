//! Codegen output contract.
//!
//! Every file written by codegen belongs to exactly one [`CodegenKind`]. The
//! helpers in this module are the only sanctioned writers for codegen files.
//! Direct `std::fs::write` inside `codegen/*` modules is a review-blocking
//! defect after this module lands.
//!
//! See `docs/superpowers/specs/2026-04-24-upgrade-migration-model-design.md`.

use std::path::{Path, PathBuf};

/// Category of a codegen output. Drives how changes propagate to running agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenKind {
    /// Unconditional overwrite on every bot start.
    Regenerated(HotReload),
    /// Read existing, merge codegen fields in, write back. Preserves unknown fields.
    MergedRMW,
    /// Created by init with an initial payload. Never touched by codegen again.
    AgentOwned,
}

/// How a `Regenerated` change reaches a running sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotReload {
    /// Takes effect on next CC invocation. No sandbox RPC needed.
    BotRestart,
    /// Applied via `openshell policy set --wait` after write. Network-only.
    SandboxPolicyApply,
    /// Boot-time-only (landlock, filesystem). Requires sandbox migration.
    SandboxRecreate,
}

/// An entry in the codegen registry.
#[derive(Debug, Clone)]
pub struct CodegenFile {
    pub kind: CodegenKind,
    pub path: PathBuf,
}
