use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Resolve RIGHTCLAW_HOME: cli_home > env_home > ~/.rightclaw
pub fn resolve_home(cli_home: Option<&str>, env_home: Option<&str>) -> miette::Result<PathBuf> {
    if let Some(home) = cli_home {
        return Ok(PathBuf::from(home));
    }
    if let Some(home) = env_home {
        return Ok(PathBuf::from(home));
    }
    let home =
        dirs::home_dir().ok_or_else(|| miette::miette!("Could not determine home directory"))?;
    Ok(home.join(".rightclaw"))
}

/// Global RightClaw configuration stored at `~/.rightclaw/config.yaml`.
#[derive(Debug, Clone, Default)]
pub struct GlobalConfig {
    pub tunnel: Option<TunnelConfig>,
    pub aggregator: AggregatorConfig,
}

/// MCP Aggregator HTTP server configuration.
///
/// Controls rmcp's DNS-rebinding Host-header check. Since v1.4.0, rmcp's
/// `StreamableHttpServerConfig::default()` only allows `localhost`/`127.0.0.1`/`::1`
/// as Host values, which breaks sandbox access where the Host is e.g.
/// `host.openshell.internal:8100`.
///
/// - Empty `allowed_hosts` (default) → `disable_allowed_hosts()`, skip the check
///   entirely. This is safe because the aggregator already authenticates every
///   request via per-agent Bearer tokens, and DNS rebinding protection only
///   matters for browser-ambient-auth scenarios that don't apply here.
/// - Non-empty → `with_allowed_hosts(...)`, enforce exactly that list. Use when
///   the aggregator is exposed on a fixed public hostname and defence-in-depth
///   is wanted.
#[derive(Debug, Clone, Default)]
pub struct AggregatorConfig {
    pub allowed_hosts: Vec<String>,
}

/// Cloudflare Named Tunnel configuration (credentials-file based, Phase 38+).
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    /// TunnelID read directly from the credentials JSON `TunnelID` field.
    pub tunnel_uuid: String,
    /// Absolute path to the cloudflared credentials JSON file.
    pub credentials_file: PathBuf,
    /// Public hostname for the tunnel (e.g. right.example.com).
    pub hostname: String,
}

/// Helper structs for YAML deserialization via serde-saphyr.
#[derive(Debug, Deserialize)]
struct RawGlobalConfig {
    tunnel: Option<RawTunnelConfig>,
    #[serde(default)]
    aggregator: Option<RawAggregatorConfig>,
}

#[derive(Debug, Deserialize)]
struct RawAggregatorConfig {
    #[serde(default)]
    allowed_hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawTunnelConfig {
    /// Legacy field — present in configs written before Phase 38. Keep to avoid parse error.
    /// Its presence (non-empty) with absent credentials_file triggers a migration error.
    #[serde(default)]
    #[allow(dead_code)]
    token: String,
    /// New field added in Phase 38.
    #[serde(default)]
    tunnel_uuid: String,
    #[serde(default)]
    credentials_file: String,
    #[serde(default)]
    hostname: String,
}

/// Path to the agents directory within a RightClaw home.
///
/// Single source of truth for `<home>/agents` — avoids scattered `home.join("agents")`.
pub fn agents_dir(home: &Path) -> PathBuf {
    home.join("agents")
}

/// Path to the backups directory for a specific agent.
///
/// Layout: `<home>/backups/<agent_name>/`
pub fn backups_dir(home: &Path, agent_name: &str) -> PathBuf {
    home.join("backups").join(agent_name)
}

/// Read global config from `<home>/config.yaml`.
///
/// Returns `Ok(GlobalConfig::default())` if the file does not exist.
/// Returns `Err` with a migration hint if the config uses the old `token:` format.
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig> {
    let path = home.join("config.yaml");
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| miette::miette!("read config.yaml: {e:#}"))?;
    let raw: RawGlobalConfig = serde_saphyr::from_str(&content)
        .map_err(|e| miette::miette!("parse config.yaml: {e:#}"))?;
    Ok(GlobalConfig {
        tunnel: raw
            .tunnel
            .map(|t| -> miette::Result<TunnelConfig> {
                if t.credentials_file.is_empty() || t.tunnel_uuid.is_empty() {
                    return Err(miette::miette!(
                        help = "run: rightclaw init --tunnel-name NAME --tunnel-hostname HOSTNAME",
                        "Tunnel config is outdated (uses token-based format) — re-run `rightclaw init` to migrate"
                    ));
                }
                Ok(TunnelConfig {
                    tunnel_uuid: t.tunnel_uuid,
                    credentials_file: PathBuf::from(&t.credentials_file),
                    hostname: t.hostname,
                })
            })
            .transpose()?,
        aggregator: raw
            .aggregator
            .map(|a| AggregatorConfig {
                allowed_hosts: a.allowed_hosts,
            })
            .unwrap_or_default(),
    })
}

