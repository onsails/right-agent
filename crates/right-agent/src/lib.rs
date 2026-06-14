#![warn(unreachable_pub)]

pub mod agent;
pub mod async_runs;
pub mod cron_skill_link;
pub mod cron_spec;
pub mod doctor;
pub mod identity_mirror;
pub mod init;
pub mod learned_skills;
pub mod rebootstrap;
pub mod runtime;
pub(crate) mod tunnel;
pub mod usage;

/// Shared crate-internal helpers for tests in this crate. Kept in `lib.rs`
/// so any `#[cfg(test)]` module can reach it via the crate path.
#[cfg(test)]
pub(crate) mod test_support {
    /// Install ring as the rustls process-level crypto provider. Idempotent —
    /// safe to call from multiple tests in the same binary.
    pub(crate) fn setup_crypto() {
        // install_default returns Err(existing provider Arc) when already
        // installed by another test in the same binary — that's not a failure.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
