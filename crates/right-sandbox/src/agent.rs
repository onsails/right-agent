//! Right's Agent Sandbox conventions.
//!
//! The single definition of the create-time spec every agent's microVM is
//! built from. Both writers — the bot's sandbox supervisor (bring-up and
//! recovery) and the `right` CLI (`agent restore`) — reach
//! [`agent_sandbox_spec`], so a sandbox created by one is identical to one
//! created by the other. Egress and secret structure are create-time only, so
//! drift between the two would silently produce sandboxes with different
//! network reach and different bindings, failing only at turn time.
//!
//! Provider credentials are resolved by the caller: reading them needs the
//! provider store, which itself depends on this crate.

use right_agent_config::NetworkPolicy;

use crate::egress::Egress;
use crate::error::SandboxError;
use crate::resources::Resources;
use crate::secrets::SecretBinding;
use crate::spec::SandboxSpec;

/// Guest image every Agent Sandbox boots from.
///
/// A stock OCI image. Right does not run package-manager or installer
/// downloads during startup: restrictive egress is create-time state and
/// existing sandboxes cannot adopt newly allowlisted download hosts in place.
pub const DEFAULT_SANDBOX_IMAGE: &str = "node:22-slim";

/// Unprivileged guest user the agent's `claude` runs as.
///
/// Provisioning makes the `.platform` entry root-owned and read-only, but its
/// guest-owned `/sandbox` parent means this is not an authoritative root path.
pub const GUEST_USER: &str = "sandbox";

/// The agent's home inside the guest, and the working directory every exec
/// defaults to.
pub const GUEST_HOME: &str = "/sandbox";

/// Domain suffixes reachable under `network_policy: restrictive`.
///
/// Suffixes, not globs: `anthropic.com` also covers `*.anthropic.com`. The
/// host destination group is always open on top of this, which is how the
/// guest reaches the MCP aggregator.
const RESTRICTIVE_EGRESS_ALLOW: &[&str] = &[
    "anthropic.com",
    "claude.com",
    "claude.ai",
    "storage.googleapis.com",
];

/// Translate an agent's declared network policy into a typed egress value.
///
/// Egress is create-time only — the SDK cannot change network policy on a
/// running sandbox. Startup provisioning must therefore work without adding
/// public hosts here; otherwise existing restrictive sandboxes would require
/// recreation to start.
pub fn egress_for(network_policy: NetworkPolicy) -> Egress {
    match network_policy {
        NetworkPolicy::Permissive => Egress::Permissive,
        NetworkPolicy::Restrictive => Egress::Restrictive {
            allow: RESTRICTIVE_EGRESS_ALLOW
                .iter()
                .map(|domain| (*domain).to_owned())
                .collect(),
        },
    }
}

/// Build the create-time specification for an agent's sandbox.
/// `secrets` are the agent's resolved provider bindings. Credential values are
/// private/redacted and enter only the SDK's scoped resolver; durable sandbox
/// configuration receives source identities and placeholders.
pub fn agent_sandbox_spec(
    sandbox_name: &str,
    network_policy: NetworkPolicy,
    secrets: Vec<SecretBinding>,
) -> Result<SandboxSpec, SandboxError> {
    let mut spec = SandboxSpec::new(sandbox_name, DEFAULT_SANDBOX_IMAGE);
    spec.resources = Resources::default();
    spec.egress = egress_for(network_policy);
    spec.secrets = secrets;
    // Deliberately no create-time `user` or `workdir`. Neither exists in the
    // stock image: provisioning creates the unprivileged user and its home
    // after boot. Setting them here makes the guest init fail before the agent
    // relay comes up, which surfaces only as "sandbox process exited before
    // agent relay became available" — the boot failure the pilot migration hit.
    // The agent still runs unprivileged: that is a per-exec override, applied
    // once provisioning has created the user.
    spec.validate()?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock image has no `sandbox` user and no `/sandbox`; provisioning
    /// creates both after boot. Pinning them at create time makes guest init
    /// die before the agent relay is up, so the spec must leave them unset —
    /// this is a real boot failure a pilot migration hit, not a style choice.
    #[test]
    fn spec_does_not_pin_a_guest_user_the_image_lacks() {
        let spec = agent_sandbox_spec("right-finance", NetworkPolicy::Permissive, Vec::new())
            .expect("defaults are a valid spec");
        assert_eq!(spec.image, DEFAULT_SANDBOX_IMAGE);
        assert_eq!(
            spec.user, None,
            "the unprivileged user is a per-exec override, created by provisioning"
        );
        assert_eq!(
            spec.workdir, None,
            "{GUEST_HOME} does not exist until provisioning creates it"
        );
        assert!(matches!(spec.egress, Egress::Permissive));
    }

    #[test]
    fn restrictive_policy_allows_only_the_claude_domains() {
        let spec = agent_sandbox_spec("right-finance", NetworkPolicy::Restrictive, Vec::new())
            .expect("restrictive is a valid spec");
        let Egress::Restrictive { allow } = spec.egress else {
            panic!("restrictive policy must produce restrictive egress");
        };
        assert_eq!(allow, RESTRICTIVE_EGRESS_ALLOW);
        assert!(
            !allow.iter().any(|domain| domain.starts_with("*.")),
            "entries are domain suffixes, not globs: {allow:?}"
        );
    }

    #[test]
    fn restrictive_policy_does_not_open_package_download_hosts() {
        let Egress::Restrictive { allow } = egress_for(NetworkPolicy::Restrictive) else {
            panic!("restrictive policy must produce restrictive egress");
        };

        for host in ["deb.debian.org", "security.debian.org", "bun.sh"] {
            assert!(
                !allow.iter().any(|suffix| host.ends_with(suffix)),
                "startup provisioning must not depend on guest access to {host}: {allow:?}"
            );
        }
    }

    #[test]
    fn invalid_name_is_rejected_before_any_sdk_call() {
        let error = agent_sandbox_spec("", NetworkPolicy::Permissive, Vec::new())
            .expect_err("an empty name cannot be created");
        assert!(matches!(error, SandboxError::InvalidSpec { .. }));
    }
}