/// Write global config to `<home>/config.yaml`.
///
/// Note: serde-saphyr is deserialize-only — YAML is written manually.
pub fn write_global_config(home: &Path, config: &GlobalConfig) -> miette::Result<()> {
    let path = home.join("config.yaml");
    let mut content = String::new();
    if let Some(ref tunnel) = config.tunnel {
        content.push_str("tunnel:\n");
        let uuid = tunnel.tunnel_uuid.replace('"', "\\\"");
        let creds = tunnel
            .credentials_file
            .display()
            .to_string()
            .replace('"', "\\\"");
        let hostname = tunnel.hostname.replace('"', "\\\"");
        content.push_str(&format!("  tunnel_uuid: \"{uuid}\"\n"));
        content.push_str(&format!("  credentials_file: \"{creds}\"\n"));
        content.push_str(&format!("  hostname: \"{hostname}\"\n"));
    }
    if !config.aggregator.allowed_hosts.is_empty() {
        content.push_str("aggregator:\n");
        content.push_str("  allowed_hosts:\n");
        for host in &config.aggregator.allowed_hosts {
            let escaped = host.replace('"', "\\\"");
            content.push_str(&format!("    - \"{escaped}\"\n"));
        }
    }
    std::fs::write(&path, &content).map_err(|e| miette::miette!("write config.yaml: {e:#}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_home_returns_cli_home_when_provided() {
        let result = resolve_home(Some("/custom/path"), Some("/env/path")).unwrap();
        assert_eq!(result, PathBuf::from("/custom/path"));
    }

    #[test]
    fn resolve_home_returns_env_home_when_cli_is_none() {
        let result = resolve_home(None, Some("/env/path")).unwrap();
        assert_eq!(result, PathBuf::from("/env/path"));
    }

    #[test]
    fn resolve_home_returns_default_when_both_none() {
        let result = resolve_home(None, None).unwrap();
        let expected = dirs::home_dir().unwrap().join(".rightclaw");
        assert_eq!(result, expected);
    }

    #[test]
    fn write_then_read_roundtrips_new_fields() {
        let dir = TempDir::new().unwrap();
        let written = GlobalConfig {
            tunnel: Some(TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            }),
            aggregator: AggregatorConfig::default(),
        };
        write_global_config(dir.path(), &written).unwrap();
        let read = read_global_config(dir.path()).unwrap();
        let tunnel = read.tunnel.expect("tunnel should be present after write");
        assert_eq!(tunnel.tunnel_uuid, "abc-123");
        assert_eq!(tunnel.credentials_file, PathBuf::from("/tmp/abc-123.json"));
        assert_eq!(tunnel.hostname, "test.example.com");
    }

    #[test]
    fn write_global_config_emits_tunnel_uuid_not_token() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: Some(TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            }),
            aggregator: AggregatorConfig::default(),
        };
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(
            content.contains("tunnel_uuid: \"abc-123\""),
            "written YAML must contain tunnel_uuid field, got: {content}"
        );
        assert!(
            !content.contains("token:"),
            "written YAML must NOT contain token field, got: {content}"
        );
    }

    #[test]
    fn old_config_with_token_only_yields_migration_error() {
        let dir = TempDir::new().unwrap();
        let yaml = "tunnel:\n  token: \"eyJhIjoiNjEy...\"\n  hostname: \"example.com\"\n";
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let err = read_global_config(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("re-run `rightclaw init`"),
            "expected migration error containing 're-run `rightclaw init`', got: {err}"
        );
    }

    #[test]
    fn old_config_missing_credentials_file_yields_migration_error() {
        let dir = TempDir::new().unwrap();
        let yaml = "tunnel:\n  token: \"tok\"\n  hostname: \"h.com\"\n";
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let err = read_global_config(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("re-run `rightclaw init`"),
            "expected migration error for old config format, got: {err}"
        );
    }

    #[test]
    fn read_global_config_returns_default_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let config = read_global_config(dir.path()).unwrap();
        assert!(config.tunnel.is_none(), "no tunnel config when file absent");
        assert!(
            config.aggregator.allowed_hosts.is_empty(),
            "aggregator.allowed_hosts defaults to empty (host check disabled)"
        );
    }

    #[test]
    fn aggregator_defaults_to_empty_allowed_hosts_when_missing() {
        let dir = TempDir::new().unwrap();
        // Valid config with only tunnel — no aggregator section.
        let yaml = "tunnel:\n  tunnel_uuid: \"u\"\n  credentials_file: \"/x\"\n  hostname: \"h\"\n";
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let config = read_global_config(dir.path()).unwrap();
        assert!(
            config.aggregator.allowed_hosts.is_empty(),
            "missing aggregator section → empty allowed_hosts → Host check disabled"
        );
    }

    #[test]
    fn aggregator_allowed_hosts_roundtrip() {
        let dir = TempDir::new().unwrap();
        let written = GlobalConfig {
            tunnel: None,
            aggregator: AggregatorConfig {
                allowed_hosts: vec![
                    "mcp.example.com".to_string(),
                    "mcp.example.com:8100".to_string(),
                ],
            },
        };
        write_global_config(dir.path(), &written).unwrap();
        let read = read_global_config(dir.path()).unwrap();
        assert_eq!(
            read.aggregator.allowed_hosts,
            vec![
                "mcp.example.com".to_string(),
                "mcp.example.com:8100".to_string(),
            ]
        );
    }

    #[test]
    fn write_skips_aggregator_block_when_allowed_hosts_empty() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig::default();
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(
            !content.contains("aggregator:"),
            "default (empty allowed_hosts) must not emit aggregator block, got: {content}"
        );
    }
}
