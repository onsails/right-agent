use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
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

/// Cloudflare Named Tunnel configuration.
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub token: String,
    pub hostname: String,
}

impl TunnelConfig {
    /// Extract UUID from tunnel token. Supports single-segment (real CF token)
    /// and three-segment JWT formats. Returns the raw UUID string (no domain suffix).
    pub fn tunnel_uuid(&self) -> miette::Result<String> {
        let parts: Vec<&str> = self.token.split('.').collect();
        let encoded = match parts.len() {
            1 => parts[0],
            3 => parts[1],
            n => {
                return Err(miette::miette!(
                    "unrecognized token format (expected 1 or 3 segments, got {n})"
                ));
            }
        };
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| miette::miette!("tunnel token base64 decode failed: {e}"))?;
        let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| miette::miette!("tunnel token JSON parse failed: {e:#}"))?;
        let uuid = payload["t"]
            .as_str()
            .ok_or_else(|| miette::miette!("tunnel token payload missing 't' field"))?;
        Ok(uuid.to_string())
    }
}

/// Helper structs for YAML deserialization via serde-saphyr.
#[derive(Debug, Deserialize)]
struct RawGlobalConfig {
    tunnel: Option<RawTunnelConfig>,
}

#[derive(Debug, Deserialize)]
struct RawTunnelConfig {
    token: String,
    /// `hostname` was added in v3.2. Old config.yaml files written before that only contain
    /// `token:`. Using `#[serde(default)]` lets those files parse successfully with an empty
    /// hostname; `cmd_up` detects the empty value and guides the user to re-run `rightclaw init`.
    #[serde(default)]
    hostname: String,
}

/// Read global config from `<home>/config.yaml`.
///
/// Returns `Ok(GlobalConfig::default())` if the file does not exist.
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig> {
    let path = home.join("config.yaml");
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| miette::miette!("read config.yaml: {e:#}"))?;
    let raw: RawGlobalConfig = serde_saphyr::from_str(&content)
        .map_err(|e| miette::miette!("parse config.yaml: {e:#}"))?;
    Ok(GlobalConfig {
        tunnel: raw.tunnel.map(|t| TunnelConfig { token: t.token, hostname: t.hostname }),
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
        let token = tunnel.token.replace('"', "\\\"");
        let hostname = tunnel.hostname.replace('"', "\\\"");
        content.push_str(&format!("  token: \"{token}\"\n"));
        content.push_str(&format!("  hostname: \"{hostname}\"\n"));
    }
    std::fs::write(&path, &content)
        .map_err(|e| miette::miette!("write config.yaml: {e:#}"))?;
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
    fn read_global_config_returns_default_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let config = read_global_config(dir.path()).unwrap();
        assert!(config.tunnel.is_none(), "no tunnel config when file absent");
    }

    fn make_fake_jwt(uuid: &str) -> String {
        let payload = format!(r#"{{"t":"{uuid}"}}"#);
        let encoded = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("eyJhbGciOiJIUzI1NiJ9.{encoded}.sig")
    }

    #[test]
    fn tunnel_uuid_decode_valid_jwt() {
        let token = make_fake_jwt("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        let cfg = TunnelConfig { token, hostname: "test.example.com".to_string() };
        let uuid = cfg.tunnel_uuid().unwrap();
        assert_eq!(uuid, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    }

    #[test]
    fn tunnel_uuid_decode_single_segment() {
        // Real CF token: single base64url-encoded JSON blob
        let token = "eyJhIjoiNjEyZWE2ZmU3ZjBiMmY2Njg5ZjdjYjAxNTc4NWZhM2UiLCJ0IjoiN2EyMTU1YTUtMmFiMy00ZmNkLTlhNDUtZTYxY2NlNDc0ODc5IiwicyI6Ik56WXhaVGs0WkRJdE56RXdOeTAwT1RsbUxUaGpPVEF0TkRrek5qSXpZVE0wTUdVMSJ9".to_string();
        let cfg = TunnelConfig { token, hostname: "right.example.com".to_string() };
        let uuid = cfg.tunnel_uuid().unwrap();
        assert_eq!(uuid, "aaaabbbb-0000-1111-2222-ccccddddeeee");
    }

    #[test]
    fn tunnel_uuid_wrong_segment_count() {
        let cfg = TunnelConfig { token: "a.b".to_string(), hostname: "h.example.com".to_string() };
        let err = cfg.tunnel_uuid().unwrap_err();
        assert!(
            err.to_string().contains("unrecognized token format (expected 1 or 3 segments, got 2)"),
            "expected unrecognized token format error in: {err}"
        );
    }

    #[test]
    fn tunnel_uuid_invalid_base64() {
        let cfg = TunnelConfig { token: "hdr.!!!.sig".to_string(), hostname: "h.example.com".to_string() };
        let err = cfg.tunnel_uuid().unwrap_err();
        assert!(!err.to_string().is_empty(), "should have error message");
    }

    #[test]
    fn tunnel_uuid_missing_t_field() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"other":"value"}"#.as_bytes());
        let cfg = TunnelConfig { token: format!("h.{payload}.sig"), hostname: "h.example.com".to_string() };
        let err = cfg.tunnel_uuid().unwrap_err();
        assert!(
            err.to_string().contains("missing 't' field"),
            "expected \"missing 't' field\" in: {err}"
        );
    }

    #[test]
    fn write_then_read_roundtrips_token_and_hostname() {
        let dir = TempDir::new().unwrap();
        let written = GlobalConfig {
            tunnel: Some(TunnelConfig {
                token: "tok123".to_string(),
                hostname: "my.example.com".to_string(),
            }),
        };
        write_global_config(dir.path(), &written).unwrap();
        let read = read_global_config(dir.path()).unwrap();
        let tunnel = read.tunnel.expect("tunnel should be present after write");
        assert_eq!(tunnel.token, "tok123");
        assert_eq!(tunnel.hostname, "my.example.com");
    }

    #[test]
    fn write_global_config_writes_hostname_field() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: Some(TunnelConfig {
                token: "mytoken".to_string(),
                hostname: "my.example.com".to_string(),
            }),
        };
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(content.contains("hostname: \"my.example.com\""), "written YAML must contain hostname field");
        assert!(content.contains("token:"), "written YAML must contain 'token:'");
    }

    #[test]
    fn read_config_parses_token_and_hostname() {
        let dir = TempDir::new().unwrap();
        let yaml = r#"tunnel:
  token: "testtoken"
  hostname: "test.example.com"
"#;
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let config = read_global_config(dir.path()).unwrap();
        let tunnel = config.tunnel.expect("tunnel should be parsed");
        assert_eq!(tunnel.token, "testtoken");
        assert_eq!(tunnel.hostname, "test.example.com");
    }
}
