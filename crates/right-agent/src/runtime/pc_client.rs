use serde::Deserialize;

use right_runtime_state::read_state;

/// Status information for a single process managed by process-compose.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProcessInfo {
    pub name: String,
    pub status: String,
    pub pid: i64,
    pub system_time: String,
    pub exit_code: i32,
}

/// Wrapper for the process-compose `/processes` endpoint response.
#[derive(Debug, Deserialize)]
pub(crate) struct ProcessesResponse {
    pub data: Vec<ProcessInfo>,
}

/// Response from the process-compose `/process/logs/{name}/{endOffset}/{limit}` endpoint.
#[derive(Debug, Deserialize)]
pub struct LogsResponse {
    pub logs: Vec<String>,
}

/// HTTP header process-compose uses to carry the API token.
///
/// Constant lifted from upstream
/// (`src/config/config.go::TokenHeader = "X-PC-Token-Key"`). Stable from at
/// least v1.94 through v1.103. Process-compose does NOT honor
/// `Authorization: Bearer …`; using that yields silent 401s on every
/// REST call.
const PC_TOKEN_HEADER: &str = "X-PC-Token-Key";

/// Async client for the process-compose REST API.
///
/// Optionally carries the `PC_API_TOKEN` value, sent in the `X-PC-Token-Key`
/// request header. When the token is set, process-compose rejects
/// unauthenticated requests — this prevents any stray HTTP caller (tests,
/// debugging tools) from accidentally stopping production bots.
pub struct PcClient {
    client: reqwest::Client,
    pub(crate) base_url: String,
    /// Optional API token (matches PC's `PC_API_TOKEN` env var).
    api_token: Option<String>,
}

