use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Migrate `~/.rightclaw/` to `~/.right/` if needed.
///
/// Returns `Ok(())` when:
/// - The old dir doesn't exist (fresh install or already migrated).
/// - Both dirs exist (already migrated; old dir left alone — operator can decide).
/// - The old dir was successfully renamed.
///
/// Returns `Err` when:
/// - process-compose is running against the old `state.json` port.
/// - The atomic rename failed (cross-filesystem `EXDEV`, permissions).
fn migrate_old_home(old: &Path, new: &Path) -> miette::Result<()> {
    if !old.exists() {
        return Ok(());
    }
    if new.exists() {
        return Ok(());
    }

    // PC-running probe: if state.json carries a port and that port is
    // accepting connections, a process-compose instance is alive. Refuse
    // migration to avoid breaking open file handles.
    const MAX_STATE_JSON_BYTES: u64 = 64 * 1024;

    let state_path = old.join("run").join("state.json");
    if state_path.exists() {
        // Bounded read — state.json is normally <1KB; cap to defend against
        // pathological / corrupted files.
        let content = std::fs::metadata(&state_path)
            .ok()
            .filter(|m| m.len() <= MAX_STATE_JSON_BYTES)
            .and_then(|_| std::fs::read_to_string(&state_path).ok());
        if let Some(content) = content
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
            && let Some(port) = json
                .get("pc_port")
                .and_then(|v| v.as_u64())
                .and_then(|p| u16::try_from(p).ok())
        {
            let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
            if std::net::TcpStream::connect_timeout(
                &addr,
                std::time::Duration::from_millis(500),
            )
            .is_ok()
            {
                return Err(miette::miette!(
                    "Detected {} with a running process-compose on port {}. \
                     Stop it before upgrading — run the old `right down` (or kill \
                     the process-compose process), then re-run.",
                    old.display(),
                    port,
                ));
            }
        }
    }

    std::fs::rename(old, new).map_err(|e| {
        miette::miette!(
            "Failed to rename {} → {}: {}. \
             If the dirs are on different filesystems, run `mv {} {}` manually and re-run.",
            old.display(),
            new.display(),
            e,
            old.display(),
            new.display(),
        )
    })?;

    tracing::info!("Migrated {} → {}", old.display(), new.display());
    Ok(())
}

/// Resolve the runtime home directory: cli_home > env_home > ~/.right
///
/// When falling through to the default path, also triggers
/// `migrate_old_home` to rename a leftover `~/.rightclaw/` from a
/// pre-rename install. The migration is idempotent and fast (single
/// existence check + rename or noop).
pub fn resolve_home(cli_home: Option<&str>, env_home: Option<&str>) -> miette::Result<PathBuf> {
    if let Some(home) = cli_home {
        return Ok(PathBuf::from(home));
    }
    if let Some(home) = env_home {
        return Ok(PathBuf::from(home));
    }
    let home_dir =
        dirs::home_dir().ok_or_else(|| miette::miette!("Could not determine home directory"))?;
    resolve_home_with_base(&home_dir)
}

/// Inner resolve with an explicit user home directory — used by tests to avoid
/// touching the real `~/` (which may have a running process-compose on `~/.right`).
fn resolve_home_with_base(home_dir: &Path) -> miette::Result<PathBuf> {
    let new = home_dir.join(".right");
    let old = home_dir.join(".rightclaw");
    migrate_old_home(&old, &new)?;
    Ok(new)
}

/// Global Right Agent configuration stored at `~/.right/config.yaml`.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub tunnel: TunnelConfig,
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

