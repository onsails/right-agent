use std::path::PathBuf;

use minijinja::{Environment, context};
use serde::Serialize;

const CF_TEMPLATE: &str = include_str!("../../../../templates/cloudflared-config.yml.j2");

/// Serializable agent entry for the cloudflared ingress template.
#[derive(Debug, Serialize)]
struct CloudflaredAgent {
    name: String,
    socket_path: String,
}

/// Cloudflared credentials for local ingress mode.
///
/// When provided, the generated config includes `tunnel:` and `credentials-file:`
/// fields so cloudflared honours local ingress rules instead of fetching remote
/// configuration from the Cloudflare dashboard (which is the behaviour with
/// `--token` alone).
pub struct CloudflaredCredentials {
    pub tunnel_uuid: String,
    pub credentials_file: PathBuf,
}

/// Generate cloudflared ingress config YAML for OAuth callback routing.
///
/// Each agent gets a path-based ingress rule routing `GET /oauth/<name>/callback`
/// to its Unix socket at `<agent_dir>/oauth-callback.sock`.
///
/// Per cloudflared requirements (Pitfall 1), the config always ends with a
/// catch-all `service: http_status:404` rule — cloudflared refuses to start
/// without it.
///
/// # Arguments
///
/// * `agents` — slice of `(name, agent_dir)` pairs
/// * `tunnel_hostname` — public hostname for the named tunnel (e.g. `example.trycloudflare.com`)
/// * `credentials` — when `Some`, embeds `tunnel:` + `credentials-file:` in the config so
///   cloudflared uses local ingress instead of remote config
pub fn generate_cloudflared_config(
    agents: &[(String, PathBuf)],
    tunnel_hostname: &str,
    credentials: Option<&CloudflaredCredentials>,
) -> miette::Result<String> {
    let cf_agents: Vec<CloudflaredAgent> = agents
        .iter()
        .map(|(name, dir)| CloudflaredAgent {
            name: name.clone(),
            socket_path: dir
                .join("oauth-callback.sock")
                .to_string_lossy()
                .to_string(),
        })
        .collect();

    // cloudflared ingress hostname must be a bare hostname without scheme.
    // Strip https:// or http:// if the caller passed a full URL.
    let hostname = tunnel_hostname
        .strip_prefix("https://")
        .or_else(|| tunnel_hostname.strip_prefix("http://"))
        .unwrap_or(tunnel_hostname);

    let tunnel_uuid = credentials.map(|c| c.tunnel_uuid.as_str()).unwrap_or("");
    let credentials_file = credentials
        .map(|c| c.credentials_file.display().to_string())
        .unwrap_or_default();

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
