use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use right_agent::agent::discover_agents;
use right_agent::init::validate_telegram_token;
use right_config::{TunnelConfig, read_global_config, write_global_config};

/// Every user-visible string from every prompt in the wizard — labels, Select
/// options, and static prefixes of dynamic-format prompts. This is the
/// source-of-truth list for the brand voice regression tests
/// (`wizard.rs::voice_pass`). When you add or change a prompt or option, update
/// this array — failing to do so is caught by tests.
#[allow(dead_code)]
pub(crate) const PROMPT_LABELS: &[&str] = &[
    // tunnel_setup: hostname
    "tunnel hostname (e.g. right.example.com):",
    "repair configured tunnel now?",
    // handle_existing_tunnel — label + TunnelExistingAction::Display options
    "existing tunnel — choose:",
    "reuse",
    "rename",
    "delete and recreate",
    "new tunnel name:",
    "delete tunnel \"", // dynamic prefix
    // telegram_setup
    "telegram bot token (keeping ", // dynamic prefix
    "telegram bot token (required — get one from @BotFather):",
    "telegram bot token (enter to skip):",
    // chat_ids_setup
    "your telegram user id (required — /start @userinfobot to find it):",
    "your telegram user id (/start @userinfobot to find it, empty to skip):",
    // config_setup / agent_setting_menu top-level — label + CombinedMenuItem::Display prefixes
    "settings:",
    "tunnel name:",
    "select agent:",
    "agent: ", // dynamic prefix (CombinedMenuItem::Agent)
    "done",
    // agent_setting_menu sub-prompts
    "model (e.g. sonnet, opus, haiku — empty to clear):",
    "allowed chat ids (comma-separated, empty to clear):",
    // learning_setup prompts
    "max daily budget usd:",
    "enable prefilter?",
    "prefilter model (empty for default haiku):",
    "enable probe-writer?",
    "probe-writer model (empty to inherit agent model):",
    "enable curator?",
    "curator model (empty to inherit agent model):",
    "curator interval hours:",
    "curator min idle hours:",
    "curator stale after days:",
    "curator archive after days:",
    "curator circuit: consecutive failures before the circuit opens:",
    "curator circuit: cooldown hours while open:",
    "curator mode (apply | report_only):",
    // network policy — label + options
    "network policy:",
    "restrictive — anthropic/claude domains only (recommended)",
    "permissive — all https domains (needed for external mcp servers)",
    // memory_setup
    "switching memory provider does not migrate existing memory. continue?",
    // hindsight api key source — label + options
    "hindsight api key source:",
    "use HINDSIGHT_API_KEY env var (recommended)",
    "enter a key to save in agent.yaml",
    "hindsight api key:",
    "hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime):",
    "hindsight rejected the key (http ", // dynamic prefix
    "save anyway?",
    // stt / ffmpeg
    "ffmpeg required for voice transcription. install via brew?",
    "enable voice transcription?",
    // whisper model — label + options
    "whisper model:",
    "tiny     — ~75 MB,   fastest, OK for short commands",
    "base     — ~150 MB,  decent",
    "small    — ~470 MB,  recommended (default)",
    "medium   — ~1.5 GB,  very good",
    "large-v3 — ~3.0 GB,  best quality, slow",
];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What to do when a named tunnel already exists in the Cloudflare account.
enum TunnelExistingAction {
    Reuse,
    Rename,
    DeleteAndRecreate,
}

impl fmt::Display for TunnelExistingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reuse => write!(f, "reuse"),
            Self::Rename => write!(f, "rename"),
            Self::DeleteAndRecreate => write!(f, "delete and recreate"),
        }
    }
}

/// A single entry from `cloudflared tunnel list -o json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct TunnelListEntry {
    id: String,
    name: String,
}

// ---------------------------------------------------------------------------
// Cloudflared CLI helpers (private)
// ---------------------------------------------------------------------------

/// Find an existing tunnel by name via cloudflared CLI.
fn find_tunnel_by_name(cf_bin: &Path, name: &str) -> miette::Result<Option<TunnelListEntry>> {
    let output = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "list", "-o", "json"])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel list: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel list failed: {stderr}"));
    }

    let tunnels: Vec<TunnelListEntry> = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("failed to parse cloudflared tunnel list JSON: {e:#}"))?;

    Ok(tunnels.into_iter().find(|t| t.name == name))
}

/// Create a new named tunnel via cloudflared CLI.
fn create_tunnel(cf_bin: &Path, name: &str) -> miette::Result<TunnelListEntry> {
    let output = Command::new(cf_bin)
        .args([
            "tunnel",
            "--loglevel",
            "error",
            "create",
            "-o",
            "json",
            name,
        ])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel create: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "cloudflared tunnel create failed: {stderr}"
        ));
    }

    let entry: TunnelListEntry = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("failed to parse cloudflared tunnel create JSON: {e:#}"))?;

    Ok(entry)
}

/// Delete a named tunnel via cloudflared CLI (cleanup connections first).
fn delete_tunnel(cf_bin: &Path, name: &str) -> miette::Result<()> {
    // Cleanup active connections first (best-effort).
    let _ = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "cleanup", name])
        .output();

    let output = Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "delete", name])
        .output()
        .map_err(|e| miette::miette!("failed to run cloudflared tunnel delete: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "cloudflared tunnel delete failed: {stderr}"
        ));
    }

    Ok(())
}

/// Route a DNS CNAME record for the tunnel. Non-fatal: logs a warning on failure.
fn route_dns(cf_bin: &Path, uuid: &str, hostname: &str) {
    let result = Command::new(cf_bin)
        .args([
            "tunnel",
            "--loglevel",
            "error",
            "route",
            "dns",
            "--overwrite-dns",
            uuid,
            hostname,
        ])
        .output();

    match result {
        Ok(output) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("cloudflared route dns failed (non-fatal): {stderr}");
        }
        Err(e) => {
            tracing::warn!("cloudflared route dns failed (non-fatal): {e:#}");
        }
        _ => {}
    }
}

trait TunnelRepairExecutor {
    fn list(&mut self) -> miette::Result<Vec<TunnelListEntry>>;
    fn create(&mut self, name: &str) -> miette::Result<TunnelListEntry>;
    fn route_dns(&mut self, uuid: &str, hostname: &str) -> miette::Result<()>;
    fn delete(&mut self, uuid: &str) -> miette::Result<()>;
    fn credentials_path(&self, uuid: &str) -> miette::Result<PathBuf>;
}

struct CloudflaredTunnelRepair {
    binary: PathBuf,
}