/// Path to the agents directory within a Right Agent home.
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
/// Returns `Err` if the file does not exist or has no `tunnel:` block —
/// Cloudflare Tunnel is mandatory (Telegram webhooks require it).
/// Returns `Err` with a migration hint if the config uses the old `token:` format.
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig> {
    let path = home.join("config.yaml");
    if !path.exists() {
        return Err(miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "global config not found at {} — tunnel configuration is required (run `right init`)",
            path.display()
        ));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| miette::miette!("read config.yaml: {e:#}"))?;
    let raw: RawGlobalConfig = serde_saphyr::from_str(&content)
        .map_err(|e| miette::miette!("parse config.yaml: {e:#}"))?;
    let raw_tunnel = raw.tunnel.ok_or_else(|| {
        miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "config.yaml has no `tunnel:` block — Cloudflare Tunnel is required (re-run `right init`)"
        )
    })?;
    if raw_tunnel.credentials_file.is_empty() || raw_tunnel.tunnel_uuid.is_empty() {
        return Err(miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "Tunnel config is outdated (uses token-based format) — re-run `right init` to migrate"
        ));
    }
    Ok(GlobalConfig {
        tunnel: TunnelConfig {
            tunnel_uuid: raw_tunnel.tunnel_uuid,
            credentials_file: PathBuf::from(&raw_tunnel.credentials_file),
            hostname: raw_tunnel.hostname,
        },
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
    content.push_str("tunnel:\n");
    let uuid = config.tunnel.tunnel_uuid.replace('"', "\\\"");
    let creds = config
        .tunnel
        .credentials_file
        .display()
        .to_string()
        .replace('"', "\\\"");
    let hostname = config.tunnel.hostname.replace('"', "\\\"");
    content.push_str(&format!("  tunnel_uuid: \"{uuid}\"\n"));
    content.push_str(&format!("  credentials_file: \"{creds}\"\n"));
    content.push_str(&format!("  hostname: \"{hostname}\"\n"));
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
        // Use a tempdir as the "user home" so we don't touch the real ~/
        // (which may have ~/.right or ~/.rightclaw with a running process-compose).
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_home_with_base(tmp.path()).unwrap();
        let expected = tmp.path().join(".right");
        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_home_returns_dot_right_default() {
        // Same isolation strategy: inject a tempdir as the user home base.
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_home_with_base(tmp.path()).unwrap();
        let expected = tmp.path().join(".right");
        assert_eq!(result, expected, "default home must be ~/.right after rename");
    }

    #[test]
    fn write_then_read_roundtrips_new_fields() {
        let dir = TempDir::new().unwrap();
        let written = GlobalConfig {
            tunnel: TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            },
            aggregator: AggregatorConfig::default(),
        };
        write_global_config(dir.path(), &written).unwrap();
        let read = read_global_config(dir.path()).unwrap();
        assert_eq!(read.tunnel.tunnel_uuid, "abc-123");
        assert_eq!(
            read.tunnel.credentials_file,
            PathBuf::from("/tmp/abc-123.json")
        );
        assert_eq!(read.tunnel.hostname, "test.example.com");
    }

    #[test]
    fn write_global_config_emits_tunnel_uuid_not_token() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            },
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
            err.to_string().contains("re-run `right init`"),
            "expected migration error containing 're-run `right init`', got: {err}"
        );
    }

    #[test]
    fn old_config_missing_credentials_file_yields_migration_error() {
        let dir = TempDir::new().unwrap();
        let yaml = "tunnel:\n  token: \"tok\"\n  hostname: \"h.com\"\n";
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let err = read_global_config(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("re-run `right init`"),
            "expected migration error for old config format, got: {err}"
        );
    }

    #[test]
    fn read_global_config_errors_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let err = read_global_config(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not found") || msg.contains("tunnel"),
            "error should mention missing config or tunnel, got: {msg}"
        );
    }

    #[test]
    fn read_global_config_errors_when_tunnel_block_missing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "aggregator:\n  allowed_hosts:\n    - example.com\n",
        )
        .unwrap();
        let err = read_global_config(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("tunnel"),
            "error must mention tunnel, got: {msg}"
        );
        assert!(
            msg.contains("right init"),
            "error must hint at right init, got: {msg}"
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
            tunnel: TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            },
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
        assert_eq!(read.tunnel.tunnel_uuid, "abc-123");
    }

    #[test]
    fn write_skips_aggregator_block_when_allowed_hosts_empty() {
        let dir = TempDir::new().unwrap();
        let config = GlobalConfig {
            tunnel: TunnelConfig {
                tunnel_uuid: "abc-123".to_string(),
                credentials_file: PathBuf::from("/tmp/abc-123.json"),
                hostname: "test.example.com".to_string(),
            },
            aggregator: AggregatorConfig::default(),
        };
        write_global_config(dir.path(), &config).unwrap();
        let content = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(
            !content.contains("aggregator:"),
            "empty allowed_hosts must not emit aggregator block, got: {content}"
        );
    }

    #[test]
    fn migrate_old_home_renames_when_only_old_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("marker"), b"hello").unwrap();

        let result = migrate_old_home(&old, &new);
        assert!(result.is_ok(), "migrate_old_home failed: {:?}", result);
        assert!(!old.exists(), "old dir must be gone after migration");
        assert!(new.exists(), "new dir must exist after migration");
        assert_eq!(
            std::fs::read(new.join("marker")).unwrap(),
            b"hello",
            "contents must be preserved"
        );
    }

    #[test]
    fn migrate_old_home_noop_when_new_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();

        let result = migrate_old_home(&old, &new);
        assert!(result.is_ok(), "must noop when new dir exists");
        assert!(old.exists(), "old dir must still exist (no rename)");
        assert!(new.exists(), "new dir must still exist");
    }

    #[test]
    fn migrate_old_home_noop_when_old_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");

        let result = migrate_old_home(&old, &new);
        assert!(result.is_ok(), "must noop when old dir absent");
        assert!(!old.exists());
        assert!(!new.exists());
    }

    #[test]
    fn migrate_old_home_refuses_when_pc_running() {
        use std::net::TcpListener;

        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");
        std::fs::create_dir_all(old.join("run")).unwrap();

        // Bind a local TCP listener and write its port into state.json.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            old.join("run").join("state.json"),
            format!(
                r#"{{"agents":[],"socket_path":"","started_at":"x","pc_port":{port},"pc_api_token":null}}"#
            ),
        )
        .unwrap();

        let err = migrate_old_home(&old, &new).expect_err("must refuse migration");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("running") || msg.contains("process-compose"),
            "error must mention PC running; got: {msg}"
        );
        assert!(old.exists(), "no rename on refusal");
        assert!(!new.exists());
    }

    #[test]
    fn migrate_old_home_handles_bad_port_in_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");
        std::fs::create_dir_all(old.join("run")).unwrap();
        // pc_port out of u16 range — must NOT panic.
        std::fs::write(
            old.join("run").join("state.json"),
            r#"{"agents":[],"socket_path":"","started_at":"x","pc_port":99999,"pc_api_token":null}"#,
        )
        .unwrap();

        // Should not panic; should fall through to rename (no PC running on a parseable port).
        let result = migrate_old_home(&old, &new);
        assert!(result.is_ok(), "must not panic on bad port: {result:?}");
        assert!(!old.exists(), "rename should have happened");
        assert!(new.exists());
    }

    #[test]
    fn migrate_old_home_handles_giant_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join(".rightclaw");
        let new = tmp.path().join(".right");
        std::fs::create_dir_all(old.join("run")).unwrap();
        // Write a >64KB file with no valid pc_port — must not OOM or hang.
        let giant = "x".repeat(128 * 1024);
        std::fs::write(old.join("run").join("state.json"), &giant).unwrap();

        let result = migrate_old_home(&old, &new);
        assert!(result.is_ok(), "must handle oversized state.json: {result:?}");
        assert!(!old.exists());
        assert!(new.exists());
    }
}
