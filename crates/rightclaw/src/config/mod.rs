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
    }
}