impl TunnelRepairExecutor for CloudflaredTunnelRepair {
    fn list(&mut self) -> miette::Result<Vec<TunnelListEntry>> {
        let output = Command::new(&self.binary)
            .args(["tunnel", "--loglevel", "error", "list", "-o", "json"])
            .output()
            .map_err(|error| miette::miette!("failed to run cloudflared tunnel list: {error:#}"))?;
        if !output.status.success() {
            return Err(miette::miette!(
                "cloudflared tunnel list failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| {
            miette::miette!("failed to parse cloudflared tunnel list JSON: {error:#}")
        })
    }

    fn create(&mut self, name: &str) -> miette::Result<TunnelListEntry> {
        create_tunnel(&self.binary, name)
    }

    fn route_dns(&mut self, uuid: &str, hostname: &str) -> miette::Result<()> {
        let output = Command::new(&self.binary)
            .args([
                "tunnel",
                "--loglevel",
                "error",
                "route",
                "dns",
                "--overwrite-dns",
                uuid,
                hostname,
            ])
            .output()
            .map_err(|error| miette::miette!("failed to run cloudflared route dns: {error:#}"))?;
        if !output.status.success() {
            return Err(miette::miette!(
                "cloudflared route dns failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn delete(&mut self, uuid: &str) -> miette::Result<()> {
        delete_tunnel(&self.binary, uuid)
    }

    fn credentials_path(&self, uuid: &str) -> miette::Result<PathBuf> {
        cloudflared_credentials_path(uuid)
    }
}

fn tunnel_account_tag(credentials: &Path, expected_uuid: &str) -> miette::Result<String> {
    let content = std::fs::read(credentials).map_err(|error| {
        miette::miette!(
            "read tunnel credentials {}: {error:#}",
            credentials.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_slice(&content).map_err(|error| {
        miette::miette!(
            "parse tunnel credentials {}: {error:#}",
            credentials.display()
        )
    })?;
    let tunnel_id = value.get("TunnelID").and_then(serde_json::Value::as_str);
    if tunnel_id != Some(expected_uuid) {
        return Err(miette::miette!(
            "replacement credentials TunnelID does not match created tunnel {expected_uuid}"
        ));
    }
    value
        .get("AccountTag")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| miette::miette!("replacement credentials are missing AccountTag"))
}

trait TunnelConfigCutover {
    fn stage(&mut self, home: &Path, config: &right_config::GlobalConfig) -> miette::Result<()>;
    fn commit(&mut self, home: &Path) -> miette::Result<()>;
    fn cleanup(&mut self) -> miette::Result<()>;
    fn pending_path(&self) -> Option<&Path>;
}

struct FilesystemTunnelConfigCutover {
    staging: PathBuf,
    pending_target: PathBuf,
    pending: Option<PathBuf>,
}

impl FilesystemTunnelConfigCutover {
    fn new(home: &Path) -> Self {
        let suffix = std::process::id();
        Self {
            staging: home.join(format!(".tunnel-repair-{suffix}")),
            pending_target: home.join(format!(".config-tunnel-repair-{suffix}.yaml")),
            pending: None,
        }
    }
}

impl TunnelConfigCutover for FilesystemTunnelConfigCutover {
    fn stage(&mut self, _home: &Path, config: &right_config::GlobalConfig) -> miette::Result<()> {
        std::fs::create_dir(&self.staging).map_err(|error| {
            miette::miette!("create tunnel repair staging directory: {error:#}")
        })?;
        write_global_config(&self.staging, config)?;
        std::fs::rename(self.staging.join("config.yaml"), &self.pending_target)
            .map_err(|error| miette::miette!("stage replacement config.yaml: {error:#}"))?;
        self.pending = Some(self.pending_target.clone());
        std::fs::remove_dir(&self.staging).map_err(|error| {
            miette::miette!("remove tunnel repair staging directory: {error:#}")
        })?;
        Ok(())
    }

    fn commit(&mut self, home: &Path) -> miette::Result<()> {
        let pending = self
            .pending
            .as_ref()
            .ok_or_else(|| miette::miette!("replacement config was not staged"))?;
        std::fs::rename(pending, home.join("config.yaml"))
            .map_err(|error| miette::miette!("atomically replace config.yaml: {error:#}"))?;
        self.pending = None;
        Ok(())
    }

    fn cleanup(&mut self) -> miette::Result<()> {
        let mut failures = Vec::new();
        if let Some(pending) = self.pending.as_ref() {
            match std::fs::remove_file(pending) {
                Ok(()) => self.pending = None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.pending = None,
                Err(error) => failures.push(format!(
                    "remove pending config {}: {error:#}",
                    pending.display()
                )),
            }
        }
        match std::fs::remove_dir_all(&self.staging) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!(
                "remove staging directory {}: {error:#}",
                self.staging.display()
            )),
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(failures.join("; ")))
        }
    }

    fn pending_path(&self) -> Option<&Path> {
        self.pending.as_deref()
    }
}

fn cleanup_inactive_replacement<E: TunnelRepairExecutor, C: TunnelConfigCutover>(
    executor: &mut E,
    replacement_uuid: &str,
    cutover: &mut C,
    primary: miette::Report,
) -> miette::Report {
    let pending_cleanup = cutover.cleanup().err();
    let tunnel_cleanup = executor.delete(replacement_uuid).err();
    if pending_cleanup.is_none() && tunnel_cleanup.is_none() {
        return primary;
    }
    let mut message = format!("{primary:#}");
    if let Some(error) = pending_cleanup {
        message.push_str(&format!("; failed to clean staged config: {error:#}"));
    }
    if let Some(error) = tunnel_cleanup {
        message.push_str(&format!(
            "; failed to delete inactive replacement tunnel: {error:#}"
        ));
    }
    miette::miette!(message)
}

fn execute_tunnel_replacement<E: TunnelRepairExecutor>(
    home: &Path,
    current: &right_config::GlobalConfig,
    executor: &mut E,
) -> miette::Result<()> {
    execute_tunnel_replacement_with_cutover(
        home,
        current,
        executor,
        &mut FilesystemTunnelConfigCutover::new(home),
    )
}

fn execute_tunnel_replacement_with_cutover<E: TunnelRepairExecutor, C: TunnelConfigCutover>(
    home: &Path,
    current: &right_config::GlobalConfig,
    executor: &mut E,
    cutover: &mut C,
) -> miette::Result<()> {
    let right_config::TunnelProvider::Cloudflared {
        tunnel_uuid: configured_uuid,
        credentials_file: configured_credentials,
    } = &current.tunnel.provider
    else {
        return Err(miette::miette!(
            "external tunnels are operator-managed and cannot be repaired"
        ));
    };
    let account_tag = tunnel_account_tag(configured_credentials, configured_uuid)?;
    let configured = executor
        .list()?
        .into_iter()
        .find(|entry| entry.id == *configured_uuid);
    let replacement_name = format!(
        "{}-repair-{}",
        configured
            .as_ref()
            .map_or("right", |entry| entry.name.as_str()),
        std::process::id()
    );
    let replacement = executor.create(&replacement_name)?;
    if replacement.name != replacement_name {
        let primary = miette::miette!("cloudflared created an unexpected tunnel name");
        let cleanup = executor.delete(&replacement.id);
        return match cleanup {
            Ok(()) => Err(primary),
            Err(error) => Err(miette::miette!(
                "{primary:#}; failed to delete inactive replacement tunnel: {error:#}"
            )),
        };
    }

    let replacement_credentials = match executor.credentials_path(&replacement.id) {
        Ok(path) => path,
        Err(primary) => {
            return Err(cleanup_inactive_replacement(
                executor,
                &replacement.id,
                cutover,
                primary,
            ));
        }
    };
    let replacement_account = match tunnel_account_tag(&replacement_credentials, &replacement.id) {
        Ok(account) => account,
        Err(primary) => {
            return Err(cleanup_inactive_replacement(
                executor,
                &replacement.id,
                cutover,
                primary,
            ));
        }
    };
    if replacement_account != account_tag {
        return Err(cleanup_inactive_replacement(
            executor,
            &replacement.id,
            cutover,
            miette::miette!("replacement tunnel belongs to a different Cloudflare account"),
        ));
    }
    let repaired = right_config::GlobalConfig {
        tunnel: TunnelConfig {
            hostname: current.tunnel.hostname.clone(),
            provider: right_config::TunnelProvider::Cloudflared {
                tunnel_uuid: replacement.id.clone(),
                credentials_file: replacement_credentials,
            },
        },
        aggregator: current.aggregator.clone(),
    };
    if let Err(primary) = cutover.stage(home, &repaired) {
        return Err(cleanup_inactive_replacement(
            executor,
            &replacement.id,
            cutover,
            primary,
        ));
    }
    if let Err(primary) = executor.route_dns(&replacement.id, &current.tunnel.hostname) {
        return Err(cleanup_inactive_replacement(
            executor,
            &replacement.id,
            cutover,
            primary,
        ));
    }
    if let Err(commit_error) = cutover.commit(home) {
        let Some(configured) = configured.as_ref() else {
            let pending = cutover
                .pending_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unavailable".to_owned());
            return Err(miette::miette!(
                "config commit failed after DNS routed to replacement: {commit_error:#}; \
                 configured tunnel {configured_uuid} no longer exists, so DNS cannot be rolled back; \
                 replacement tunnel {} retained as the active DNS target; pending replacement config retained at {pending} for operator recovery",
                replacement.id
            ));
        };
        if let Err(rollback_error) = executor.route_dns(&configured.id, &current.tunnel.hostname) {
            let pending = cutover
                .pending_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unavailable".to_owned());
            return Err(miette::miette!(
                "config commit failed after DNS routed to replacement: {commit_error:#}; \
                 DNS rollback to configured tunnel {configured_uuid} failed: {rollback_error:#}; \
                 replacement tunnel {} retained as the active DNS target; pending replacement config retained at {pending} for operator recovery",
                replacement.id
            ));
        }
        return Err(cleanup_inactive_replacement(
            executor,
            &replacement.id,
            cutover,
            commit_error,
        ));
    }
    if let Some(configured) = configured {
        executor.delete(&configured.id)?;
    }
    Ok(())
}

/// Check whether the cloudflared login certificate exists at `~/.cloudflared/cert.pem`.
fn detect_cloudflared_cert() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".cloudflared").join("cert.pem").exists())
        .unwrap_or(false)
}

/// Return the path to the cloudflared credentials file for a given tunnel UUID.
fn cloudflared_credentials_path(uuid: &str) -> miette::Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    Ok(home.join(".cloudflared").join(format!("{uuid}.json")))
}

// ---------------------------------------------------------------------------
// Tunnel setup (public)
// ---------------------------------------------------------------------------

/// Run the interactive (or non-interactive) tunnel setup flow.
///
/// Returns the configured `TunnelConfig`. Cloudflare Tunnel is mandatory; this
/// function errors if no cloudflared certificate is available.
pub fn tunnel_setup(
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    interactive: bool,
) -> miette::Result<TunnelConfig> {
    if !detect_cloudflared_cert() {
        return Err(miette::miette!(
            help = "run: cloudflared tunnel login",
            "no cloudflared certificate found at ~/.cloudflared/cert.pem"
        ));
    }

    let cf_bin = which::which("cloudflared")
        .map_err(|_| miette::miette!("cloudflared binary not found in PATH"))?;

    let existing = find_tunnel_by_name(&cf_bin, tunnel_name)?;

    let uuid = match existing {
        Some(entry) => {
            let has_local_creds = cloudflared_credentials_path(&entry.id)
                .map(|p| p.exists())
                .unwrap_or(false);

            if interactive {
                handle_existing_tunnel(&cf_bin, &entry, tunnel_name, has_local_creds)?
            } else if has_local_creds {
                // Non-interactive: silently reuse when credentials are present.
                entry.id
            } else {
                // Non-interactive: cannot reuse without local credentials.
                // Delete and recreate so cloudflared can actually start.
                tracing::warn!(
                    "Tunnel '{}' exists but credentials file is missing locally — recreating",
                    entry.name
                );
                delete_tunnel(&cf_bin, &entry.name)?;
                let fresh = create_tunnel(&cf_bin, tunnel_name)?;
                let theme = right_ui::detect();
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Ok)
                        .noun("tunnel")
                        .verb("recreated")
                        .detail(fresh.name.as_str())
                        .render(theme)
                );
                fresh.id
            }
        }
        None => {
            let entry = create_tunnel(&cf_bin, tunnel_name)?;
            let theme = right_ui::detect();
            println!(
                "{}",
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            entry.id
        }
    };

    // Resolve hostname.
    let hostname = if let Some(h) = tunnel_hostname {
        h.to_string()
    } else if interactive {
        let input = inquire::Text::new("tunnel hostname (e.g. right.example.com):")
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        input.trim().to_string()
    } else {
        return Err(miette::miette!(
            "tunnel hostname is required in non-interactive mode (use --tunnel-hostname)"
        ));
    };

    // Validate: must be a bare domain, not a URL.
    if hostname.starts_with("https://") || hostname.starts_with("http://") {
        return Err(miette::miette!(
            help = "use just the domain, e.g. right.example.com",
            "hostname must be a bare domain, not a url"
        ));
    }

    if hostname.is_empty() {
        return Err(miette::miette!("tunnel hostname cannot be empty"));
    }

    // Route DNS (non-fatal).
    route_dns(&cf_bin, &uuid, &hostname);

    // Verify credentials file exists — this should always pass after the
    // checks above, but guard against unexpected state.
    let credentials_file = cloudflared_credentials_path(&uuid)?;
    if !credentials_file.exists() {
        return Err(miette::miette!(
            help = "Run `right config set` and select Tunnel to reconfigure",
            "tunnel credentials missing at {} — cloudflared cannot start",
            credentials_file.display()
        ));
    }

    Ok(TunnelConfig {
        hostname,
        provider: right_config::TunnelProvider::Cloudflared {
            tunnel_uuid: uuid,
            credentials_file,
        },
    })
}

/// Configure the tunnel for `provider: external` — the operator owns the
/// public TLS front (e.g. their own caddy/nginx). The only thing we need
/// from them is the public hostname; cloudflared is never touched.
pub fn external_tunnel_setup(
    tunnel_hostname: Option<&str>,
    interactive: bool,
) -> miette::Result<TunnelConfig> {
    let hostname = if let Some(h) = tunnel_hostname {
        h.trim().to_string()
    } else if interactive {
        let input = inquire::Text::new("tunnel hostname (e.g. right.example.com):")
            .with_help_message(
                "hostname the bot tells Telegram for webhooks; your reverse proxy \
                 must terminate TLS here and forward to bot.sock per agent",
            )
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        input.trim().to_string()
    } else {
        return Err(miette::miette!(
            "tunnel hostname is required in non-interactive mode (use --tunnel-hostname)"
        ));
    };

    if hostname.starts_with("https://") || hostname.starts_with("http://") {
        return Err(miette::miette!(
            help = "use just the domain, e.g. right.example.com",
            "hostname must be a bare domain, not a url"
        ));
    }
    if hostname.is_empty() {
        return Err(miette::miette!("tunnel hostname cannot be empty"));
    }

    Ok(TunnelConfig {
        hostname,
        provider: right_config::TunnelProvider::External,
    })
}

/// Interactively replace a broken Right-owned tunnel while retaining the public hostname
/// and unrelated global settings. Agent state is never touched.
pub fn repair_configured_tunnel(
    home: &Path,
    current: &right_config::GlobalConfig,
) -> miette::Result<()> {
    let repair = inquire::Confirm::new("repair configured tunnel now?")
        .with_default(true)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
    if !repair {
        return Err(miette::miette!("tunnel repair declined"));
    }
    let binary = which::which("cloudflared")
        .map_err(|_| miette::miette!("cloudflared binary not found in PATH"))?;
    execute_tunnel_replacement(home, current, &mut CloudflaredTunnelRepair { binary })
}

/// Replace only the top-level Telegram token with a safely quoted YAML scalar.
pub fn set_agent_telegram_token(path: &Path, token: &str) -> miette::Result<()> {
    validate_telegram_token(token)?;
    let quoted = serde_json::to_string(token)
        .map_err(|e| miette::miette!("serialize Telegram token: {e:#}"))?;
    update_agent_yaml_field(path, "telegram_token", &quoted)
}

