use serde::Deserialize;

/// Default TCP port for the process-compose API.
pub const PC_PORT: u16 = 18927;

/// Default TCP port for the right-mcp-server HTTP transport.
pub const MCP_HTTP_PORT: u16 = 8100;

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

/// Async client for the process-compose REST API.
///
/// Optionally carries a `PC_API_TOKEN` bearer token. When the token is set,
/// process-compose rejects unauthenticated requests — this prevents any
/// stray HTTP caller (tests, debugging tools) from accidentally stopping
/// production bots.
pub struct PcClient {
    client: reqwest::Client,
    pub(crate) base_url: String,
    /// Optional Bearer token for PC_API_TOKEN authentication.
    api_token: Option<String>,
}

impl PcClient {
    /// Create a new client connected to process-compose via TCP.
    ///
    /// Crate-private: external callers must construct through [`PcClient::from_home`]
    /// so that `rightclaw --home <path>` isolation is enforced. See the
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

    /// Construct a client from the rightclaw home directory.
    ///
    /// Reads the running PC port and API token from `<home>/run/state.json`.
    /// Returns `Ok(None)` when no PC was started from this home (state file absent) —
    /// this is the normal case for tempdir-isolated tests and for commands run before
    /// `rightclaw up`. Returns `Err` on malformed state or other I/O errors.
    ///
    /// This is the only public constructor — it guarantees that commands run
    /// against an isolated `--home <tempdir>` never accidentally hit the
    /// user's live process-compose on the default port.
    pub fn from_home(home: &std::path::Path) -> miette::Result<Option<Self>> {
        let state_path = home.join("run").join("state.json");
        if !state_path.exists() {
            return Ok(None);
        }
        let state = crate::runtime::state::read_state(&state_path)?;
        let client = Self::new(state.pc_port, state.pc_api_token)?;
        Ok(Some(client))
    }

    /// Apply authentication to a request builder if a token is configured.
    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_token {
            Some(token) => builder.bearer_auth(token),
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
        let resp = self
            .auth(self.client.get(format!("{}/processes", self.base_url)))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to list processes: {e:#}"))?;

        let data: ProcessesResponse = resp
            .json()
            .await
            .map_err(|e| miette::miette!("failed to parse process list: {e:#}"))?;
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
}

#[cfg(test)]
#[path = "pc_client_tests.rs"]
mod tests;
