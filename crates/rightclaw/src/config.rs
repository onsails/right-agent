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

/// Cloudflare Named Tunnel configuration (per D-03).
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub token: String,
    pub hostname: String,
}

/// Helper structs for YAML deserialization via serde-saphyr.
#[derive(Debug, Deserialize)]
struct RawGlobalConfig {
    tunnel: Option<RawTunnelConfig>,
}

#[derive(Debug, Deserialize)]
struct RawTunnelConfig {
    token: String,
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
        tunnel: raw.tunnel.map(|t| TunnelConfig {
            token: t.token,
            hostname: t.hostname,
        }),
    })
}

/// Write global config to `<home>/config.yaml`.
///
/// Note: serde-saphyr is deserialize-only — YAML is written manually.
/// The schema is small and stable so this is not a maintenance burden.
pub fn write_global_config(home: &Path, config: &GlobalConfig) -> miette::Result<()> {
    let path = home.join("config.yaml");
    let mut content = String::new();
    if let Some(ref tunnel) = config.tunnel {
        content.push_str("tunnel:\n");
        // Escape quotes in token/hostname defensively
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

    // --- JWT hostname derivation tests (Task 1 RED tests) ---

    fn make_fake_jwt(uuid: &str) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = format!(r#"{{"t":"{uuid}"}}"#);
        let encoded = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("eyJhbGciOiJIUzI1NiJ9.{encoded}.sig")
    }

    #[test]
    fn hostname_decode_valid_token() {
        let token = make_fake_jwt("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        let cfg = TunnelConfig { token };
        let h = cfg.hostname().unwrap();
        assert_eq!(h, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.cfargotunnel.com");
    }

    #[test]
    fn hostname_decode_wrong_segment_count() {
        let cfg = TunnelConfig { token: "notajwt".to_string() };
        let err = cfg.hostname().unwrap_err();
        assert!(
            err.to_string().contains("wrong number of segments"),
            "expected 'wrong number of segments' in: {err}"
        );
    }

    #[test]
    fn hostname_decode_invalid_base64() {
        let cfg = TunnelConfig { token: "hdr.!!!.sig".to_string() };
        let err = cfg.hostname().unwrap_err();
        // Should fail at base64 decode step
        assert!(err.to_string().len() > 0, "should have error message");
    }

    #[test]
    fn hostname_decode_missing_t_field() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = URL_SAFE_NO_PAD.encode(r#"{"other":"value"}"#.as_bytes());
        let cfg = TunnelConfig { token: format!("h.{payload}.sig") };
        let err = cfg.hostname().unwrap_err();
        assert!(
            err.to_string().contains("missing 't' field"),
            "expected \"missing 't' field\" in: {err}"
        );
    }

    #[test]
    fn write_then_read_roundtrips_token_only() {
        let dir = TempDir::new().unwrap();
        let written = GlobalConfig {
            tunnel: Some(TunnelConfig {
                token: "tok123".to_string(),
            }),
        };
        write_global_config(dir.path(), &written).unwrap();
        let read = read_global_config(dir.path()).unwrap();
        let tunnel = read.tunnel.expect("tunnel should be present after write");
        assert_eq!(tunnel.token, "tok123");
    }

    #[test]
    fn read_config_with_legacy_hostname_field_silently_ignored() {
        let dir = TempDir::new().unwrap();
        let yaml = r#"tunnel:
  token: "tok"
  hostname: "old.example.com"
"#;
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let config = read_global_config(dir.path()).unwrap();
        let tunnel = config.tunnel.expect("tunnel should be parsed");
        assert_eq!(tunnel.token, "tok");
        // no hostname field on struct — confirms it was silently ignored
    }

    #[test]
    fn write_global_config_writes_only_token_field() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: Some(TunnelConfig {
                token: "mytoken".to_string(),
            }),
        };
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(
            !content.contains("hostname:"),
            "written YAML must not contain 'hostname:'"
        );
        assert!(content.contains("token:"), "written YAML must contain 'token:'");
    }

    // --- Updated existing tests (no hostname field) ---

    #[test]
    fn write_global_config_creates_valid_yaml() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: Some(TunnelConfig {
                token: "mytoken".to_string(),
            }),
        };
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        // Should be parseable by serde-saphyr
        let raw: RawGlobalConfig = serde_saphyr::from_str(&content).unwrap();
        let tunnel = raw.tunnel.expect("tunnel should parse from written YAML");
        assert_eq!(tunnel.token, "mytoken");
    }

    #[test]
    fn read_global_config_parses_yaml_with_tunnel_fields() {
        let dir = TempDir::new().unwrap();
        let yaml = r#"tunnel:
  token: "testtoken"
  hostname: "test.example.com"
"#;
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let config = read_global_config(dir.path()).unwrap();
        let tunnel = config.tunnel.expect("tunnel should be parsed");
        assert_eq!(tunnel.token, "testtoken");
        // hostname field no longer exists on TunnelConfig — only token is checked
    }
}
