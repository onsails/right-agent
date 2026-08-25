//! Offline legacy multiprocess-WAL repair orchestration.
//!
//! `right agent db-repair <name>...` is intentionally narrow: callers select
//! validated agent names only (no database path or SQL). One invocation
//! preflights EVERY selected agent, proves project-wide runtime quiescence
//! ONCE, then repairs the databases sequentially through the single public
//! `right_db::repair_legacy_wal` filesystem transaction.

use std::path::Path;
use std::time::Duration;

use right_agent::runtime::PcClient;
use right_db::{RepairReport, RepairRequest};

/// Production deadline from the approved recovery plan.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

struct PreflightAgent {
    name: String,
    agent_dir: std::path::PathBuf,
    backups_dir: std::path::PathBuf,
}

/// CLI entry point. Does not restart automatically: acceptance of the repaired
/// manifests is deliberately separate from mutation.
pub(crate) async fn cmd_agent_db_repair(home: &Path, names: &[String]) -> miette::Result<()> {
    let reports = run_db_repair(home, names, SHUTDOWN_TIMEOUT).await?;
    render_success(names, &reports)?;
    Ok(())
}

/// Preflight every selected agent, establish one project-wide quiescence
/// session, then repair sequentially. If a later repair fails, earlier
/// successful manifests remain and the runtime stays down (no restart path in
/// this module).
async fn run_db_repair(
    home: &Path,
    names: &[String],
    shutdown_timeout: Duration,
) -> miette::Result<Vec<RepairReport>> {
    run_db_repair_with(home, names, shutdown_timeout, |_name, request| async move {
        right_db::repair_legacy_wal(request).await
    })
    .await
}

/// Repair-function seam used to deterministically inject a mid-swap failure
/// while retaining the real preflight/quiescence orchestration in tests.
async fn run_db_repair_with<F, Fut>(
    home: &Path,
    names: &[String],
    shutdown_timeout: Duration,
    mut repair: F,
) -> miette::Result<Vec<RepairReport>>
where
    F: FnMut(String, RepairRequest) -> Fut,
    Fut: Future<Output = Result<RepairReport, right_db::DbError>>,
{
    let agents = preflight_agents(home, names)?;

    // Exclude `right up` before inspecting state, then retain the guard through
    // shutdown, the state recheck, and every filesystem repair transaction.
    let quiescence_guard = right_agent::runtime::acquire_runtime_exclusion(home).await?;
    if let Some(client) = PcClient::from_home(home)? {
        client.shutdown_and_wait(shutdown_timeout).await?;
    }
    // Recheck only after shutdown while startup remains excluded. The endpoint
    // may be gone while state.json is intentionally retained as historical state.
    if let Some(client) = PcClient::from_home(home)? {
        client.health_check().await.map_or(Ok(()), |_| {
            Err(miette::miette!(
                "runtime remained reachable after shutdown; refusing database repair"
            ))
        })?;
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut reports = Vec::with_capacity(agents.len());
    for agent in agents {
        let request = RepairRequest {
            agent_dir: agent.agent_dir,
            backups_dir: agent.backups_dir,
            timestamp: timestamp.clone(),
        };
        let report = repair(agent.name.clone(), request).await.map_err(|error| {
            let completed: Vec<&str> = reports
                .iter()
                .map(|report: &RepairReport| {
                    report
                        .db_path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .unwrap_or("unknown")
                })
                .collect();
            if completed.is_empty() {
                miette::miette!(
                    "database repair failed for agent '{}': {error:#}",
                    agent.name
                )
            } else {
                miette::miette!(
                    "database repair failed for agent '{}': {error:#}. Runtime remains down; \
                     prior successful manifests are preserved for: {}",
                    agent.name,
                    completed.join(", ")
                )
            }
        })?;
        reports.push(report);
    }
    drop(quiescence_guard);
    Ok(reports)
}

/// Validate the complete selection before runtime shutdown. No repair starts
/// unless every name is valid, every agent dir exists, and every data.db is a
/// regular file.
fn preflight_agents(home: &Path, names: &[String]) -> miette::Result<Vec<PreflightAgent>> {
    if names.is_empty() {
        return Err(miette::miette!(
            "db-repair requires at least one agent name"
        ));
    }
    let agents_root = right_config::agents_dir(home);
    let mut seen = std::collections::BTreeSet::new();
    let mut agents = Vec::with_capacity(names.len());
    for name in names {
        right_agent::agent::discovery::validate_agent_name(name)
            .map_err(|error| miette::miette!("invalid agent name '{name}': {error:#}"))?;
        if !seen.insert(name.clone()) {
            return Err(miette::miette!(
                "agent '{name}' was selected more than once"
            ));
        }
        let agent_dir = agents_root.join(name);
        if !agent_dir.is_dir() {
            return Err(miette::miette!(
                "agent '{name}' does not exist at {}",
                agent_dir.display()
            ));
        }
        let db_path = agent_dir.join("data.db");
        if !db_path.is_file() {
            return Err(miette::miette!(
                "agent '{name}' has no data.db at {}",
                db_path.display()
            ));
        }
        agents.push(PreflightAgent {
            name: name.clone(),
            agent_dir,
            backups_dir: right_config::backups_dir(home, name),
        });
    }
    Ok(agents)
}

/// All user-facing CLI output goes through right-ui atoms.
fn render_success(names: &[String], reports: &[RepairReport]) -> miette::Result<()> {
    let theme = right_ui::detect();
    println!("{}", right_ui::section(theme, "database repair"));
    println!("{}", right_ui::Rail::blank(theme));
    let mut block = right_ui::Block::new();
    for (name, report) in names.iter().zip(reports) {
        block.push(
            right_ui::status(right_ui::Glyph::Ok)
                .noun(name)
                .verb("recovered")
                .detail(format!(
                    "schema {}, manifest {}",
                    report.schema_version,
                    report.manifest_path.display()
                )),
        );
    }
    let current_exe = std::env::current_exe()
        .map_err(|error| miette::miette!("failed to resolve current executable path: {error:#}"))?;
    block.push(
        right_ui::status(right_ui::Glyph::Info)
            .noun("next")
            .verb("review every manifest, then start explicitly")
            .fix(format!(
                "\"{}\" up --agents {} --detach --non-interactive",
                current_exe.display(),
                names.join(",")
            )),
    );
    println!("{}", block.render(theme));
    println!("{}", right_ui::Rail::blank(theme));
    Ok(())
}

#[cfg(test)]
#[path = "db_repair_tests.rs"]
mod tests;
