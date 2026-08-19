use std::time::Duration;

use crate::sandbox::{Sandbox, exec_argv};
use right_agent::doctor::{CheckStatus, DoctorCheck};
use right_dashboard::api_types::{
    DoctorCheckResponse, DoctorResponse, SandboxDiskStats, SandboxMemoryStats, SandboxProcess,
    SandboxStatsResponse,
};

const SANDBOX_PROCESS_LIMIT: usize = 50;
const SANDBOX_COMMAND_LIMIT_CHARS: usize = 160;
const SANDBOX_STATS_SCRIPT: &str = r#"limit="$1"
printf '__DISK__\n'
(df -Pk /sandbox 2>/dev/null || df -Pk / 2>/dev/null) | awk 'NR==2 {print $2 " " $3 " " $4 " " $5 " " $6}'
printf '__MEM__\n'
if [ -r /proc/meminfo ]; then awk '/MemTotal:/ {total=$2} /MemAvailable:/ {available=$2} END {if (total != "") print total " " available}' /proc/meminfo; fi
printf '__LOAD__\n'
if [ -r /proc/loadavg ]; then awk '{print $1 " " $2 " " $3}' /proc/loadavg; fi
printf '__PS__\n'
ps -eo pid=,ppid=,pcpu=,pmem=,rss=,args= 2>/dev/null | head -n "$limit""#;

pub(super) fn doctor_response_from_checks(agent: &str, checks: Vec<DoctorCheck>) -> DoctorResponse {
    let mut pass = Vec::new();
    let mut warn = Vec::new();
    let mut fail = Vec::new();

    for check in checks {
        let status = doctor_status(&check.status);
        let response = DoctorCheckResponse {
            name: check.name,
            status: status.to_owned(),
            detail: check.detail,
            fix: check.fix,
        };
        match status {
            "pass" => pass.push(response),
            "warn" => warn.push(response),
            _ => fail.push(response),
        }
    }

    DoctorResponse {
        agent: agent.to_owned(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        pass_count: pass.len() as i64,
        warn_count: warn.len() as i64,
        fail_count: fail.len() as i64,
        pass,
        warn,
        fail,
    }
}

pub(super) async fn sandbox_stats_response(
    agent: &str,
    sandbox: Option<&Sandbox>,
) -> SandboxStatsResponse {
    let Some(sandbox) = sandbox else {
        return unavailable_sandbox_stats(agent, "the agent's sandbox is unavailable");
    };

    match read_sandbox_stats(agent, sandbox).await {
        Ok(response) => response,
        Err(error) => unavailable_sandbox_stats(agent, &format!("{error:#}")),
    }
}

fn parse_ps_output(stdout: &str, limit: usize) -> Vec<SandboxProcess> {
    stdout
        .lines()
        .filter_map(parse_process_line)
        .take(limit)
        .collect()
}

fn doctor_status(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "pass",
        CheckStatus::Warn => "warn",
        CheckStatus::Fail => "fail",
    }
}

async fn read_sandbox_stats(
    agent: &str,
    sandbox: &Sandbox,
) -> miette::Result<SandboxStatsResponse> {
    let limit = SANDBOX_PROCESS_LIMIT.to_string();
    let command = [
        "sh",
        "-c",
        SANDBOX_STATS_SCRIPT,
        "dashboard-sandbox-stats",
        limit.as_str(),
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);
    let (stdout, exit_code) =
        match tokio::time::timeout(timeout, exec_argv(sandbox, &command)).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(miette::miette!(
                    "sandbox probe timed out after {}s",
                    super::DASHBOARD_SANDBOX_TIMEOUT_SECS
                ));
            }
        };
    if exit_code != 0 {
        return Err(miette::miette!(
            "sandbox stats probe exited with code {exit_code}"
        ));
    }

    Ok(parse_sandbox_stats_output(agent, &stdout))
}

fn parse_sandbox_stats_output(agent: &str, stdout: &str) -> SandboxStatsResponse {
    let mut section = "";
    let mut disk = None;
    let mut mem_total_kib = None;
    let mut mem_available_kib = None;
    let mut load_average_1m = None;
    let mut load_average_5m = None;
    let mut load_average_15m = None;
    let mut ps_stdout = String::new();

    for line in stdout.lines() {
        match line {
            "__DISK__" | "__MEM__" | "__LOAD__" | "__PS__" => {
                section = line;
            }
            _ if section == "__DISK__" => {
                disk = parse_disk_line(line);
            }
            _ if section == "__MEM__" => {
                let mut parts = line.split_whitespace();
                mem_total_kib = parts.next().and_then(|part| part.parse::<u64>().ok());
                mem_available_kib = parts.next().and_then(|part| part.parse::<u64>().ok());
            }
            _ if section == "__LOAD__" => {
                let mut parts = line.split_whitespace();
                load_average_1m = parts.next().and_then(|part| part.parse::<f64>().ok());
                load_average_5m = parts.next().and_then(|part| part.parse::<f64>().ok());
                load_average_15m = parts.next().and_then(|part| part.parse::<f64>().ok());
            }
            _ if section == "__PS__" => {
                ps_stdout.push_str(line);
                ps_stdout.push('\n');
            }
            _ => {}
        }
    }

    let total_bytes = mem_total_kib.map(kib_to_bytes);
    let available_bytes = mem_available_kib.map(kib_to_bytes);
    let used_bytes = total_bytes
        .zip(available_bytes)
        .map(|(total, available)| total.saturating_sub(available));
    let memory = if total_bytes.is_some()
        || available_bytes.is_some()
        || load_average_1m.is_some()
        || load_average_5m.is_some()
        || load_average_15m.is_some()
    {
        Some(SandboxMemoryStats {
            total_bytes,
            available_bytes,
            used_bytes,
            load_average_1m,
            load_average_5m,
            load_average_15m,
        })
    } else {
        None
    };

    SandboxStatsResponse {
        agent: agent.to_owned(),
        source: "sandbox".to_owned(),
        warning: None,
        disk,
        memory,
        processes: parse_ps_output(&ps_stdout, SANDBOX_PROCESS_LIMIT),
    }
}