impl PcClient {
    /// Create a new client connected to process-compose via TCP.
    ///
    /// Crate-private: external callers must construct through [`PcClient::from_home`]
    /// so that `right --home <path>` isolation is enforced. See the
    /// "Runtime isolation — mandatory" section in ARCHITECTURE.md.
    pub(crate) fn new(port: u16, api_token: Option<String>) -> miette::Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| miette::miette!("failed to create process-compose client: {e:#}"))?;
        Ok(Self {
            client,
            base_url: format!("http://localhost:{port}"),
            api_token,
        })
    }

    /// Construct a client from the right home directory.
    ///
    /// Reads the running PC port and API token from `<home>/run/state.json`.
    /// Returns `Ok(None)` when no PC was started from this home (state file absent) —
    /// this is the normal case for tempdir-isolated tests and for commands run before
    /// `right up`. Returns `Err` on malformed state or other I/O errors.
    ///
    /// This is the only public constructor — it guarantees that commands run
    /// against an isolated `--home <tempdir>` never accidentally hit the
    /// user's live process-compose on the default port.
    pub fn from_home(home: &std::path::Path) -> miette::Result<Option<Self>> {
        let state_path = home.join("run").join("state.json");
        if !state_path.exists() {
            return Ok(None);
        }
        let state = read_state(&state_path)?;
        let client = Self::new(state.pc_port, state.pc_api_token)?;
        Ok(Some(client))
    }

    /// Apply authentication to a request builder if a token is configured.
    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_token {
            Some(token) => builder.header(PC_TOKEN_HEADER, token),
            None => builder,
        }
    }

    /// Check if process-compose is alive.
    pub async fn health_check(&self) -> miette::Result<()> {
        let resp = self
            .auth(self.client.get(format!("{}/live", self.base_url)))
            .send()
            .await
            .map_err(|e| miette::miette!("process-compose health check failed: {e:#}"))?;

        if !resp.status().is_success() {
            return Err(miette::miette!(
                "process-compose health check returned {}",
                resp.status()
            ));
        }
        Ok(())
    }

    /// List all processes and their current status.
    pub async fn list_processes(&self) -> miette::Result<Vec<ProcessInfo>> {
        self.fetch_processes()
            .await
            .map_err(|e| miette::miette!("failed to list processes: {e:#}"))
    }

    /// `list_processes` with the raw reqwest error preserved so callers can
    /// classify transport failures (`is_connect`) instead of stringly
    /// matching rendered miette text.
    async fn fetch_processes(&self) -> Result<Vec<ProcessInfo>, reqwest::Error> {
        let resp = self
            .auth(self.client.get(format!("{}/processes", self.base_url)))
            .send()
            .await?;
        let data: ProcessesResponse = resp.json().await?;
        Ok(data.data)
    }

    /// Restart a specific process by name.
    pub async fn restart_process(&self, name: &str) -> miette::Result<()> {
        let resp = self
            .auth(
                self.client
                    .post(format!("{}/process/restart/{name}", self.base_url)),
            )
            .send()
            .await
            .map_err(|e| miette::miette!("failed to restart process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!(
                "restart process '{name}' failed ({status}): {body}"
            ));
        }
        Ok(())
    }

    /// Restart the managed cloudflared process when regenerated ingress config changed.
    ///
    /// Cloudflared reads local ingress config at process start; process-compose
    /// reload does not restart it when only the config file content changes.
    pub async fn restart_cloudflared_if_config_changed(
        &self,
        cloudflared_config_changed: bool,
    ) -> miette::Result<()> {
        if !cloudflared_config_changed {
            return Ok(());
        }

        self.restart_process("cloudflared").await
    }

    /// Best-effort cloudflared restart: log a warning on failure and continue.
    ///
    /// Used by register/destroy/restart paths where a stale tunnel is a
    /// degraded-but-survivable outcome — propagating the error would roll
    /// back unrelated successful work (agent registration, removal, restart).
    pub async fn restart_cloudflared_or_warn(&self, cloudflared_config_changed: bool) {
        if let Err(e) = self
            .restart_cloudflared_if_config_changed(cloudflared_config_changed)
            .await
        {
            tracing::warn!(
                error = format!("{e:#}"),
                "failed to restart cloudflared after config change"
            );
        }
    }

    /// Stop a specific process by name.
    pub async fn stop_process(&self, name: &str) -> miette::Result<()> {
        let resp = self
            .auth(
                self.client
                    .patch(format!("{}/process/stop/{name}", self.base_url)),
            )
            .send()
            .await
            .map_err(|e| miette::miette!("failed to stop process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!(
                "stop process '{name}' failed ({status}): {body}"
            ));
        }
        Ok(())
    }

    /// Start a disabled or stopped process by name.
    pub async fn start_process(&self, name: &str) -> miette::Result<()> {
        let resp = self
            .auth(
                self.client
                    .post(format!("{}/process/start/{name}", self.base_url)),
            )
            .send()
            .await
            .map_err(|e| miette::miette!("failed to start process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!(
                "start process '{name}' failed ({status}): {body}"
            ));
        }
        Ok(())
    }

    /// Read recent log lines for a process.
    ///
    /// Uses the PC endpoint `GET /process/logs/{name}/{endOffset}/{limit}`.
    /// `endOffset=0` reads from the end, `limit` controls how many lines.
    pub async fn get_process_logs(&self, name: &str, limit: usize) -> miette::Result<Vec<String>> {
        let resp = self
            .auth(
                self.client
                    .get(format!("{}/process/logs/{name}/0/{limit}", self.base_url)),
            )
            .send()
            .await
            .map_err(|e| miette::miette!("failed to get logs for '{name}': {e:#}"))?;

        let data: LogsResponse = resp
            .json()
            .await
            .map_err(|e| miette::miette!("failed to parse logs for '{name}': {e:#}"))?;
        Ok(data.logs)
    }

    /// Tell process-compose to re-read its configuration files from disk.
    ///
    /// Uses `POST /project/configuration` — process-compose diffs the new config
    /// against running state and adds/updates/removes processes accordingly.
    pub async fn reload_configuration(&self) -> miette::Result<()> {
        let resp = self
            .auth(
                self.client
                    .post(format!("{}/project/configuration", self.base_url)),
            )
            .send()
            .await
            .map_err(|e| {
                miette::miette!("failed to reload process-compose configuration: {e:#}")
            })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!(
                "process-compose configuration reload failed ({status}): {body}"
            ));
        }
        Ok(())
    }

    /// Stop all processes (shutdown process-compose).
    pub async fn shutdown(&self) -> miette::Result<()> {
        self.auth(self.client.post(format!("{}/project/stop", self.base_url)))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to shutdown process-compose: {e:#}"))?;
        Ok(())
    }

    /// Stop the whole project and wait until the runtime is provably down.
    ///
    /// Used by offline operator commands (`right agent db-repair`) that mutate
    /// `data.db*` files: quiescence must be PROVEN, never assumed.
    ///
    /// Sequence: snapshot process names → `POST /project/stop` → poll
    /// health/process endpoints every [`SHUTDOWN_POLL_DELAY`] until the server
    /// is unreachable (process-compose exits once all processes stop — the
    /// generated config never sets `--keep-project`) after every previously
    /// active `right-mcp-server`/`*-bot` was observed terminal.
    ///
    /// Fail-closed rules:
    /// - endpoint unreachable BEFORE a successful shutdown request → error;
    /// - shutdown request rejected → error;
    /// - `timeout` elapses first → error naming the still-active processes.
    ///
    /// Returns the sorted names of processes that were active at snapshot.
    pub async fn shutdown_and_wait(
        &self,
        timeout: std::time::Duration,
    ) -> miette::Result<Vec<String>> {
        self.health_check().await.map_err(|e| {
            miette::miette!(
                "runtime state exists but process-compose is unreachable before shutdown; \
                 refusing to proceed (fail closed): {e:#}"
            )
        })?;
        let snapshot = self.list_processes().await.map_err(|e| {
            miette::miette!("failed to snapshot process list before shutdown: {e:#}")
        })?;
        let mut previously_active: Vec<String> = snapshot
            .iter()
            .filter(|p| !is_terminal_status(&p.status))
            .map(|p| p.name.clone())
            .collect();
        previously_active.sort();

        // The shared `shutdown()` ignores the response status; quiescence
        // requires the request to be accepted, so issue it here with a
        // checked status.
        let resp = self
            .auth(self.client.post(format!("{}/project/stop", self.base_url)))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to shutdown process-compose: {e:#}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.map_err(|error| {
                miette::miette!(
                    "process-compose project stop failed ({status}) and its error body \
                     could not be read: {error:#}"
                )
            })?;
            return Err(miette::miette!(
                "process-compose project stop failed ({status}): {body}"
            ));
        }

        let deadline = std::time::Instant::now() + timeout;
        let mut still_active: Vec<String> = snapshot
            .iter()
            .filter(|p| is_db_holding_process(&p.name) && !is_terminal_status(&p.status))
            .map(|p| format!("{} ({})", p.name, p.status))
            .collect();
        let mut last_api_error: Option<String> = None;
        loop {
            match self.fetch_processes().await {
                Ok(processes) => {
                    still_active = processes
                        .iter()
                        .filter(|p| {
                            is_db_holding_process(&p.name) && !is_terminal_status(&p.status)
                        })
                        .map(|p| format!("{} ({})", p.name, p.status))
                        .collect();
                }
                Err(e) if e.is_connect() => {
                    // Server gone: process-compose only exits after stopping
                    // every process, so the runtime is quiesced.
                    return Ok(previously_active);
                }
                Err(error) => {
                    // A non-connect transport/body error is not proof of
                    // quiescence. Retain it for the timeout error chain while
                    // allowing process-compose to finish shutting down.
                    last_api_error = Some(format!("{error:#}"));
                }
            }
            if std::time::Instant::now() >= deadline {
                let api_detail = last_api_error
                    .as_deref()
                    .map(|error| format!("; last process endpoint error: {error}"))
                    .unwrap_or_default();
                return if still_active.is_empty() {
                    Err(miette::miette!(
                        "timed out ({timeout:?}) waiting for process-compose to exit after \
                         project stop; processes are terminal but the server is still up{api_detail}"
                    ))
                } else {
                    Err(miette::miette!(
                        "timed out ({timeout:?}) waiting for runtime shutdown; still active: {}{}",
                        still_active.join(", "),
                        api_detail
                    ))
                };
            }
            tokio::time::sleep(SHUTDOWN_POLL_DELAY).await;
        }
    }
}

/// Poll cadence for [`PcClient::shutdown_and_wait`].
const SHUTDOWN_POLL_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Processes that hold `data.db*` handles: the MCP aggregator and per-agent
/// bots. Cloudflared never touches the databases, so it is not gated on.
fn is_db_holding_process(name: &str) -> bool {
    name == "right-mcp-server" || name.ends_with("-bot")
}

/// Terminal process-compose statuses (v1.110 `src/types/process.go`):
/// the process will not run again without an explicit start. Anything else
/// (Running, Launching, Launched, Restarting, Terminating, Pending,
/// Foreground, Scheduled) is treated as active — fail-closed.
fn is_terminal_status(status: &str) -> bool {
    matches!(status, "Completed" | "Skipped" | "Error" | "Disabled")
}

#[cfg(test)]
#[path = "pc_client_tests.rs"]
mod tests;