// ---------------------------------------------------------------------------
// Handle existing tunnel (interactive)
// ---------------------------------------------------------------------------

/// Prompt the user to decide what to do with an existing tunnel.
fn handle_existing_tunnel(
    cf_bin: &Path,
    existing: &TunnelListEntry,
    original_name: &str,
    has_local_creds: bool,
) -> miette::Result<String> {
    let short_uuid = if existing.id.len() > 8 {
        &existing.id[..8]
    } else {
        &existing.id
    };
    let theme = right_ui::detect();
    println!(
        "{}",
        right_ui::status(right_ui::Glyph::Warn)
            .noun("tunnel")
            .verb(format!("found \"{}\"", existing.name))
            .detail(format!("{}…", short_uuid))
            .render(theme)
    );

    if !has_local_creds {
        println!(
            "{}    note: credentials file missing on this machine. choose \"delete and recreate\" to regenerate.",
            right_ui::Rail::blank(theme)
        );
    }

    let mut options = Vec::new();
    if has_local_creds {
        options.push(TunnelExistingAction::Reuse);
    }
    options.push(TunnelExistingAction::DeleteAndRecreate);
    options.push(TunnelExistingAction::Rename);

    let selection = inquire::Select::new("existing tunnel — choose:", options)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

    match selection {
        TunnelExistingAction::Reuse => Ok(existing.id.clone()),

        TunnelExistingAction::Rename => {
            let new_name = inquire::Text::new("new tunnel name:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return Err(miette::miette!("tunnel name cannot be empty"));
            }
            let entry = create_tunnel(cf_bin, new_name)?;
            let theme = right_ui::detect();
            println!(
                "{}",
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            Ok(entry.id)
        }

        TunnelExistingAction::DeleteAndRecreate => {
            let confirmed =
                inquire::Confirm::new(&format!("delete tunnel \"{}\" permanently?", existing.name))
                    .with_default(false)
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

            if !confirmed {
                return Err(miette::miette!("cancelled"));
            }

            delete_tunnel(cf_bin, &existing.name)?;
            println!(
                "{}",
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("deleted")
                    .detail(existing.name.as_str())
                    .render(theme)
            );

            let entry = create_tunnel(cf_bin, original_name)?;
            println!(
                "{}",
                right_ui::status(right_ui::Glyph::Ok)
                    .noun("tunnel")
                    .verb("created")
                    .detail(entry.name.as_str())
                    .render(theme)
            );
            Ok(entry.id)
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram setup (public)
// ---------------------------------------------------------------------------

/// Outcome of `telegram_setup`. Distinguishes "back" (Esc) from "skip"
/// (Enter on empty when `!required`) so the caller can route navigation
/// vs. value separately.
#[derive(Debug, Clone)]
pub enum TelegramSetupOutcome {
    /// User entered a valid token.
    Token(String),
    /// User pressed Enter on empty (only possible when `!required`).
    Skipped,
    /// User pressed Esc — caller should navigate back.
    Back,
}

/// Run the interactive Telegram bot token setup flow.
///
/// When `required` is true and there is no existing token, an empty input
/// is rejected and the prompt re-runs until a valid token is entered. Esc
/// always navigates back; Ctrl+C triggers a "Cancel setup?" confirm via
/// `inquire_back` and propagates as `Err` if confirmed.
pub fn telegram_setup(
    existing_token: Option<&str>,
    interactive: bool,
    required: bool,
) -> miette::Result<TelegramSetupOutcome> {
    if !interactive {
        return Ok(TelegramSetupOutcome::Skipped);
    }

    let prompt_msg = if let Some(token) = existing_token {
        let masked = if token.len() > 8 {
            format!("{}...{}", &token[..4], &token[token.len() - 4..])
        } else {
            "****".to_string()
        };
        format!("telegram bot token (keeping {masked} — enter new or press enter to keep):")
    } else if required {
        "telegram bot token (required — get one from @BotFather):".to_string()
    } else {
        "telegram bot token (enter to skip):".to_string()
    };

    loop {
        let Some(input) =
            right_agent::init::inquire_back(|| inquire::Text::new(&prompt_msg).prompt())?
        else {
            return Ok(TelegramSetupOutcome::Back);
        };

        let trimmed = input.trim();

        if trimmed.is_empty() {
            if required && existing_token.is_none() {
                eprintln!(
                    "  a token is required. create a bot via @BotFather, paste the token here. esc to go back."
                );
                continue;
            }
            return Ok(match existing_token {
                Some(t) => TelegramSetupOutcome::Token(t.to_string()),
                None => TelegramSetupOutcome::Skipped,
            });
        }

        match validate_telegram_token(trimmed) {
            Ok(()) => return Ok(TelegramSetupOutcome::Token(trimmed.to_string())),
            Err(e) if required => {
                let theme = right_ui::detect();
                eprintln!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("invalid")
                        .verb(format!("{e:#}"))
                        .render(theme)
                );
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Chat ID setup (public)
// ---------------------------------------------------------------------------

/// Prompt for Telegram chat IDs during init.
///
/// When `required` is true, an empty input is rejected and an unparseable
/// entry re-prompts (instead of erroring out). Returns `Ok(Some(ids))`
/// (possibly empty when `!required`) on submit; `Ok(None)` when the user
/// navigates back (Esc); `Err` on confirmed cancel (Ctrl+C).
pub fn chat_ids_setup(required: bool) -> miette::Result<Option<Vec<i64>>> {
    let prompt_text = if required {
        "your telegram user id (required — /start @userinfobot to find it):"
    } else {
        "your telegram user id (/start @userinfobot to find it, empty to skip):"
    };

    loop {
        let Some(input) =
            right_agent::init::inquire_back(|| inquire::Text::new(prompt_text).prompt())?
        else {
            return Ok(None);
        };

        let trimmed = input.trim();
        if trimmed.is_empty() {
            if required {
                eprintln!(
                    "  at least one chat id is required so the bot knows who can talk to it. /start @userinfobot for your numeric id. esc to go back."
                );
                continue;
            }
            return Ok(Some(vec![]));
        }

        let parsed: Result<Vec<i64>, _> = trimmed
            .split(',')
            .map(|s| {
                s.trim()
                    .parse::<i64>()
                    .map_err(|e| miette::miette!("invalid chat id \"{}\": {e}", s.trim()))
            })
            .collect();

        match parsed {
            Ok(ids) => return Ok(Some(ids)),
            Err(e) if required => {
                let theme = right_ui::detect();
                eprintln!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("invalid")
                        .verb(format!("{e:#}"))
                        .render(theme)
                );
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Combined settings menu (public)
// ---------------------------------------------------------------------------

/// Menu items for the combined (global + per-agent) settings menu.
enum CombinedMenuItem {
    Tunnel(String),
    Agent(String),
    Done,
}

impl fmt::Display for CombinedMenuItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tunnel(display) => write!(f, "{display}"),
            Self::Agent(name) => write!(f, "agent: {name}"),
            Self::Done => write!(f, "done"),
        }
    }
}

/// Interactive menu showing both global settings and per-agent settings.
///
/// Bare `right config` (no subcommand) launches this.
pub async fn combined_setting_menu(home: &Path) -> miette::Result<()> {
    loop {
        let config = read_global_config(home)?;

        let tunnel_label = match &config.tunnel.provider {
            right_config::TunnelProvider::Cloudflared { tunnel_uuid, .. } => {
                let short = &tunnel_uuid[..8.min(tunnel_uuid.len())];
                format!("tunnel: {} (cloudflared {short})", config.tunnel.hostname)
            }
            right_config::TunnelProvider::External => {
                format!("tunnel: {} (external)", config.tunnel.hostname)
            }
        };

        let agents_dir = right_config::agents_dir(home);
        let agents = if agents_dir.exists() {
            discover_agents(&agents_dir).unwrap_or_default()
        } else {
            vec![]
        };

        let mut options: Vec<CombinedMenuItem> = vec![CombinedMenuItem::Tunnel(tunnel_label)];
        for agent in &agents {
            options.push(CombinedMenuItem::Agent(agent.name.clone()));
        }
        options.push(CombinedMenuItem::Done);

        let selection = inquire::Select::new("settings:", options)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        match selection {
            CombinedMenuItem::Done => break,
            CombinedMenuItem::Tunnel(_) => {
                // Edit the tunnel in-place for whichever provider is already
                // configured. Without this branch, an `external` operator
                // selecting the tunnel row would be silently switched to
                // cloudflared (and hard-error on a host with no cloudflared
                // certificate).
                let result = match &config.tunnel.provider {
                    right_config::TunnelProvider::Cloudflared { .. } => {
                        let tunnel_name = inquire::Text::new("tunnel name:")
                            .with_default("right")
                            .prompt()
                            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                        tunnel_setup(tunnel_name.trim(), None, true)?
                    }
                    right_config::TunnelProvider::External => external_tunnel_setup(None, true)?,
                };

                let mut new_config = read_global_config(home)?;
                new_config.tunnel = result;
                write_global_config(home, &new_config)?;
                let theme = right_ui::detect();
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Ok)
                        .noun("tunnel")
                        .verb("saved")
                        .render(theme)
                );
            }
            CombinedMenuItem::Agent(name) => {
                let _ = agent_setting_menu(home, Some(&name)).await?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Agent settings menu (public)
// ---------------------------------------------------------------------------

/// Interactive menu for editing per-agent settings.
///
/// If `agent_name` is `None`, presents a picker to choose from discovered agents.
/// Returns the chosen agent name so callers can act on it (e.g. sandbox migration).
pub async fn agent_setting_menu(home: &Path, agent_name: Option<&str>) -> miette::Result<String> {
    let agents_dir = right_config::agents_dir(home);

    let chosen_name = match agent_name {
        Some(name) => name.to_string(),
        None => {
            let agents = discover_agents(&agents_dir)?;
            if agents.is_empty() {
                return Err(miette::miette!(
                    "no agents found in {}",
                    agents_dir.display()
                ));
            }
            let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
            inquire::Select::new("select agent:", names)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?
        }
    };

    let agent_yaml_path = agents_dir.join(&chosen_name).join("agent.yaml");
    if !agent_yaml_path.exists() {
        return Err(miette::miette!(
            "agent.yaml not found at {}",
            agent_yaml_path.display()
        ));
    }

    loop {
        let content = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("read agent.yaml: {e:#}"))?;
        let config: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content)
            .map_err(|e| miette::miette!("parse agent.yaml: {e:#}"))?;

        let token_display = match &config.telegram_token {
            Some(t) if t.len() > 8 => format!("{}...{}", &t[..4], &t[t.len() - 4..]),
            Some(_) => "****".to_string(),
            None => "(not set)".to_string(),
        };
        let model_display = config.model.as_deref().unwrap_or("(default)");
        let chat_ids_display = if config.allowed_chat_ids.is_empty() {
            "(none — blocks all)".to_string()
        } else {
            config
                .allowed_chat_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let network_policy_display = format!("{}", config.network_policy);
        let memory_display = format_memory_display(&config.memory, &chosen_name);
        let learning_display = format_learning_display(&config.learning);

        let opt_token = format!("telegram token: {token_display}");
        let opt_model = format!("model: {model_display}");
        let opt_chat_ids = format!("allowed chat ids: {chat_ids_display}");
        let opt_network_policy = format!("network policy: {network_policy_display}");
        let opt_memory = format!("memory: {memory_display}");
        let opt_learning = format!("learning: {learning_display}");
        let stt_display = if config.stt.enabled {
            format!("on ({})", config.stt.model.yaml_str())
        } else {
            "off".to_string()
        };
        let opt_stt = format!("stt: {stt_display}");
        let opt_done = "done".to_string();

        let mut options = vec![
            opt_token.clone(),
            opt_model.clone(),
            opt_chat_ids.clone(),
            opt_network_policy.clone(),
        ];
        options.push(opt_stt.clone());
        options.push(opt_memory.clone());
        options.push(opt_learning.clone());
        options.push(opt_done.clone());

        let theme = right_ui::detect();
        println!(
            "{}",
            right_ui::section(theme, &format!("agent: {chosen_name}"))
        );
        println!("{}", right_ui::Rail::blank(theme));

        let selection = inquire::Select::new("settings:", options)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;

        if selection == opt_done {
            break;
        }

        let saved_noun: Option<&str> = if selection == opt_token {
            match telegram_setup(config.telegram_token.as_deref(), true, false)? {
                TelegramSetupOutcome::Token(token) => {
                    update_agent_yaml_field(
                        &agent_yaml_path,
                        "telegram_token",
                        &format!("\"{token}\""),
                    )?;
                    Some("telegram token")
                }
                TelegramSetupOutcome::Skipped | TelegramSetupOutcome::Back => {
                    // Enter on empty (keep existing) or Esc — no change.
                    None
                }
            }
        } else if selection == opt_model {
            let input = inquire::Text::new("model (e.g. sonnet, opus, haiku — empty to clear):")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                remove_agent_yaml_field(&agent_yaml_path, "model")?;
            } else {
                update_agent_yaml_field(&agent_yaml_path, "model", &format!("\"{trimmed}\""))?;
            }
            Some("model")
        } else if selection == opt_chat_ids {
            let input = inquire::Text::new("allowed chat ids (comma-separated, empty to clear):")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                update_agent_yaml_chat_ids(&agent_yaml_path, &[])?;
            } else {
                let ids: Vec<i64> = trimmed
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<i64>()
                            .map_err(|e| miette::miette!("invalid chat id \"{}\": {e}", s.trim()))
                    })
                    .collect::<miette::Result<Vec<_>>>()?;
                update_agent_yaml_chat_ids(&agent_yaml_path, &ids)?;
            }
            Some("allowed chat ids")
        } else if selection == opt_network_policy {
            let options = vec![
                "restrictive — anthropic/claude domains only (recommended)",
                "permissive — all https domains (needed for external mcp servers)",
            ];
            let choice = inquire::Select::new("network policy:", options)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let policy = if choice.starts_with("restrictive") {
                "restrictive"
            } else {
                "permissive"
            };
            update_agent_yaml_field(&agent_yaml_path, "network_policy", policy)?;
            Some("network policy")
        } else if selection == opt_memory {
            match memory_setup(config.memory.as_ref(), &chosen_name).await? {
                Some(new_cfg) => {
                    if matches!(
                        new_cfg.provider,
                        right_agent::agent::types::MemoryProvider::File
                    ) {
                        remove_agent_yaml_memory(&agent_yaml_path)?;
                    } else {
                        update_agent_yaml_memory(&agent_yaml_path, &new_cfg)?;
                    }
                    Some("memory")
                }
                None => {
                    // User cancelled — no change.
                    None
                }
            }
        } else if selection == opt_learning {
            match learning_setup(&config.learning)? {
                Some(new_cfg) => {
                    update_agent_yaml_learning(&agent_yaml_path, &new_cfg)?;
                    Some("learning")
                }
                None => {
                    // User cancelled — no change.
                    None
                }
            }
        } else if selection == opt_stt {
            match stt_setup()? {
                Some((enabled, model)) => {
                    let stt = right_agent::agent::types::SttConfig { enabled, model };
                    update_agent_yaml_stt(&agent_yaml_path, &stt)?;
                    Some("stt")
                }
                None => {
                    // User cancelled — no change.
                    None
                }
            }
        } else {
            unreachable!("agent_setting_menu: unknown selection {selection:?}");
        };

        if let Some(noun) = saved_noun {
            println!(
                "{}",
                right_ui::status(right_ui::Glyph::Ok)
                    .noun(noun)
                    .verb("saved")
                    .render(theme)
            );
        }
    }

    Ok(chosen_name)
}

/// Compact one-line summary of memory config for the settings menu.
fn format_memory_display(
    memory: &Option<right_agent::agent::types::MemoryConfig>,
    agent_name: &str,
) -> String {
    use right_agent::agent::types::MemoryProvider;
    match memory.as_ref().map(|m| &m.provider) {
        None | Some(MemoryProvider::File) => "file".to_string(),
        Some(MemoryProvider::Hindsight) => {
            let cfg = memory.as_ref().unwrap();
            let bank = cfg.bank_id.as_deref().unwrap_or(agent_name);
            format!("hindsight (bank: {bank}, budget: {})", cfg.recall_budget)
        }
    }
}

/// Compact one-line summary of learning config for the settings menu.
fn format_learning_display(learning: &right_agent::agent::types::LearningConfig) -> String {
    let prefilter = if learning.prefilter_enabled {
        "on"
    } else {
        "off"
    };
    let probe = if learning.probe_writer_enabled {
        "on"
    } else {
        "off"
    };
    let curator = if learning.curator_enabled {
        "on"
    } else {
        "off"
    };
    format!(
        "prefilter:{prefilter} probe:{probe} curator:{curator} daily:${}",
        learning.max_daily_budget_usd
    )
}

/// Interactive learning config submenu. Empty model fields inherit the agent
/// model; empty numeric fields keep their current values.
fn learning_setup(
    existing: &right_agent::agent::types::LearningConfig,
) -> miette::Result<Option<right_agent::agent::types::LearningConfig>> {
    // max_daily_budget_usd
    let Some(budget_input) =
        right_agent::init::inquire_back(|| inquire::Text::new("max daily budget usd:").prompt())?
    else {
        return Ok(None);
    };
    let max_daily_budget_usd =
        parse_learning_budget_usd(budget_input.trim(), existing.max_daily_budget_usd)?;

    // prefilter_enabled
    let prefilter_default = if existing.prefilter_enabled {
        "yes"
    } else {
        "no"
    };
    let Some(prefilter_enabled_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("enable prefilter?")
            .with_default(prefilter_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let prefilter_enabled =
        parse_bool_yes_no(&prefilter_enabled_input).map_err(|e| miette::miette!(e))?;

    // prefilter_model
    let prefilter_model_default = existing
        .prefilter_model
        .as_deref()
        .unwrap_or("claude-haiku-4-5-20251001");
    let Some(prefilter_model_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("prefilter model (empty for default haiku):")
            .with_default(prefilter_model_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let prefilter_model =
        parse_optional_model_override(&prefilter_model_input).map_err(|e| miette::miette!(e))?;

    // probe_writer_enabled
    let probe_writer_default = if existing.probe_writer_enabled {
        "yes"
    } else {
        "no"
    };
    let Some(probe_writer_enabled_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("enable probe-writer?")
            .with_default(probe_writer_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let probe_writer_enabled =
        parse_bool_yes_no(&probe_writer_enabled_input).map_err(|e| miette::miette!(e))?;

    // probe_writer_model
    let Some(probe_writer_model_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("probe-writer model (empty to inherit agent model):").prompt()
    })?
    else {
        return Ok(None);
    };
    let probe_writer_model =
        parse_optional_model_override(&probe_writer_model_input).map_err(|e| miette::miette!(e))?;

    // curator_enabled
    let curator_default = if existing.curator_enabled {
        "yes"
    } else {
        "no"
    };
    let Some(curator_enabled_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("enable curator?")
            .with_default(curator_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_enabled =
        parse_bool_yes_no(&curator_enabled_input).map_err(|e| miette::miette!(e))?;

    // curator_model
    let Some(curator_model_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator model (empty to inherit agent model):").prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_model =
        parse_optional_model_override(&curator_model_input).map_err(|e| miette::miette!(e))?;

    // curator_interval_hours
    let Some(curator_interval_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator interval hours:")
            .with_default(&existing.curator_interval_hours.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_interval_hours = parse_u32_positive(
        curator_interval_input.trim(),
        existing.curator_interval_hours,
        "curator interval hours",
    )?;

    // curator_min_idle_hours
    let Some(curator_min_idle_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator min idle hours:")
            .with_default(&existing.curator_min_idle_hours.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_min_idle_hours = parse_u32_positive(
        curator_min_idle_input.trim(),
        existing.curator_min_idle_hours,
        "curator min idle hours",
    )?;

    // curator_stale_after_days
    let Some(curator_stale_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator stale after days:")
            .with_default(&existing.curator_stale_after_days.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_stale_after_days = parse_u32_positive(
        curator_stale_input.trim(),
        existing.curator_stale_after_days,
        "curator stale after days",
    )?;

    // curator_archive_after_days
    let Some(curator_archive_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator archive after days:")
            .with_default(&existing.curator_archive_after_days.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_archive_after_days = parse_u32_positive(
        curator_archive_input.trim(),
        existing.curator_archive_after_days,
        "curator archive after days",
    )?;

    // curator_cost_spike_k
    let Some(curator_cost_spike_k_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new(
            "curator cost-spike multiplier k (today's probe-writer cost >= k * baseline P50 → trigger):",
        )
        .with_default(&format!("{:.3}", existing.curator_cost_spike_k))
        .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_cost_spike_k = parse_positive_finite_f64(
        curator_cost_spike_k_input.trim(),
        existing.curator_cost_spike_k,
        "curator cost-spike multiplier k",
    )?;

    // curator_cost_spike_baseline_days
    let Some(curator_cost_spike_baseline_days_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator cost-spike baseline window (days):")
            .with_default(&existing.curator_cost_spike_baseline_days.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_cost_spike_baseline_days = parse_u32_positive(
        curator_cost_spike_baseline_days_input.trim(),
        existing.curator_cost_spike_baseline_days,
        "curator cost-spike baseline window days",
    )?;

    // curator_cost_spike_min_floor_usd
    let Some(curator_cost_spike_min_floor_usd_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new(
            "curator cost-spike absolute floor (USD; below this, spike never fires):",
        )
        .with_default(&format!("{:.3}", existing.curator_cost_spike_min_floor_usd))
        .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_cost_spike_min_floor_usd = parse_positive_finite_f64(
        curator_cost_spike_min_floor_usd_input.trim(),
        existing.curator_cost_spike_min_floor_usd,
        "curator cost-spike floor USD",
    )?;

    // curator_skill_change_threshold
    let Some(curator_skill_change_threshold_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new(
            "curator skill-change count threshold (new/patched skills since last run → trigger):",
        )
        .with_default(&existing.curator_skill_change_threshold.to_string())
        .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_skill_change_threshold = parse_u32_positive(
        curator_skill_change_threshold_input.trim(),
        existing.curator_skill_change_threshold,
        "curator skill-change count threshold",
    )?;

    // curator_min_cooldown_hours
    let Some(curator_min_cooldown_hours_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator minimum cooldown (hours; blocks ALL triggers):")
            .with_default(&existing.curator_min_cooldown_hours.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_min_cooldown_hours = parse_u32_positive(
        curator_min_cooldown_hours_input.trim(),
        existing.curator_min_cooldown_hours,
        "curator minimum cooldown hours",
    )?;

    // baseline_window_days
    let Some(baseline_window_days_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("prefilter baseline window (days):")
            .with_default(&existing.baseline_window_days.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let baseline_window_days = parse_u32_positive(
        baseline_window_days_input.trim(),
        existing.baseline_window_days,
        "prefilter baseline window days",
    )?;

    // baseline_min_sample
    let Some(baseline_min_sample_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("prefilter baseline minimum sample size:")
            .with_default(&existing.baseline_min_sample.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let baseline_min_sample = parse_u32_positive(
        baseline_min_sample_input.trim(),
        existing.baseline_min_sample,
        "prefilter baseline minimum sample size",
    )?;

    // curator_circuit_failure_threshold
    let Some(curator_circuit_failure_threshold_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator circuit: consecutive failures before the circuit opens:")
            .with_default(&existing.curator_circuit_failure_threshold.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_circuit_failure_threshold = parse_u32_positive(
        curator_circuit_failure_threshold_input.trim(),
        existing.curator_circuit_failure_threshold,
        "curator circuit failure threshold",
    )?;

    // curator_circuit_cooldown_hours
    let Some(curator_circuit_cooldown_hours_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator circuit: cooldown hours while open:")
            .with_default(&existing.curator_circuit_cooldown_hours.to_string())
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_circuit_cooldown_hours = parse_u32_positive(
        curator_circuit_cooldown_hours_input.trim(),
        existing.curator_circuit_cooldown_hours,
        "curator circuit cooldown hours",
    )?;

    // curator_mode
    let mode_default = match existing.curator_mode {
        right_agent::agent::types::CuratorMode::Apply => "apply",
        right_agent::agent::types::CuratorMode::ReportOnly => "report_only",
    };
    let Some(curator_mode_input) = right_agent::init::inquire_back(|| {
        inquire::Text::new("curator mode (apply | report_only):")
            .with_default(mode_default)
            .prompt()
    })?
    else {
        return Ok(None);
    };
    let curator_mode = match curator_mode_input.trim() {
        "report_only" => right_agent::agent::types::CuratorMode::ReportOnly,
        _ => right_agent::agent::types::CuratorMode::Apply,
    };

    Ok(Some(right_agent::agent::types::LearningConfig {
        prefilter_enabled,
        prefilter_model,
        probe_writer_enabled,
        probe_writer_model,
        curator_enabled,
        curator_model,
        curator_interval_hours,
        curator_min_idle_hours,
        curator_stale_after_days,
        curator_archive_after_days,
        curator_paused: existing.curator_paused,
        max_daily_budget_usd,
        curator_cost_spike_k,
        curator_cost_spike_baseline_days,
        curator_cost_spike_min_floor_usd,
        curator_skill_change_threshold,
        curator_min_cooldown_hours,
        curator_circuit_failure_threshold,
        curator_circuit_cooldown_hours,
        curator_mode,
        baseline_window_days,
        baseline_min_sample,
        // Deprecated fields — leave at None.
        fork_probe_enabled: None,
        fork_probe_model: None,
        legacy_probe_model: None,
        background_review_enabled: None,
        episode_selector_model: None,
        episode_selector_max_budget_usd: None,
        episode_settle_seconds: None,
        circuit_failure_threshold: None,
        circuit_cooldown_minutes: None,
    }))
}

fn parse_learning_budget_usd(input: &str, fallback: f64) -> miette::Result<f64> {
    if input.is_empty() {
        return Ok(fallback);
    }
    let value = input
        .parse::<f64>()
        .map_err(|e| miette::miette!("invalid learning budget \"{input}\": {e}"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(miette::miette!(
            "learning budget must be a positive finite number"
        ));
    }
    Ok(value)
}

pub(crate) fn parse_optional_model_override(input: &str) -> Result<Option<String>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

pub(crate) fn parse_bool_yes_no(input: &str) -> Result<bool, String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" | "on" => Ok(true),
        "no" | "n" | "false" | "off" => Ok(false),
        other => Err(format!("invalid boolean: '{other}'")),
    }
}

fn parse_u32_positive(input: &str, fallback: u32, label: &str) -> miette::Result<u32> {
    if input.is_empty() {
        return Ok(fallback);
    }
    let v: u32 = input
        .parse()
        .map_err(|e| miette::miette!("invalid {label}: {e}"))?;
    if v == 0 {
        return Err(miette::miette!("{label} must be > 0"));
    }
    Ok(v)
}

fn parse_positive_finite_f64(input: &str, fallback: f64, label: &str) -> miette::Result<f64> {
    if input.is_empty() {
        return Ok(fallback);
    }
    let v: f64 = input
        .parse()
        .map_err(|e| miette::miette!("invalid {label}: {e}"))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(miette::miette!("{label} must be a positive finite number"));
    }
    Ok(v)
}

/// Interactive memory config submenu. Returns `Ok(None)` when the user
/// cancels without committing a change.
///
/// On switch between providers, warns that memory does not migrate. On
/// Hindsight, optionally uses `HINDSIGHT_API_KEY` from the environment
/// and validates the key against `GET /v1/default/banks`.
async fn memory_setup(
    current: Option<&right_agent::agent::types::MemoryConfig>,
    agent_name: &str,
) -> miette::Result<Option<right_agent::agent::types::MemoryConfig>> {
    use right_agent::agent::types::{MemoryConfig, MemoryProvider};
    use right_agent::init::{
        DEFAULT_RECALL_BUDGET, DEFAULT_RECALL_MAX_TOKENS, ValidationResult,
        prompt_hindsight_bank_id, prompt_memory_provider, prompt_recall_budget,
        prompt_recall_max_tokens,
    };

    let current_provider = current
        .map(|m| m.provider.clone())
        .unwrap_or(MemoryProvider::File);

    let Some(new_provider) = prompt_memory_provider()? else {
        return Ok(None);
    };

    if new_provider != current_provider {
        let confirm = inquire::Confirm::new(
            "switching memory provider does not migrate existing memory. continue?",
        )
        .with_default(false)
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        if !confirm {
            return Ok(None);
        }
    }

    if matches!(new_provider, MemoryProvider::File) {
        return Ok(Some(MemoryConfig {
            provider: MemoryProvider::File,
            api_key: None,
            bank_id: None,
            recall_budget: DEFAULT_RECALL_BUDGET,
            recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
        }));
    }

    // Hindsight: api_key. If HINDSIGHT_API_KEY is set, offer to use it.
    let env_key = std::env::var("HINDSIGHT_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let api_key: Option<String> = if env_key.is_some() {
        let opts = vec![
            "use HINDSIGHT_API_KEY env var (recommended)",
            "enter a key to save in agent.yaml",
        ];
        let choice = inquire::Select::new("hindsight api key source:", opts)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        if choice.starts_with("use HINDSIGHT_API_KEY") {
            None
        } else {
            let input = inquire::Text::new("hindsight api key:")
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Some(trimmed.to_string())
        }
    } else {
        let input = inquire::Text::new(
            "hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime):",
        )
        .prompt()
        .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    let bank_id = match prompt_hindsight_bank_id(agent_name)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let budget = match prompt_recall_budget()? {
        Some(b) => b,
        None => return Ok(None),
    };
    let max_tokens = match prompt_recall_max_tokens()? {
        Some(t) => t,
        None => return Ok(None),
    };

    // Validation uses whatever key we can resolve right now:
    // - explicit key if user entered one
    // - HINDSIGHT_API_KEY env var as fallback
    // If neither is available, skip validation with a note.
    let resolved_key = api_key.clone().or_else(|| {
        std::env::var("HINDSIGHT_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    });

    if let Some(k) = resolved_key.as_deref() {
        let theme = right_ui::detect();
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Info)
                .noun("hindsight")
                .verb("validating key")
                .render(theme)
        );
        match right_agent::init::validate_hindsight_key(k).await {
            ValidationResult::Valid { banks } => {
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Ok)
                        .noun("hindsight")
                        .verb(format!("{banks} bank(s) accessible"))
                        .render(theme)
                );
            }
            ValidationResult::Invalid { status } => {
                let proceed = inquire::Confirm::new(&format!(
                    "hindsight rejected the key (http {status}). save anyway?"
                ))
                .with_default(false)
                .prompt()
                .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                if !proceed {
                    return Ok(None);
                }
            }
            ValidationResult::Unreachable { detail } => {
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("hindsight")
                        .verb(format!("unreachable ({detail})"))
                        .render(theme)
                );
                let proceed = inquire::Confirm::new("save anyway?")
                    .with_default(true)
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
                if !proceed {
                    return Ok(None);
                }
            }
        }
    } else {
        let theme = right_ui::detect();
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Warn)
                .noun("hindsight")
                .verb("no key — saving without validation")
                .render(theme)
        );
    }

    Ok(Some(MemoryConfig {
        provider: MemoryProvider::Hindsight,
        api_key,
        bank_id,
        recall_budget: budget,
        recall_max_tokens: max_tokens,
    }))
}

// ---------------------------------------------------------------------------
// STT setup helpers (public)
// ---------------------------------------------------------------------------

/// macOS: detect brew, prompt to install ffmpeg, run, re-check.
/// Linux: print install instructions only.
/// Returns true iff ffmpeg is in PATH after this call.
pub fn prompt_ffmpeg_install() -> miette::Result<bool> {
    if right_stt::ffmpeg_available() {
        return Ok(true);
    }

    match std::env::consts::OS {
        "macos" => {
            if which::which("brew").is_err() {
                println!("ffmpeg required, but Homebrew (brew) is not installed.");
                println!("Install Homebrew first: https://brew.sh");
                println!("Then run: brew install ffmpeg");
                return Ok(false);
            }
            let install =
                inquire::Confirm::new("ffmpeg required for voice transcription. install via brew?")
                    .with_default(true)
                    .prompt()
                    .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            let theme = right_ui::detect();
            if !install {
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("stt")
                        .verb("disabled (install ffmpeg: brew install ffmpeg)")
                        .render(theme)
                );
                return Ok(false);
            }
            // Spawn brew install with stdout/stderr inherited so user sees output.
            let status = std::process::Command::new("brew")
                .args(["install", "ffmpeg"])
                .status()
                .map_err(|e| miette::miette!("spawn brew: {e:#}"))?;
            if !status.success() {
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Err)
                        .noun("ffmpeg")
                        .verb(format!("install failed ({status})"))
                        .render(theme)
                );
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("stt")
                        .verb("disabled")
                        .render(theme)
                );
                return Ok(false);
            }
            if !right_stt::ffmpeg_available() {
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("ffmpeg")
                        .verb("not in PATH yet — restart shell")
                        .render(theme)
                );
                println!(
                    "{}",
                    right_ui::status(right_ui::Glyph::Warn)
                        .noun("stt")
                        .verb("disabled")
                        .render(theme)
                );
                return Ok(false);
            }
            tracing::info!("ffmpeg installed via brew");
            Ok(true)
        }
        "linux" => {
            println!("ffmpeg required for voice transcription. install:");
            println!("  Debian/Ubuntu:  sudo apt install ffmpeg");
            println!("  NixOS / devenv: add 'pkgs.ffmpeg' to your packages");
            println!("then re-run this command.");
            Ok(false)
        }
        other => {
            println!("ffmpeg required, but auto-install is not supported on '{other}'.");
            println!("Install ffmpeg from https://ffmpeg.org/download.html, then re-run.");
            Ok(false)
        }
    }
}

/// Wizard step: ask enable/disable + model selection, run ffmpeg detection
/// + install prompt as needed. Returns Some((enabled, model)) on completion,
///   None if the user pressed Esc on either prompt (caller decides where to go back).
pub fn stt_setup() -> miette::Result<Option<(bool, right_agent_config::WhisperModel)>> {
    use right_agent_config::WhisperModel;

    // Step 1: enable y/n
    let Some(enable) = right_agent::init::inquire_back(|| {
        inquire::Confirm::new("enable voice transcription?")
            .with_default(true)
            .with_help_message(
                "telegram voice + video notes are transcribed locally via whisper.cpp.",
            )
            .prompt()
    })?
    else {
        return Ok(None);
    };

    if !enable {
        return Ok(Some((false, WhisperModel::Small)));
    }

    // Step 2: model select
    let Some(picked) = right_agent::init::inquire_back(|| {
        inquire::Select::new(
            "whisper model:",
            vec![
                "tiny     — ~75 MB,   fastest, OK for short commands",
                "base     — ~150 MB,  decent",
                "small    — ~470 MB,  recommended (default)",
                "medium   — ~1.5 GB,  very good",
                "large-v3 — ~3.0 GB,  best quality, slow",
            ],
        )
        .with_starting_cursor(2) // small
        .prompt()
    })?
    else {
        // Caller routes the back navigation (skips two steps from the user's perspective).
        return Ok(None);
    };
    let model = if picked.starts_with("tiny") {
        WhisperModel::Tiny
    } else if picked.starts_with("base") {
        WhisperModel::Base
    } else if picked.starts_with("small") {
        WhisperModel::Small
    } else if picked.starts_with("medium") {
        WhisperModel::Medium
    } else if picked.starts_with("large-v3") {
        WhisperModel::LargeV3
    } else {
        unreachable!("unexpected whisper option label: {picked}")
    };

    // Step 3: ffmpeg check + optional install
    let ffmpeg_ok = prompt_ffmpeg_install()?;
    Ok(Some((ffmpeg_ok, model)))
}

// ---------------------------------------------------------------------------
// YAML mutation helpers (private)
// ---------------------------------------------------------------------------

/// Update (or append) a top-level scalar field in an agent.yaml file.
fn update_agent_yaml_field(path: &Path, key: &str, value: &str) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let prefix = format!("{key}:");
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                found = true;
                format!("{key}: {value}")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        lines.push(format!("{key}: {value}"));
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Remove a top-level scalar field from an agent.yaml file.
fn remove_agent_yaml_field(path: &Path, key: &str) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let prefix = format!("{key}:");
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect();

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Write or update `sandbox.name` in agent.yaml.
pub fn update_agent_yaml_sandbox_name(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    let path = agent_dir.join("agent.yaml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Remove existing sandbox block (header + indented lines), preserving
    // all sub-fields except `name:`.
    let mut sandbox_lines: Vec<String> = Vec::new();
    let mut other_lines: Vec<String> = Vec::new();
    let mut in_sandbox_block = false;

    for line in content.lines() {
        if line == "sandbox:" {
            in_sandbox_block = true;
            continue;
        }
        if in_sandbox_block {
            if line.starts_with("  ") {
                // Skip existing name: line — we'll add the new one.
                if !line.trim_start().starts_with("name:") {
                    sandbox_lines.push(line.to_string());
                }
                continue;
            }
            in_sandbox_block = false;
        }
        other_lines.push(line.to_string());
    }

    // Rebuild: other lines first, then sandbox block with name.
    let mut lines = other_lines;
    lines.push("sandbox:".to_string());
    for sl in sandbox_lines {
        lines.push(sl);
    }
    lines.push(format!("  name: \"{sandbox_name}\""));

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(&path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Remove the entire `memory:` block from an agent.yaml file.
///
/// Used when switching from Hindsight to File: omitting the block lets the
/// config parser fall back to its default (File provider).
fn remove_agent_yaml_memory(path: &Path) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let mut lines: Vec<String> = Vec::new();
    let mut in_memory_block = false;
    for line in content.lines() {
        if line == "memory:" {
            in_memory_block = true;
            continue;
        }
        if in_memory_block {
            if line.starts_with("  ") {
                continue;
            }
            in_memory_block = false;
        }
        lines.push(line.to_string());
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Replace the `memory:` block in an agent.yaml file with a freshly
/// serialized version of `cfg`. Emits only non-default recall fields to
/// keep the yaml tidy.
pub(crate) fn update_agent_yaml_memory(
    path: &Path,
    cfg: &right_agent::agent::types::MemoryConfig,
) -> miette::Result<()> {
    use right_agent::agent::types::MemoryProvider;
    use right_agent::init::{DEFAULT_RECALL_BUDGET, DEFAULT_RECALL_MAX_TOKENS};

    // Strip any pre-existing memory block first.
    remove_agent_yaml_memory(path)?;

    let mut block = String::from("\nmemory:\n");
    let provider_str = match cfg.provider {
        MemoryProvider::File => "file",
        MemoryProvider::Hindsight => "hindsight",
    };
    block.push_str(&format!("  provider: {provider_str}\n"));
    if let Some(ref k) = cfg.api_key {
        block.push_str(&format!("  api_key: \"{k}\"\n"));
    }
    if let Some(ref b) = cfg.bank_id {
        block.push_str(&format!("  bank_id: \"{b}\"\n"));
    }
    if cfg.recall_budget != DEFAULT_RECALL_BUDGET {
        block.push_str(&format!("  recall_budget: {}\n", cfg.recall_budget));
    }
    if cfg.recall_max_tokens != DEFAULT_RECALL_MAX_TOKENS {
        block.push_str(&format!("  recall_max_tokens: {}\n", cfg.recall_max_tokens));
    }

    let mut content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&block);

    std::fs::write(path, &content)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Replace the `learning:` block in an agent.yaml file.
///
/// Removes any existing `learning:` block (header + indented body), then
/// appends the new block. Optional model fields are omitted when `None` so the
/// runtime inherits the agent model.
fn update_agent_yaml_learning(
    path: &Path,
    learning: &right_agent::agent::types::LearningConfig,
) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    let mut lines: Vec<String> = Vec::new();
    let mut in_learning_block = false;
    for line in content.lines() {
        if line == "learning:" {
            in_learning_block = true;
            continue;
        }
        if in_learning_block {
            if line.starts_with("  ") {
                continue;
            }
            in_learning_block = false;
        }
        lines.push(line.to_string());
    }

    lines.push("learning:".to_string());
    lines.push(format!(
        "  max_daily_budget_usd: {}",
        learning.max_daily_budget_usd
    ));
    lines.push(format!(
        "  prefilter_enabled: {}",
        learning.prefilter_enabled
    ));
    if let Some(model) = learning.prefilter_model.as_deref() {
        lines.push(format!("  prefilter_model: \"{model}\""));
    }
    lines.push(format!(
        "  probe_writer_enabled: {}",
        learning.probe_writer_enabled
    ));
    if let Some(model) = learning.probe_writer_model.as_deref() {
        lines.push(format!("  probe_writer_model: \"{model}\""));
    }
    lines.push(format!("  curator_enabled: {}", learning.curator_enabled));
    if let Some(model) = learning.curator_model.as_deref() {
        lines.push(format!("  curator_model: \"{model}\""));
    }
    lines.push(format!(
        "  curator_interval_hours: {}",
        learning.curator_interval_hours
    ));
    lines.push(format!(
        "  curator_min_idle_hours: {}",
        learning.curator_min_idle_hours
    ));
    lines.push(format!(
        "  curator_stale_after_days: {}",
        learning.curator_stale_after_days
    ));
    lines.push(format!(
        "  curator_archive_after_days: {}",
        learning.curator_archive_after_days
    ));
    lines.push(format!(
        "  curator_cost_spike_k: {:.3}",
        learning.curator_cost_spike_k
    ));
    lines.push(format!(
        "  curator_cost_spike_baseline_days: {}",
        learning.curator_cost_spike_baseline_days
    ));
    lines.push(format!(
        "  curator_cost_spike_min_floor_usd: {:.3}",
        learning.curator_cost_spike_min_floor_usd
    ));
    lines.push(format!(
        "  curator_skill_change_threshold: {}",
        learning.curator_skill_change_threshold
    ));
    lines.push(format!(
        "  curator_min_cooldown_hours: {}",
        learning.curator_min_cooldown_hours
    ));
    lines.push(format!(
        "  curator_circuit_failure_threshold: {}",
        learning.curator_circuit_failure_threshold
    ));
    lines.push(format!(
        "  curator_circuit_cooldown_hours: {}",
        learning.curator_circuit_cooldown_hours
    ));
    lines.push(format!(
        "  curator_mode: {}",
        match learning.curator_mode {
            right_agent::agent::types::CuratorMode::Apply => "apply",
            right_agent::agent::types::CuratorMode::ReportOnly => "report_only",
        }
    ));
    lines.push(format!(
        "  baseline_window_days: {}",
        learning.baseline_window_days
    ));
    lines.push(format!(
        "  baseline_min_sample: {}",
        learning.baseline_min_sample
    ));

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod learning_yaml_tests {
    use super::*;
    use right_agent::agent::types::LearningConfig;
    use tempfile::tempdir;

    fn write_yaml(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("agent.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn update_agent_yaml_learning_full_block_roundtrips() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let learning = LearningConfig {
            max_daily_budget_usd: 12.5,
            prefilter_enabled: false,
            prefilter_model: Some("claude-haiku-4-5-20251001".to_owned()),
            probe_writer_enabled: true,
            probe_writer_model: Some("claude-sonnet-4-6".to_owned()),
            curator_enabled: true,
            curator_model: None,
            curator_interval_hours: 48,
            curator_min_idle_hours: 4,
            curator_stale_after_days: 14,
            curator_archive_after_days: 60,
            ..LearningConfig::default()
        };
        update_agent_yaml_learning(&path, &learning).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("model: \"sonnet\""),
            "pre-existing field survives"
        );
        assert!(content.contains("prefilter_model: \"claude-haiku-4-5-20251001\""));
        assert!(content.contains("probe_writer_model: \"claude-sonnet-4-6\""));
        assert!(!content.contains("curator_model:"), "absent model omitted");
        let parsed: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content).unwrap();
        assert!(
            (parsed.learning.max_daily_budget_usd - learning.max_daily_budget_usd).abs()
                < f64::EPSILON
        );
        assert!(!parsed.learning.prefilter_enabled);
        assert!(parsed.learning.probe_writer_enabled);
        assert_eq!(parsed.learning.curator_interval_hours, 48);
        assert_eq!(parsed.learning.curator_stale_after_days, 14);
        assert_eq!(parsed.learning.curator_archive_after_days, 60);
    }

    #[test]
    fn update_agent_yaml_learning_replaces_existing_and_omits_absent_models() {
        let dir = tempdir().unwrap();
        let initial = "model: \"sonnet\"\n\nlearning:\n  max_daily_budget_usd: 7.5\n  curator_interval_hours: 999\n\nnetwork_policy: permissive\n";
        let path = write_yaml(dir.path(), initial);
        let learning = LearningConfig {
            max_daily_budget_usd: 2.0,
            ..LearningConfig::default()
        };
        update_agent_yaml_learning(&path, &learning).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("max_daily_budget_usd: 2"));
        assert!(
            !content.contains("999"),
            "old curator_interval_hours replaced"
        );
        assert!(content.contains("network_policy: permissive"));
        assert!(
            !content.contains("prefilter_model:") || content.contains("prefilter_model: \"claude"),
            "prefilter model written correctly"
        );
        let parsed: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content).unwrap();
        assert!((parsed.learning.max_daily_budget_usd - 2.0).abs() < f64::EPSILON);
        assert_eq!(
            parsed.learning.curator_interval_hours,
            LearningConfig::default().curator_interval_hours
        );
    }

    #[test]
    fn update_agent_yaml_learning_persists_circuit_knobs_and_mode() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let learning = LearningConfig {
            curator_circuit_failure_threshold: 7,
            curator_circuit_cooldown_hours: 6,
            curator_mode: right_agent::agent::types::CuratorMode::ReportOnly,
            ..LearningConfig::default()
        };
        update_agent_yaml_learning(&path, &learning).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("curator_circuit_failure_threshold: 7"));
        assert!(content.contains("curator_circuit_cooldown_hours: 6"));
        assert!(content.contains("curator_mode: report_only"));
        // Round-trip: the operator's non-default choices must survive a parse,
        // not silently fall back to serde defaults (apply / 3 / 24).
        let parsed: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content).unwrap();
        assert_eq!(parsed.learning.curator_circuit_failure_threshold, 7);
        assert_eq!(parsed.learning.curator_circuit_cooldown_hours, 6);
        assert_eq!(
            parsed.learning.curator_mode,
            right_agent::agent::types::CuratorMode::ReportOnly
        );
    }

    #[test]
    fn parse_optional_model_override_blank_returns_none() {
        assert_eq!(parse_optional_model_override(""), Ok(None));
        assert_eq!(parse_optional_model_override("   "), Ok(None));
    }

    #[test]
    fn parse_optional_model_override_string_returns_some() {
        assert_eq!(
            parse_optional_model_override("claude-haiku-4-5-20251001"),
            Ok(Some("claude-haiku-4-5-20251001".to_owned()))
        );
    }

    #[test]
    fn parse_bool_yes_no_accepts_yes_no_true_false() {
        assert_eq!(parse_bool_yes_no("yes"), Ok(true));
        assert_eq!(parse_bool_yes_no("no"), Ok(false));
        assert_eq!(parse_bool_yes_no("true"), Ok(true));
        assert_eq!(parse_bool_yes_no("false"), Ok(false));
        assert!(parse_bool_yes_no("maybe").is_err());
    }

    #[test]
    fn parse_u32_positive_rejects_zero() {
        assert!(parse_u32_positive("0", 1, "test").is_err());
    }

    #[test]
    fn parse_u32_positive_empty_returns_fallback() {
        assert_eq!(parse_u32_positive("", 42, "test").unwrap(), 42);
    }

    #[test]
    fn parse_u32_positive_parses_value() {
        assert_eq!(parse_u32_positive("168", 1, "test").unwrap(), 168);
    }

    #[test]
    fn parse_positive_finite_f64_rejects_zero() {
        assert!(parse_positive_finite_f64("0", 1.0, "test").is_err());
    }

    #[test]
    fn parse_positive_finite_f64_rejects_negative() {
        assert!(parse_positive_finite_f64("-1.5", 1.0, "test").is_err());
    }

    #[test]
    fn parse_positive_finite_f64_rejects_inf() {
        assert!(parse_positive_finite_f64("inf", 1.0, "test").is_err());
    }

    #[test]
    fn parse_positive_finite_f64_empty_returns_fallback() {
        let v = parse_positive_finite_f64("", 3.0, "test").unwrap();
        assert!((v - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_positive_finite_f64_parses_value() {
        let v = parse_positive_finite_f64("2.5", 1.0, "test").unwrap();
        assert!((v - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn update_agent_yaml_learning_emits_new_knob_fields() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let learning = LearningConfig {
            curator_cost_spike_k: 5.0,
            curator_cost_spike_baseline_days: 7,
            curator_cost_spike_min_floor_usd: 0.10,
            curator_skill_change_threshold: 5,
            curator_min_cooldown_hours: 24,
            baseline_window_days: 21,
            baseline_min_sample: 30,
            ..LearningConfig::default()
        };
        update_agent_yaml_learning(&path, &learning).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: right_agent::agent::AgentConfig = serde_saphyr::from_str(&content).unwrap();
        assert!((parsed.learning.curator_cost_spike_k - 5.0).abs() < 1e-9);
        assert_eq!(parsed.learning.curator_cost_spike_baseline_days, 7);
        assert!((parsed.learning.curator_cost_spike_min_floor_usd - 0.10).abs() < 1e-9);
        assert_eq!(parsed.learning.curator_skill_change_threshold, 5);
        assert_eq!(parsed.learning.curator_min_cooldown_hours, 24);
        assert_eq!(parsed.learning.baseline_window_days, 21);
        assert_eq!(parsed.learning.baseline_min_sample, 30);
    }
}

#[cfg(test)]
mod memory_yaml_tests {
    use super::*;
    use right_agent::agent::types::{MemoryConfig, MemoryProvider, RecallBudget};
    use tempfile::tempdir;

    fn write_yaml(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("agent.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn parse_agent_config(path: &std::path::Path) -> right_agent::agent::AgentConfig {
        let content = std::fs::read_to_string(path).unwrap();
        serde_saphyr::from_str(&content).unwrap()
    }

    #[test]
    fn update_agent_yaml_memory_full_hindsight_block_roundtrips() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("hs_abc123".into()),
            bank_id: Some("my-bank".into()),
            recall_budget: RecallBudget::High,
            recall_max_tokens: 8192,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let parsed = parse_agent_config(&path);
        let m = parsed.memory.expect("memory block must be present");
        assert_eq!(m.provider, MemoryProvider::Hindsight);
        assert_eq!(m.api_key.as_deref(), Some("hs_abc123"));
        assert_eq!(m.bank_id.as_deref(), Some("my-bank"));
        assert_eq!(m.recall_budget, RecallBudget::High);
        assert_eq!(m.recall_max_tokens, 8192);
    }

    #[test]
    fn update_agent_yaml_memory_omits_default_recall_fields() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("hs_x".into()),
            bank_id: None,
            recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("recall_budget"),
            "default recall_budget must not be emitted, got:\n{content}"
        );
        assert!(
            !content.contains("recall_max_tokens"),
            "default recall_max_tokens must not be emitted, got:\n{content}"
        );
        assert!(
            !content.contains("bank_id"),
            "absent bank_id must not be emitted, got:\n{content}"
        );
    }

    #[test]
    fn update_agent_yaml_memory_replaces_existing_block() {
        let dir = tempdir().unwrap();
        let initial = "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"old\"\n  bank_id: \"old-bank\"\n";
        let path = write_yaml(dir.path(), initial);
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("new".into()),
            bank_id: Some("new-bank".into()),
            recall_budget: right_agent::init::DEFAULT_RECALL_BUDGET,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("\"old\""), "old api_key must be gone");
        assert!(!content.contains("old-bank"), "old bank_id must be gone");
        assert!(content.contains("\"new\""), "new api_key must be present");
        assert!(content.contains("new-bank"), "new bank_id must be present");

        let parsed = parse_agent_config(&path);
        let m = parsed.memory.unwrap();
        assert_eq!(m.api_key.as_deref(), Some("new"));
        assert_eq!(m.bank_id.as_deref(), Some("new-bank"));
    }

    #[test]
    fn remove_agent_yaml_memory_strips_entire_block() {
        let dir = tempdir().unwrap();
        let initial = "model: \"sonnet\"\n\nmemory:\n  provider: hindsight\n  api_key: \"x\"\n  bank_id: \"b\"\n\nnetwork_policy: permissive\n";
        let path = write_yaml(dir.path(), initial);
        remove_agent_yaml_memory(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("memory:"),
            "memory block must be gone, got:\n{content}"
        );
        assert!(
            !content.contains("api_key"),
            "api_key must be gone, got:\n{content}"
        );
        assert!(
            content.contains("network_policy: permissive"),
            "unrelated fields must survive, got:\n{content}"
        );

        let parsed = parse_agent_config(&path);
        assert!(parsed.memory.is_none(), "memory must be None after removal");
    }

    #[test]
    fn update_agent_yaml_memory_appends_when_absent() {
        let dir = tempdir().unwrap();
        let path = write_yaml(dir.path(), "model: \"sonnet\"\n");
        let cfg = MemoryConfig {
            provider: MemoryProvider::Hindsight,
            api_key: Some("k".into()),
            bank_id: None,
            recall_budget: RecallBudget::Low,
            recall_max_tokens: right_agent::init::DEFAULT_RECALL_MAX_TOKENS,
        };
        update_agent_yaml_memory(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("model: \"sonnet\""),
            "pre-existing field survives"
        );
        assert!(content.contains("memory:"), "memory block added");
        assert!(
            content.contains("recall_budget: low"),
            "non-default budget emitted"
        );
    }
}

#[cfg(test)]
mod up_repair_tests {
    use super::{
        FilesystemTunnelConfigCutover, TunnelConfigCutover, TunnelListEntry, TunnelRepairExecutor,
        execute_tunnel_replacement, execute_tunnel_replacement_with_cutover,
        set_agent_telegram_token,
    };
    use std::path::{Path, PathBuf};

    struct RecordingTunnelExecutor {
        configured: TunnelListEntry,
        configured_remote_exists: bool,
        replacement: TunnelListEntry,
        configured_credentials: PathBuf,
        replacement_credentials: PathBuf,
        failed_routes: Vec<String>,
        routed: Vec<String>,
        deleted: Vec<String>,
    }

    impl TunnelRepairExecutor for RecordingTunnelExecutor {
        fn list(&mut self) -> miette::Result<Vec<TunnelListEntry>> {
            Ok(if self.configured_remote_exists {
                vec![self.configured.clone()]
            } else {
                Vec::new()
            })
        }

        fn create(&mut self, name: &str) -> miette::Result<TunnelListEntry> {
            self.replacement.name = name.to_owned();
            Ok(self.replacement.clone())
        }

        fn route_dns(&mut self, uuid: &str, _hostname: &str) -> miette::Result<()> {
            self.routed.push(uuid.to_owned());
            if self.failed_routes.iter().any(|failed| failed == uuid) {
                Err(miette::miette!("route failed for {uuid}"))
            } else {
                Ok(())
            }
        }

        fn delete(&mut self, uuid: &str) -> miette::Result<()> {
            self.deleted.push(uuid.to_owned());
            Ok(())
        }

        fn credentials_path(&self, uuid: &str) -> miette::Result<PathBuf> {
            if uuid == self.configured.id {
                Ok(self.configured_credentials.clone())
            } else {
                Ok(self.replacement_credentials.clone())
            }
        }
    }

    fn tunnel_config(credentials: &Path) -> right_config::GlobalConfig {
        right_config::GlobalConfig {
            tunnel: right_config::TunnelConfig {
                hostname: "right.example.com".to_owned(),
                provider: right_config::TunnelProvider::Cloudflared {
                    tunnel_uuid: "old-uuid".to_owned(),
                    credentials_file: credentials.to_owned(),
                },
            },
            aggregator: right_config::AggregatorConfig {
                allowed_hosts: vec!["aggregator.internal".to_owned()],
            },
        }
    }

    fn tunnel_executor(dir: &Path, failed_routes: &[&str]) -> RecordingTunnelExecutor {
        let old = dir.join("old.json");
        let new = dir.join("new.json");
        std::fs::write(&old, r#"{"TunnelID":"old-uuid","AccountTag":"account"}"#).unwrap();
        std::fs::write(&new, r#"{"TunnelID":"new-uuid","AccountTag":"account"}"#).unwrap();
        RecordingTunnelExecutor {
            configured: TunnelListEntry {
                id: "old-uuid".to_owned(),
                name: "configured-name".to_owned(),
            },
            configured_remote_exists: true,
            replacement: TunnelListEntry {
                id: "new-uuid".to_owned(),
                name: String::new(),
            },
            configured_credentials: old,
            replacement_credentials: new,
            failed_routes: failed_routes
                .iter()
                .map(|uuid| (*uuid).to_owned())
                .collect(),
            routed: Vec::new(),
            deleted: Vec::new(),
        }
    }

    struct FailingCommitCutover {
        filesystem: FilesystemTunnelConfigCutover,
    }

    impl FailingCommitCutover {
        fn new(home: &Path) -> Self {
            Self {
                filesystem: FilesystemTunnelConfigCutover::new(home),
            }
        }
    }

    impl TunnelConfigCutover for FailingCommitCutover {
        fn stage(
            &mut self,
            home: &Path,
            config: &right_config::GlobalConfig,
        ) -> miette::Result<()> {
            self.filesystem.stage(home, config)
        }

        fn commit(&mut self, _home: &Path) -> miette::Result<()> {
            Err(miette::miette!("injected config commit failure"))
        }

        fn cleanup(&mut self) -> miette::Result<()> {
            self.filesystem.cleanup()
        }

        fn pending_path(&self) -> Option<&Path> {
            self.filesystem.pending_path()
        }
    }

    #[test]
    fn telegram_token_setter_preserves_yaml_and_quotes_scalar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(
            &path,
            "# keep\nmodel: sonnet\ntelegram_token: \"1:old\"\nsandbox:\n  name: right-a\n",
        )
        .unwrap();

        set_agent_telegram_token(&path, "123:new_token").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# keep"));
        assert!(content.contains("sandbox:\n  name: right-a"));
        assert!(content.contains("telegram_token: \"123:new_token\""));
        let parsed: right_agent_config::AgentConfig = serde_saphyr::from_str(&content).unwrap();
        assert_eq!(parsed.telegram_token.as_deref(), Some("123:new_token"));
    }

    #[test]
    fn telegram_token_setter_rejects_invalid_token_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        let original = "model: sonnet\n";
        std::fs::write(&path, original).unwrap();

        assert!(set_agent_telegram_token(&path, "not-a-token").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn tunnel_repair_route_failure_preserves_old_config_and_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let original = std::fs::read(dir.path().join("config.yaml")).unwrap();
        let mut executor = tunnel_executor(dir.path(), &["new-uuid"]);

        let error = execute_tunnel_replacement(dir.path(), &config, &mut executor).unwrap_err();

        assert!(format!("{error:#}").contains("route failed"));
        assert_eq!(
            std::fs::read(dir.path().join("config.yaml")).unwrap(),
            original
        );
        assert_eq!(executor.deleted, ["new-uuid"]);
        assert_eq!(executor.routed, ["new-uuid"]);
    }

    #[test]
    fn tunnel_repair_cuts_over_before_deleting_configured_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let mut executor = tunnel_executor(dir.path(), &[]);

        execute_tunnel_replacement(dir.path(), &config, &mut executor).unwrap();

        let repaired = right_config::read_global_config(dir.path()).unwrap();
        assert_eq!(repaired.tunnel.hostname, "right.example.com");
        assert_eq!(repaired.aggregator.allowed_hosts, ["aggregator.internal"]);
        assert!(matches!(
            repaired.tunnel.provider,
            right_config::TunnelProvider::Cloudflared { ref tunnel_uuid, .. }
                if tunnel_uuid == "new-uuid"
        ));
        assert_eq!(executor.deleted, ["old-uuid"]);
        assert_eq!(executor.routed, ["new-uuid"]);
    }

    #[test]
    fn tunnel_repair_replaces_configured_tunnel_missing_from_remote_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let mut executor = tunnel_executor(dir.path(), &[]);
        executor.configured_remote_exists = false;

        execute_tunnel_replacement(dir.path(), &config, &mut executor).unwrap();

        let repaired = right_config::read_global_config(dir.path()).unwrap();
        assert!(matches!(
            repaired.tunnel.provider,
            right_config::TunnelProvider::Cloudflared { ref tunnel_uuid, .. }
                if tunnel_uuid == "new-uuid"
        ));
        assert_eq!(
            executor.replacement.name,
            format!("right-repair-{}", std::process::id())
        );
        assert_eq!(executor.routed, ["new-uuid"]);
        assert!(executor.deleted.is_empty());
    }

    #[test]
    fn tunnel_repair_commit_failure_rolls_dns_back_before_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let original = std::fs::read(dir.path().join("config.yaml")).unwrap();
        let mut executor = tunnel_executor(dir.path(), &[]);
        let mut cutover = FailingCommitCutover::new(dir.path());

        let error = execute_tunnel_replacement_with_cutover(
            dir.path(),
            &config,
            &mut executor,
            &mut cutover,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("injected config commit failure"));
        assert_eq!(executor.routed, ["new-uuid", "old-uuid"]);
        assert_eq!(executor.deleted, ["new-uuid"]);
        assert_eq!(
            std::fs::read(dir.path().join("config.yaml")).unwrap(),
            original
        );
        assert!(cutover.pending_path().is_none());
    }

    #[test]
    fn tunnel_repair_commit_failure_without_old_remote_retains_active_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let original = std::fs::read(dir.path().join("config.yaml")).unwrap();
        let mut executor = tunnel_executor(dir.path(), &[]);
        executor.configured_remote_exists = false;
        let mut cutover = FailingCommitCutover::new(dir.path());

        let error = execute_tunnel_replacement_with_cutover(
            dir.path(),
            &config,
            &mut executor,
            &mut cutover,
        )
        .unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("injected config commit failure"));
        assert!(message.contains("configured tunnel old-uuid no longer exists"));
        assert!(message.contains("replacement tunnel new-uuid retained"));
        assert_eq!(executor.routed, ["new-uuid"]);
        assert!(executor.deleted.is_empty());
        assert_eq!(
            std::fs::read(dir.path().join("config.yaml")).unwrap(),
            original
        );
        let pending = cutover.pending_path().expect("pending recovery config");
        assert!(pending.exists());
        assert!(
            std::fs::read_to_string(pending)
                .unwrap()
                .contains("new-uuid")
        );
    }

    #[test]
    fn tunnel_repair_failed_dns_rollback_retains_replacement_and_pending_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = tunnel_config(&dir.path().join("old.json"));
        right_config::write_global_config(dir.path(), &config).unwrap();
        let original = std::fs::read(dir.path().join("config.yaml")).unwrap();
        let mut executor = tunnel_executor(dir.path(), &["old-uuid"]);
        let mut cutover = FailingCommitCutover::new(dir.path());

        let error = execute_tunnel_replacement_with_cutover(
            dir.path(),
            &config,
            &mut executor,
            &mut cutover,
        )
        .unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("injected config commit failure"));
        assert!(message.contains("route failed for old-uuid"));
        assert!(message.contains("replacement tunnel new-uuid retained"));
        assert_eq!(executor.routed, ["new-uuid", "old-uuid"]);
        assert!(executor.deleted.is_empty());
        assert_eq!(
            std::fs::read(dir.path().join("config.yaml")).unwrap(),
            original
        );
        let pending = cutover.pending_path().expect("pending recovery config");
        assert!(pending.exists());
        assert!(
            std::fs::read_to_string(pending)
                .unwrap()
                .contains("new-uuid")
        );
    }
}

/// Replace the `stt:` block in an agent.yaml file.
///
/// Removes any existing `stt:` block (header + indented body), then appends
/// the new block.  Matches the style of the other `update_agent_yaml_*`
/// helpers: line-by-line block stripping + unconditional append.
fn update_agent_yaml_stt(
    path: &Path,
    stt: &right_agent::agent::types::SttConfig,
) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Strip existing stt: block (header + indented lines).
    let mut lines: Vec<String> = Vec::new();
    let mut in_stt_block = false;
    for line in content.lines() {
        if line == "stt:" {
            in_stt_block = true;
            continue;
        }
        if in_stt_block {
            if line.starts_with("  ") {
                continue;
            }
            in_stt_block = false;
        }
        lines.push(line.to_string());
    }

    // Append new stt block.
    lines.push("stt:".to_string());
    lines.push(format!("  enabled: {}", stt.enabled));
    lines.push(format!("  model: {}", stt.model.yaml_str()));

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

/// Replace the `allowed_chat_ids` block in an agent.yaml file.
///
/// Removes any existing `allowed_chat_ids:` block (header + indented list items),
/// then appends the new block if `ids` is non-empty.
fn update_agent_yaml_chat_ids(path: &Path, ids: &[i64]) -> miette::Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Filter out the existing allowed_chat_ids block.
    let mut lines: Vec<String> = Vec::new();
    let mut in_chat_ids_block = false;
    for line in content.lines() {
        if line.starts_with("allowed_chat_ids:") {
            in_chat_ids_block = true;
            continue;
        }
        if in_chat_ids_block {
            // List items under allowed_chat_ids are indented.
            if line.starts_with("  - ") || line.starts_with("  -\t") {
                continue;
            }
            // Non-indented line means block ended.
            in_chat_ids_block = false;
        }
        lines.push(line.to_string());
    }

    // Append new block if needed.
    if !ids.is_empty() {
        lines.push("allowed_chat_ids:".to_string());
        for id in ids {
            lines.push(format!("  - {id}"));
        }
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, &output)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod stt_yaml_tests {
    use super::*;
    use right_agent::agent::types::SttConfig;
    use right_agent_config::WhisperModel;

    #[test]
    fn append_stt_when_block_missing() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "telegram_token: \"x\"\n").unwrap();

        let stt = SttConfig {
            enabled: true,
            model: WhisperModel::Small,
        };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("stt:"));
        assert!(content.contains("enabled: true"));
        assert!(content.contains("model: small"));
    }

    #[test]
    fn replace_stt_when_block_present() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "telegram_token: \"x\"\nstt:\n  enabled: true\n  model: tiny\n",
        )
        .unwrap();

        let stt = SttConfig {
            enabled: false,
            model: WhisperModel::Small,
        };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        // Block replaced, not duplicated:
        assert_eq!(content.matches("stt:").count(), 1, "exactly one stt: block");
        assert!(content.contains("enabled: false"));
        assert!(content.contains("model: small"));
        assert!(!content.contains("model: tiny"));
    }
}

#[cfg(test)]
mod voice_pass {
    use super::PROMPT_LABELS;

    const ALLOWED_PROPER_NOUNS: &[&str] = &[
        "HINDSIGHT_API_KEY",
        "RIGHT_TG_TOKEN",
        "MEMORY.md",
        "@BotFather",
        "@userinfobot",
    ];

    #[test]
    fn labels_are_lowercase_first() {
        for label in PROMPT_LABELS {
            let first = label.chars().next().unwrap();
            assert!(
                !first.is_uppercase() || ALLOWED_PROPER_NOUNS.iter().any(|p| label.starts_with(p)),
                "prompt has uppercase first letter: {label:?}"
            );
        }
    }

    #[test]
    fn labels_have_no_exclamation_marks() {
        for label in PROMPT_LABELS {
            assert!(!label.contains('!'), "prompt contains '!': {label:?}");
        }
    }
}
