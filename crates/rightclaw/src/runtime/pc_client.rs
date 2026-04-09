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

/// Async client for the process-compose REST API over a Unix domain socket.
pub struct PcClient {
    client: reqwest::Client,
    base_url: String,
}

impl PcClient {
    /// Create a new client connected to process-compose via TCP.
    pub fn new(port: u16) -> miette::Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| miette::miette!("failed to create process-compose client: {e:#}"))?;
        Ok(Self {
            client,
            base_url: format!("http://localhost:{port}"),
        })
    }

    /// Check if process-compose is alive.
    pub async fn health_check(&self) -> miette::Result<()> {
        let resp = self
            .client
            .get(format!("{}/live", self.base_url))
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
            .client
            .get(format!("{}/processes", self.base_url))
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
            .client
            .post(format!("{}/process/restart/{name}", self.base_url))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to restart process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!("restart process '{name}' failed ({status}): {body}"));
        }
        Ok(())
    }

    /// Stop a specific process by name.
    pub async fn stop_process(&self, name: &str) -> miette::Result<()> {
        let resp = self
            .client
            .patch(format!("{}/process/stop/{name}", self.base_url))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to stop process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!("stop process '{name}' failed ({status}): {body}"));
        }
        Ok(())
    }

    /// Start a disabled or stopped process by name.
    pub async fn start_process(&self, name: &str) -> miette::Result<()> {
        let resp = self
            .client
            .post(format!("{}/process/start/{name}", self.base_url))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to start process '{name}': {e:#}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(miette::miette!("start process '{name}' failed ({status}): {body}"));
        }
        Ok(())
    }

    /// Read recent log lines for a process.
    ///
    /// Uses the PC endpoint `GET /process/logs/{name}/{endOffset}/{limit}`.
    /// `endOffset=0` reads from the end, `limit` controls how many lines.
    pub async fn get_process_logs(&self, name: &str, limit: usize) -> miette::Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/process/logs/{name}/0/{limit}", self.base_url))
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
            .client
            .post(format!("{}/project/configuration", self.base_url))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to reload process-compose configuration: {e:#}"))?;
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
        self.client
            .post(format!("{}/project/stop", self.base_url))
            .send()
            .await
            .map_err(|e| miette::miette!("failed to shutdown process-compose: {e:#}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "pc_client_tests.rs"]
mod tests;
