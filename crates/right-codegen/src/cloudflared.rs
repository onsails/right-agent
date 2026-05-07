use std::path::PathBuf;

use minijinja::{Environment, context};
use serde::Serialize;

const CF_TEMPLATE: &str = include_str!("../templates/cloudflared-config.yml.j2");

/// Serializable agent entry for the cloudflared ingress template.
#[derive(Debug, Serialize)]
struct CloudflaredAgent {
    name: String,
    socket_path: String,
}

/// Cloudflared credentials for local ingress mode.
///
/// The generated config always includes `tunnel:` and `credentials-file:`
/// fields so cloudflared honours local ingress rules instead of fetching
/// remote configuration from the Cloudflare dashboard (which is the
/// behaviour with `--token` alone).
pub struct CloudflaredCredentials {
    pub tunnel_uuid: String,
    pub credentials_file: PathBuf,
}

/// Generate cloudflared ingress config YAML for Telegram webhook and
/// OAuth callback routing.
///
/// Per agent the template emits two ingress rules — a `/tg/<agent>/.*`
/// webhook rule and an `/oauth/<agent>/callback` rule — followed by a
/// catch-all `service: http_status:404`. First match wins, so the
/// webhook rule comes before the OAuth rule for the same agent.
///
/// # Arguments
///
/// * `agents` — slice of `(name, agent_dir)` pairs
/// * `tunnel_hostname` — public hostname for the named tunnel (e.g. `right.example.com`)
/// * `credentials` — tunnel credentials embedded as `tunnel:` + `credentials-file:`.
///   Mandatory: cloudflared runs in local-ingress mode unconditionally.
pub fn generate_cloudflared_config(
    agents: &[(String, PathBuf)],
    tunnel_hostname: &str,
    credentials: &CloudflaredCredentials,
) -> miette::Result<String> {
    let cf_agents: Vec<CloudflaredAgent> = agents
        .iter()
        .map(|(name, dir)| CloudflaredAgent {
            name: name.clone(),
            socket_path: dir.join("bot.sock").to_string_lossy().to_string(),
        })
        .collect();

    // cloudflared ingress hostname must be a bare hostname without scheme.
    let hostname = tunnel_hostname
        .strip_prefix("https://")
        .or_else(|| tunnel_hostname.strip_prefix("http://"))
        .unwrap_or(tunnel_hostname);

    let tunnel_uuid = credentials.tunnel_uuid.as_str();
    let credentials_file = credentials.credentials_file.display().to_string();

    let mut env = Environment::new();
    env.add_template("cloudflared", CF_TEMPLATE)
        .map_err(|e| miette::miette!("cloudflared template parse error: {e:#}"))?;
    let tmpl = env
        .get_template("cloudflared")
        .expect("template was just added");
    tmpl.render(context! {
        agents => cf_agents,
        tunnel_hostname => hostname,
        tunnel_uuid => tunnel_uuid,
        credentials_file => credentials_file,
    })
    .map_err(|e| miette::miette!("cloudflared template render error: {e:#}"))
}

#[cfg(test)]
#[path = "cloudflared_tests.rs"]
mod tests;
