//! Resource sizing for an Agent Sandbox.
//!
//! The SDK's own defaults are 1 vCPU / 512 MiB / a 4 GiB writable layer —
//! far too small for a Claude Code agent, and the 4 GiB default layer boots
//! measurably slower than an explicit 16 GiB one (stage-1 verdict 7). Right
//! therefore always sets its own defaults explicitly. Memory is a *limit*,
//! not a reservation: the host only commits what the guest touches.

use crate::error::SandboxError;

/// Default vCPU count per Agent Sandbox.
pub const DEFAULT_CPUS: u8 = 2;

/// Default memory limit per Agent Sandbox, in MiB (8 GiB).
pub const DEFAULT_MEMORY_MIB: u32 = 8 * 1024;

/// Default writable-layer size per Agent Sandbox, in MiB (16 GiB).
pub const DEFAULT_WRITABLE_LAYER_MIB: u32 = 16 * 1024;

/// Per-agent resource sizing (`sandbox.resources` in the agent config).
///
/// The defaults are Right's, applied at create; every field is overridable
/// per agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resources {
    /// vCPU count.
    pub cpus: u8,

    /// Memory limit in MiB.
    pub memory_mib: u32,

    /// Writable-layer (guest rootfs upper) size in MiB.
    pub writable_layer_mib: u32,
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            cpus: DEFAULT_CPUS,
            memory_mib: DEFAULT_MEMORY_MIB,
            writable_layer_mib: DEFAULT_WRITABLE_LAYER_MIB,
        }
    }
}

impl Resources {
    /// Reject zero-sized resources before they reach the runtime.
    pub(crate) fn validate(&self) -> Result<(), SandboxError> {
        if self.cpus == 0 {
            return Err(SandboxError::InvalidSpec {
                field: "resources.cpus",
                reason: "must be at least 1".to_owned(),
            });
        }
        if self.memory_mib == 0 {
            return Err(SandboxError::InvalidSpec {
                field: "resources.memory_mib",
                reason: "must be at least 1".to_owned(),
            });
        }
        if self.writable_layer_mib == 0 {
            return Err(SandboxError::InvalidSpec {
                field: "resources.writable_layer_mib",
                reason: "must be at least 1".to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_rights_own_not_the_sdks() {
        let resources = Resources::default();
        assert_eq!(resources.cpus, 2);
        assert_eq!(resources.memory_mib, 8192);
        assert_eq!(resources.writable_layer_mib, 16384);
    }

    #[test]
    fn zero_sized_resources_are_rejected() {
        for resources in [
            Resources {
                cpus: 0,
                ..Resources::default()
            },
            Resources {
                memory_mib: 0,
                ..Resources::default()
            },
            Resources {
                writable_layer_mib: 0,
                ..Resources::default()
            },
        ] {
            assert!(resources.validate().is_err(), "{resources:?} must fail");
        }
    }

    #[test]
    fn default_resources_validate() {
        Resources::default().validate().expect("defaults are valid");
    }
}
