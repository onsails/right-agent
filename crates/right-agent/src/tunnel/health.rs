use std::path::Path;
use std::time::Duration;

use right_core::config::read_global_config;
use crate::runtime::pc_client::PcClient;

/// Timeout for the tunnel hostname reachability probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Name of the cloudflared process in process-compose.
const CLOUDFLARED_PROCESS_NAME: &str = "cloudflared";

/// Expected process-compose status for a healthy process.
const RUNNING_STATUS: &str = "Running";

/// Current state of the Cloudflare tunnel.
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelState {
    /// No tunnel section in config.yaml.
    NotConfigured,
    /// Tunnel configured but cloudflared process is not running.
    NotRunning,
    /// Cloudflared is running but hostname is unreachable.
    Unhealthy { reason: String },
    /// Tunnel is up and the hostname responds.
    Healthy,
}

impl TunnelState {
    /// Human-readable error for non-healthy states. Returns `None` for `Healthy`.
    pub fn error_message(&self) -> Option<String> {
        match self {
            Self::NotConfigured => Some(
                "Tunnel not configured. Run `right config` to set up. \
                 Without a tunnel, OAuth callbacks can't reach this agent."
                    .to_string(),
            ),
            Self::NotRunning => Some(
                "Tunnel is configured but cloudflared is not running. \
                 Is `right up` running?"
                    .to_string(),
            ),
            Self::Unhealthy { reason } => Some(format!(
                "Tunnel is configured and cloudflared is running, but the hostname \
                 is not reachable: {reason}. Check DNS and Cloudflare dashboard."
            )),
            Self::Healthy => None,
        }
    }
}

/// Check the current tunnel health.
///
/// 1. Reads `config.yaml` — returns `NotConfigured` if no tunnel section.
/// 2. Queries process-compose for a `cloudflared` process in `Running` status.
/// 3. Probes `https://<hostname>/healthz-tunnel-probe` — any HTTP response (even 404) means healthy.
///
/// `PcClient::from_home` resolves the PC port from `<home>/run/state.json`; when
/// this home has no recorded runtime, returns `NotRunning` without probing any port.
pub async fn check_tunnel(home: &Path) -> TunnelState {
    // Step 1: read config
    let config = match read_global_config(home) {
        Ok(c) => c,
        Err(_) => return TunnelState::NotConfigured,
    };

    let tunnel = config.tunnel;

    // Step 2: check process-compose for cloudflared
    let pc = match PcClient::from_home(home) {
        Ok(Some(c)) => c,
        Ok(None) => return TunnelState::NotRunning,
        Err(_) => return TunnelState::NotRunning,
    };

    let processes = match pc.list_processes().await {
        Ok(p) => p,
        Err(_) => return TunnelState::NotRunning,
    };

    let cloudflared_running = processes
        .iter()
        .any(|p| p.name == CLOUDFLARED_PROCESS_NAME && p.status == RUNNING_STATUS);

    if !cloudflared_running {
        return TunnelState::NotRunning;
    }

    // Step 3: probe the hostname
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            return TunnelState::Unhealthy {
                reason: format!("{e:#}"),
            };
        }
    };

    let url = format!("https://{}/healthz-tunnel-probe", tunnel.hostname);
    match client.get(&url).send().await {
        Ok(_) => TunnelState::Healthy,
        Err(e) => TunnelState::Unhealthy {
            reason: format!("{e:#}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn not_configured_has_error_message() {
        let msg = TunnelState::NotConfigured.error_message();
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(
            text.contains("right config"),
            "expected 'right config' in message, got: {text}"
        );
    }

    #[test]
    fn not_running_has_error_message() {
        let msg = TunnelState::NotRunning.error_message();
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(
            text.contains("right up"),
            "expected 'right up' in message, got: {text}"
        );
    }

    #[test]
    fn unhealthy_includes_reason() {
        let state = TunnelState::Unhealthy {
            reason: "connection refused".to_string(),
        };
        let msg = state.error_message();
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(
            text.contains("connection refused"),
            "expected reason in message, got: {text}"
        );
    }

    #[test]
    fn healthy_has_no_error() {
        assert!(TunnelState::Healthy.error_message().is_none());
    }

    #[tokio::test]
    async fn check_tunnel_returns_not_configured_when_no_config() {
        let dir = TempDir::new().unwrap();
        let state = check_tunnel(dir.path()).await;
        assert_eq!(state, TunnelState::NotConfigured);
    }

    #[tokio::test]
    async fn check_tunnel_returns_not_running_when_pc_unreachable() {
        let dir = TempDir::new().unwrap();
        // Write a valid config.yaml with tunnel section
        let yaml = concat!(
            "tunnel:\n",
            "  tunnel_uuid: \"test-uuid\"\n",
            "  credentials_file: \"/tmp/test.json\"\n",
            "  hostname: \"test.example.com\"\n",
        );
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();

        // No <home>/run/state.json → from_home returns None → NotRunning.
        let state = check_tunnel(dir.path()).await;
        assert_eq!(state, TunnelState::NotRunning);
    }
}