fn parse_disk_line(line: &str) -> Option<SandboxDiskStats> {
    let mut parts = line.split_whitespace();
    let total_bytes = kib_to_bytes(parts.next()?.parse().ok()?);
    let used_bytes = kib_to_bytes(parts.next()?.parse().ok()?);
    let available_bytes = kib_to_bytes(parts.next()?.parse().ok()?);
    let used_percent = parts.next()?.trim_end_matches('%').parse().ok()?;
    let mount = parts.next()?.to_owned();

    Some(SandboxDiskStats {
        mount,
        total_bytes,
        used_bytes,
        available_bytes,
        used_percent,
    })
}

fn parse_process_line(line: &str) -> Option<SandboxProcess> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let ppid = parts.next()?.parse().ok()?;
    let cpu_percent = parts.next()?.parse().ok()?;
    let memory_percent = parts.next()?.parse().ok()?;
    let rss_bytes = kib_to_bytes(parts.next()?.parse().ok()?);
    let command = cap_command(&parts.collect::<Vec<_>>().join(" "));

    if command.is_empty() {
        return None;
    }

    Some(SandboxProcess {
        pid,
        ppid,
        cpu_percent,
        memory_percent,
        rss_bytes,
        command,
    })
}

fn cap_command(command: &str) -> String {
    command.chars().take(SANDBOX_COMMAND_LIMIT_CHARS).collect()
}

fn kib_to_bytes(kib: u64) -> u64 {
    kib.saturating_mul(1024)
}

fn unavailable_sandbox_stats(agent: &str, detail: &str) -> SandboxStatsResponse {
    SandboxStatsResponse {
        agent: agent.to_owned(),
        source: "unavailable".to_owned(),
        warning: Some(detail.to_owned()),
        disk: None,
        memory: None,
        processes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use right_agent::doctor::{CheckStatus, DoctorCheck};

    use super::*;

    #[tokio::test]
    async fn parse_process_lines_bounds_process_count() {
        let long_command = format!("{}{}", "x".repeat(180), " tail");
        let stdout = format!(
            "1 0 0.0 0.1 123 /sbin/init\n2 1 5.5 1.2 256 /bin/sh -c echo hello\nbad line\n3 1 0.1 0.2 512 {long_command}\n"
        );

        let processes = parse_ps_output(&stdout, 2);

        assert_eq!(processes.len(), 2);
        assert_eq!(processes[0].pid, 1);
        assert_eq!(processes[0].ppid, 0);
        assert_eq!(processes[0].rss_bytes, 123 * 1024);
        assert_eq!(processes[1].command, "/bin/sh -c echo hello");

        let capped = parse_ps_output(&stdout, 10)
            .into_iter()
            .find(|process| process.pid == 3)
            .expect("third process parsed");
        assert_eq!(capped.command.chars().count(), 160);
    }

    #[tokio::test]
    async fn doctor_response_groups_statuses() {
        let checks = vec![
            DoctorCheck {
                name: "right".to_string(),
                status: CheckStatus::Pass,
                detail: "found".to_string(),
                fix: None,
            },
            DoctorCheck {
                name: "tunnel".to_string(),
                status: CheckStatus::Warn,
                detail: "not configured".to_string(),
                fix: Some("configure tunnel".to_string()),
            },
            DoctorCheck {
                name: "agents/".to_string(),
                status: CheckStatus::Fail,
                detail: "missing".to_string(),
                fix: None,
            },
        ];

        let response = doctor_response_from_checks("alpha", checks);

        assert_eq!(response.agent, "alpha");
        assert_eq!(response.pass_count, 1);
        assert_eq!(response.warn_count, 1);
        assert_eq!(response.fail_count, 1);
        assert_eq!(response.pass[0].name, "right");
        assert_eq!(response.warn[0].status, "warn");
        assert_eq!(response.warn[0].fix.as_deref(), Some("configure tunnel"));
        assert_eq!(response.fail[0].name, "agents/");
    }

    #[tokio::test]
    async fn parse_sandbox_stats_output_uses_df_mountpoint() {
        let stdout = "__DISK__\n1000 250 750 25% /sandbox\n__MEM__\n100 40\n__LOAD__\n1.00 0.50 0.25\n__PS__\n1 0 0.0 0.1 10 /init\n";

        let response = parse_sandbox_stats_output("alpha", stdout);

        let disk = response.disk.expect("disk stats");
        assert_eq!(disk.mount, "/sandbox");
        assert_eq!(disk.total_bytes, 1000 * 1024);
        assert_eq!(disk.used_percent, 25.0);
    }
}
