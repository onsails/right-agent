//! Agent-facing provider capability records.
//!
//! Joins the live sandbox policy, the sandbox's injected placeholder env vars,
//! and each attached provider's profile into a description the agent can read
//! to learn which binary can spend which credential on which host. Read-only;
//! never returns credential or placeholder values.

use std::collections::HashSet;

use crate::openshell_proto::openshell::sandbox::v1::{NetworkPolicyRule, SandboxPolicy};
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
use tonic::transport::Channel;

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
    /// Binary paths declared by the provider profile.
    pub profile_binaries: Vec<String>,
    /// Endpoint hosts declared by the provider profile.
    pub profile_endpoint_hosts: Vec<String>,
}

/// Agent-facing capability record for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapability {
    pub display_name: String,
    /// Env var names actually injected into this sandbox for the provider.
    pub env_vars: Vec<String>,
    /// Binary paths allowed to use the credential by policy or provider profile.
    pub allowed_binaries: Vec<String>,
    /// Hosts the credential is valid for by policy or provider profile.
    pub endpoint_hosts: Vec<String>,
    /// One-line, agent-readable usage guidance.
    pub usage_hint: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilitiesError {
    #[error("provider gRPC: {0}")]
    Provider(#[from] crate::providers::ProviderError),
    #[error("profile gRPC: {0}")]
    Profile(#[from] crate::managed_profiles::ManagedProfileError),
    #[error("policy read: {0}")]
    Policy(String),
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

/// True when the provider's composed `_provider_<name>` rule is present in the
/// sandbox's active policy. This is the direct composition signal: use it to
/// confirm composition actually happened, never the `policy set` return value.
pub fn provider_is_composed(policy: &SandboxPolicy, provider_name: &str) -> bool {
    rule_for_provider(policy, provider_name).is_some()
}

/// True when the provider's composed rule is present and contains the expected
/// endpoint host/path. Generic provider config updates use this stricter signal
/// so an already-present stale rule cannot satisfy composition confirmation.
pub fn provider_is_composed_with_endpoint(
    policy: &SandboxPolicy,
    provider_name: &str,
    expected_host: &str,
    expected_path: &str,
) -> bool {
    rule_for_provider(policy, provider_name).is_some_and(|rule| {
        rule.endpoints
            .iter()
            .any(|endpoint| endpoint_matches(endpoint, expected_host, expected_path))
    })
}

fn endpoint_matches(
    endpoint: &crate::openshell_proto::openshell::sandbox::v1::NetworkEndpoint,
    expected_host: &str,
    expected_path: &str,
) -> bool {
    // Hosts are DNS names: compare case-insensitively so a gateway that
    // normalizes the composed host's case is not mistaken for a stale
    // (uncomposed) rule. Path stays an exact match — it is a literal
    // upstream prefix, not case-folded.
    endpoint.host.eq_ignore_ascii_case(expected_host) && endpoint.path == expected_path
}

/// True when the composed `_provider_<name>` rule contains EVERY expected
/// (host, path). Multi-host providers must confirm all hosts so a stale rule
/// carrying only the unchanged first host cannot pass on an update.
pub fn provider_is_composed_with_all_endpoints(
    policy: &SandboxPolicy,
    provider_name: &str,
    expected: &[(String, String)],
) -> bool {
    if expected.is_empty() {
        return false;
    }

    rule_for_provider(policy, provider_name).is_some_and(|rule| {
        expected.iter().all(|(host, path)| {
            rule.endpoints
                .iter()
                .any(|endpoint| endpoint_matches(endpoint, host, path))
        })
    })
}

/// True when the composed `_provider_<name>` rule contains exactly the expected
/// endpoint set, ignoring duplicate active endpoints that match the same
/// expected pair. Use this when a provider update removes hosts, because a
/// stale superset rule must not confirm the new desired config.
pub fn provider_is_composed_with_exact_endpoints(
    policy: &SandboxPolicy,
    provider_name: &str,
    expected: &[(String, String)],
) -> bool {
    if expected.is_empty() {
        return false;
    }

    rule_for_provider(policy, provider_name).is_some_and(|rule| {
        expected.iter().all(|(host, path)| {
            rule.endpoints
                .iter()
                .any(|endpoint| endpoint_matches(endpoint, host, path))
        }) && rule.endpoints.iter().all(|endpoint| {
            expected
                .iter()
                .any(|(host, path)| endpoint_matches(endpoint, host, path))
        })
    })
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn build_usage_hint(
    allowed_binaries: &[String],
    hosts: &[String],
    env_vars: &[String],
    active: bool,
) -> String {
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

    if env_vars.is_empty() {
        return format!(
            "Reach {hosts_list}, but capability metadata is incomplete: the injected env var could not be identified, so auth-header guidance cannot be generated."
        );
    }

    let env_list = env_vars.join(", ");
    let first_env = &env_vars[0];

    if allowed_binaries.iter().any(|binary| binary == "**") {
        return format!(
            "Reach {hosts_list} using {env_list}. Write the auth exactly as the API documents using ${first_env}. The gateway substitutes the secret for the placeholder on matching requests. Do not print the placeholder."
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
        "Reach {hosts_list} via {binary_list} using {env_list}. Only those binaries may use matching requests; if making raw HTTP requests, write the auth exactly as the API documents using ${first_env}. The gateway substitutes the secret for the placeholder on matching requests. Do not print the placeholder."
    )
}

/// Join provider inputs with policy/profile constraints and sandbox env keys.
/// Pure and sorted deterministically for stable output.
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

            let materialized_profile = !env_vars.is_empty()
                && (!input.profile_binaries.is_empty() || !input.profile_endpoint_hosts.is_empty());
            let active = rule.is_some() || materialized_profile;

            let mut allowed_binaries: Vec<String> = match rule {
                Some(rule) => rule
                    .binaries
                    .iter()
                    .map(|binary| binary.path.clone())
                    .collect(),
                None if materialized_profile => input.profile_binaries.clone(),
                None => Vec::new(),
            };
            allowed_binaries.sort_unstable();
            allowed_binaries.dedup();

            let mut endpoint_hosts: Vec<String> = match rule {
                Some(rule) => rule
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.host.clone())
                    .collect(),
                None if materialized_profile => input.profile_endpoint_hosts.clone(),
                None => Vec::new(),
            };
            endpoint_hosts.sort_unstable();
            endpoint_hosts.dedup();

            let usage_hint =
                build_usage_hint(&allowed_binaries, &endpoint_hosts, &env_vars, active);

            ProviderCapability {
                display_name: input.display_name.clone(),
                env_vars,
                usage_hint,
                allowed_binaries,
                endpoint_hosts,
            }
        })
        .collect();
    capabilities.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    capabilities
}

fn env_keys_from_stdout(stdout: &str) -> HashSet<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

async fn sandbox_env_keys(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
) -> Result<HashSet<String>, CapabilitiesError> {
    // Run inside the sandbox so OpenShell exposes provider placeholder vars,
    // but print only names to keep placeholder values out of logs and results.
    let (stdout, exit_code) = crate::openshell::exec_in_sandbox(
        client,
        sandbox_id,
        &[
            "sh",
            "-lc",
            "env_output=$(env) || exit $?; printf '%s\\n' \"$env_output\" | sed 's/=.*//'",
        ],
        crate::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| CapabilitiesError::Policy(format!("{e:#}")))?;

    if exit_code != 0 {
        return Err(CapabilitiesError::Policy(format!(
            "sandbox env key scan exited with status {exit_code}"
        )));
    }

    Ok(env_keys_from_stdout(&stdout))
}

pub async fn provider_capabilities_for_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
) -> Result<Vec<ProviderCapability>, CapabilitiesError> {
    let provider_names = crate::providers::list_attached(client, sandbox_name).await?;
    let mut inputs = Vec::with_capacity(provider_names.len());

    for name in provider_names {
        let provider = match crate::providers::get_provider(client, &name).await {
            Ok(provider) => provider,
            // Detached between list_attached and now — drop it rather than fail
            // the whole call and hide every still-attached provider.
            Err(crate::providers::ProviderError::NotFound(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let provider_type = provider.type_;
        let profile = crate::managed_profiles::get_profile(client, &provider_type).await?;

        let (display_name, candidate_env_vars, profile_binaries, profile_endpoint_hosts) =
            match profile {
                Some(profile) => {
                    let display_name = if profile.display_name.is_empty() {
                        provider_type.clone()
                    } else {
                        profile.display_name
                    };
                    let candidate_env_vars = profile
                        .credentials
                        .iter()
                        .flat_map(|credential| credential.env_vars.iter().cloned())
                        .collect();
                    let profile_binaries = profile
                        .binaries
                        .iter()
                        .map(|binary| binary.path.clone())
                        .filter(|path| !path.is_empty())
                        .collect();
                    let profile_endpoint_hosts = profile
                        .endpoints
                        .iter()
                        .map(|endpoint| endpoint.host.clone())
                        .filter(|host| !host.is_empty())
                        .collect();
                    (
                        display_name,
                        candidate_env_vars,
                        profile_binaries,
                        profile_endpoint_hosts,
                    )
                }
                None => (provider_type, Vec::new(), Vec::new(), Vec::new()),
            };

        inputs.push(ProviderCapabilityInput {
            name,
            display_name,
            candidate_env_vars,
            profile_binaries,
            profile_endpoint_hosts,
        });
    }

    let policy = crate::openshell::get_effective_policy(client, sandbox_name)
        .await
        .map_err(|e| CapabilitiesError::Policy(format!("{e:#}")))?
        .unwrap_or_default();
    let sandbox_id = crate::openshell::resolve_sandbox_id(client, sandbox_name)
        .await
        .map_err(|e| CapabilitiesError::Policy(format!("{e:#}")))?;
    let env_keys = sandbox_env_keys(client, &sandbox_id).await?;

    Ok(correlate_provider_capabilities(&inputs, &policy, &env_keys))
}

#[cfg(test)]
#[path = "provider_capabilities_tests.rs"]
mod tests;
