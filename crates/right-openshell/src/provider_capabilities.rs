//! Agent-facing provider capability records.
//!
//! Joins the live effective sandbox policy, the sandbox's injected placeholder
//! env vars, and each attached provider's profile into a description the agent
//! can read to learn which binary can spend which credential on which host.
//! Read-only; never returns credential or placeholder values.

use std::collections::HashSet;

use crate::openshell_proto::openshell::sandbox::v1::{NetworkPolicyRule, SandboxPolicy};

/// One attached provider's identity and candidate env vars, gathered from the
/// gateway before correlation. Pure-function input keeps the join testable.
#[derive(Debug, Clone)]
pub struct ProviderCapabilityInput {
    /// Gateway provider name, e.g. `right-right-github`.
    pub name: String,
    /// User-friendly name from the profile, e.g. `GitHub`.
    pub display_name: String,
    /// Env vars the profile declares for this credential.
    pub candidate_env_vars: Vec<String>,
}

/// Agent-facing capability record for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapability {
    pub display_name: String,
    /// Env var names actually injected into this sandbox for the provider.
    pub env_vars: Vec<String>,
    /// Binary paths allowed to use the credential by the effective policy.
    pub allowed_binaries: Vec<String>,
    /// Hosts the credential is valid for by the effective policy.
    pub endpoint_hosts: Vec<String>,
    /// One-line, agent-readable usage guidance.
    pub usage_hint: String,
}

/// `_provider_` prefix OpenShell uses for composed provider rules.
const PROVIDER_RULE_PREFIX: &str = "_provider_";

/// Lowercase and map every non-ASCII-alphanumeric char to `_`.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn rule_for_provider<'a>(
    policy: &'a SandboxPolicy,
    provider_name: &str,
) -> Option<&'a NetworkPolicyRule> {
    let want = sanitize(provider_name);
    policy.network_policies.iter().find_map(|(key, rule)| {
        let stripped = key.strip_prefix(PROVIDER_RULE_PREFIX)?;
        (sanitize(stripped) == want).then_some(rule)
    })
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn build_usage_hint(allowed_binaries: &[String], hosts: &[String], active: bool) -> String {
    if !active {
        return "Attached but not currently active in the sandbox policy; the provider may still be composing. While inactive, requests will not receive gateway credential substitution and will 401."
            .to_string();
    }

    if hosts.is_empty() {
        return "No endpoint hosts are currently allowed for this credential in the sandbox policy; gateway credential substitution cannot work until the policy includes a host."
            .to_string();
    }

    let hosts_list = hosts.join(", ");
    if allowed_binaries.is_empty() {
        return format!(
            "No binaries are currently allowed to use this credential on {hosts_list}; gateway credential substitution cannot work until the policy includes a binary."
        );
    }

    if allowed_binaries.iter().any(|binary| binary == "**") {
        return format!(
            "Any binary can use this credential on {hosts_list}; the gateway substitutes the credential automatically for matching requests. Do not paste the placeholder env var elsewhere."
        );
    }

    let mut binary_names: Vec<&str> = allowed_binaries
        .iter()
        .map(|binary| basename(binary))
        .collect();
    binary_names.sort_unstable();
    binary_names.dedup();
    let binary_list = binary_names.join(", ");

    format!(
        "Reach {hosts_list} via {binary_list}; the gateway substitutes the credential automatically for matching requests. curl/fetch/python or arbitrary clients will 401 if they paste the placeholder because they do not receive substitution."
    )
}

/// Join provider inputs with the effective policy and sandbox env keys. Pure
/// and sorted deterministically for stable output.
pub fn correlate_provider_capabilities(
    inputs: &[ProviderCapabilityInput],
    policy: &SandboxPolicy,
    sandbox_env_keys: &HashSet<String>,
) -> Vec<ProviderCapability> {
    let mut capabilities: Vec<ProviderCapability> = inputs
        .iter()
        .map(|input| {
            let rule = rule_for_provider(policy, &input.name);

            let mut env_vars: Vec<String> = input
                .candidate_env_vars
                .iter()
                .filter(|env_var| sandbox_env_keys.contains(*env_var))
                .cloned()
                .collect();
            env_vars.sort_unstable();
            env_vars.dedup();

            let mut allowed_binaries: Vec<String> = rule
                .map(|rule| {
                    rule.binaries
                        .iter()
                        .map(|binary| binary.path.clone())
                        .collect()
                })
                .unwrap_or_default();
            allowed_binaries.sort_unstable();
            allowed_binaries.dedup();

            let mut endpoint_hosts: Vec<String> = rule
                .map(|rule| {
                    rule.endpoints
                        .iter()
                        .map(|endpoint| endpoint.host.clone())
                        .collect()
                })
                .unwrap_or_default();
            endpoint_hosts.sort_unstable();
            endpoint_hosts.dedup();

            ProviderCapability {
                display_name: input.display_name.clone(),
                env_vars,
                usage_hint: build_usage_hint(&allowed_binaries, &endpoint_hosts, rule.is_some()),
                allowed_binaries,
                endpoint_hosts,
            }
        })
        .collect();
    capabilities.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    capabilities
}

#[cfg(test)]
#[path = "provider_capabilities_tests.rs"]
mod tests;
