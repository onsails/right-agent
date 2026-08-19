//! Egress policy for an Agent Sandbox.
//!
//! The typed `Egress` value replaces the OpenShell-era `policy.yaml` codegen:
//! the policy is a value applied through the SDK at sandbox create. Egress is
//! create-time only — the SDK's `modify()` has no network-policy field, so
//! changing an agent's egress mode requires a sandbox recreate.
//!
//! Both modes keep the `host` destination group open: the guest reaches the
//! MCP aggregator through `host.microsandbox.internal`, and `from_profiles`
//! already adds exactly one narrow DNS rule for the gateway resolver.

use microsandbox::{NetworkPolicy, NetworkProfile};

use crate::error::SandboxError;

/// Egress mode for an Agent Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Egress {
    /// Whole public internet plus the host group. Right's default floor.
    #[default]
    Permissive,

    /// Deny-by-default public egress: only the host group plus an explicit
    /// domain-suffix allowlist. Shipped but documented experimental until
    /// exercised; the allowlist entries are domain suffixes (`"example.com"`
    /// also covers `*.example.com`).
    Restrictive {
        /// Domain suffixes the guest may reach over public egress.
        allow: Vec<String>,
    },
}

impl Egress {
    /// Validate the mode without constructing anything SDK-side worth
    /// keeping. Suitable for config-load time checks.
    pub fn validate(&self) -> Result<(), SandboxError> {
        self.to_policy().map(|_| ())
    }

    /// Translate to the SDK's deny-by-default network policy.
    pub(crate) fn to_policy(&self) -> Result<NetworkPolicy, SandboxError> {
        match self {
            Self::Permissive => Ok(NetworkPolicy::from_profiles([
                NetworkProfile::Public,
                NetworkProfile::Host,
            ])),
            Self::Restrictive { allow } => {
                let mut policy = NetworkPolicy::from_profiles([NetworkProfile::Host]);
                for entry in allow {
                    policy = policy.allow_domain_suffix(entry).map_err(|err| {
                        SandboxError::InvalidSpec {
                            field: "egress.allow",
                            reason: format!("invalid domain suffix {entry:?}: {err}"),
                        }
                    })?;
                }
                Ok(policy)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use microsandbox::NetworkAction;

    use super::*;

    /// Debug rendering of a rule's destination; the SDK's destination types
    /// are not re-exported, so tests match on their Debug shape.
    fn destinations(policy: &NetworkPolicy) -> Vec<String> {
        policy
            .rules
            .iter()
            .map(|rule| format!("{:?}", rule.destination))
            .collect()
    }

    /// The DNS rule `from_profiles` adds: Host group, UDP+TCP, port 53 only.
    fn is_dns_rule(rule: &microsandbox::NetworkRule) -> bool {
        format!("{:?}", rule.destination) == "Group(Host)"
            && rule
                .ports
                .iter()
                .all(|range| range.start == 53 && range.end == 53)
            && !rule.ports.is_empty()
    }

    /// The unrestricted host-group rule (no port filter).
    fn is_host_group_rule(rule: &microsandbox::NetworkRule) -> bool {
        format!("{:?}", rule.destination) == "Group(Host)" && rule.ports.is_empty()
    }

    #[test]
    fn permissive_allows_public_and_host_with_dns() {
        let policy = Egress::Permissive.to_policy().expect("permissive policy");

        assert_eq!(policy.default_egress, NetworkAction::Deny);
        assert_eq!(policy.default_ingress, NetworkAction::Allow);
        let destinations = destinations(&policy);
        assert_eq!(
            destinations,
            ["Group(Host)", "Group(Public)", "Group(Host)"],
            "permissive = one narrow DNS rule + public + host: {destinations:?}"
        );
        assert!(is_dns_rule(&policy.rules[0]), "first rule is DNS: {:?}", policy.rules[0]);
        assert!(
            policy
                .rules
                .iter()
                .all(|rule| rule.action == NetworkAction::Allow),
            "every permissive rule is an allow rule"
        );
    }

    #[test]
    fn restrictive_without_allowlist_reaches_only_the_host() {
        let policy = Egress::Restrictive { allow: Vec::new() }
            .to_policy()
            .expect("restrictive policy");

        assert_eq!(policy.default_egress, NetworkAction::Deny);
        assert_eq!(policy.rules.len(), 2, "DNS + host only: {:?}", policy.rules);
        assert!(is_dns_rule(&policy.rules[0]));
        assert!(is_host_group_rule(&policy.rules[1]));
        assert!(
            !destinations(&policy).iter().any(|d| d.contains("Public")),
            "restrictive must never include the public group"
        );
    }

    #[test]
    fn restrictive_allowlist_prepends_domain_suffix_rules() {
        let policy = Egress::Restrictive {
            allow: vec!["example.com".to_owned(), "crates.io".to_owned()],
        }
        .to_policy()
        .expect("allowlisted policy");

        let destinations = destinations(&policy);
        assert_eq!(
            destinations.len(),
            4,
            "two allowlist rules + DNS + host: {destinations:?}"
        );
        // Both allowlist rules match before DNS and the host group. Their
        // relative order carries no semantics (all are allows to distinct
        // domains), so assert the set, not the SDK's prepend order.
        let allowlist = &destinations[..2];
        for domain in ["example.com", "crates.io"] {
            assert!(
                allowlist
                    .iter()
                    .any(|d| d.contains("DomainSuffix") && d.contains(domain)),
                "missing allow rule for {domain}: {allowlist:?}"
            );
        }
        assert!(is_dns_rule(&policy.rules[2]));
        assert!(is_host_group_rule(&policy.rules[3]));
    }

    #[test]
    fn invalid_allowlist_entries_are_rejected_with_the_entry_named() {
        let err = Egress::Restrictive {
            allow: vec!["ok.com".to_owned(), "not a domain!".to_owned()],
        }
        .validate()
        .expect_err("an invalid suffix must fail validation");

        match err {
            SandboxError::InvalidSpec { field, reason } => {
                assert_eq!(field, "egress.allow");
                assert!(
                    reason.contains("not a domain!"),
                    "the offending entry must be named: {reason}"
                );
            }
            other => panic!("expected InvalidSpec, got {other:?}"),
        }
    }
}
