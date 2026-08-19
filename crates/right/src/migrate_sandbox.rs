//! `right agent migrate-sandbox <agent>` — move an agent out of its OpenShell
//! sandbox and into a microsandbox VM.
//!
//! Ordering is the entire safety story. The OpenShell sandbox is deleted only
//! after the new sandbox has been created, restored, and *verified*; every
//! earlier failure deletes the half-built microVM and leaves both the OpenShell
//! sandbox and the on-disk `agent.yaml` exactly as they were, so re-running the
//! command after a partial failure starts from the same clean state.
//!
//! Credentials are the one thing that cannot come along: OpenShell redacts a
//! provider's value on every read path, so it is unreadable by design. The
//! command reports the providers the old sandbox had attached and points the
//! operator at the dashboard's `/providers` flow — the same path that adds a
//! provider to any other agent.
//!
//! Everything that touches OpenShell lives in [`legacy_openshell`], a frozen
//! CLI-only read path that goes away with this command.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use right_agent::sandbox_migrate::{
    MIGRATION_EXCLUDES, carried_entries, hand_home_to_guest_user, restore_archive, verify_restore,
};

mod legacy_openshell;

/// Guest home in *both* runtimes: OpenShell's sandbox root and
/// [`right_sandbox::GUEST_HOME`] are both `/sandbox`, which is why the archive
/// the migration downloads has the same `sandbox/…` member layout as a
/// microsandbox backup and restores through the same `--strip-components=1`.
const OPENSHELL_GUEST_HOME: &str = "/sandbox";

/// How long the OpenShell-side `tar` stream may take.
const ARCHIVE_TIMEOUT_SECS: u64 = 900;

/// How long the source sandbox gets to reach READY before the migration gives
/// up on reading it.
const OPENSHELL_READY_TIMEOUT_SECS: u64 = 120;

/// How long to wait for the OpenShell gateway to confirm the old sandbox is
/// gone.
const OPENSHELL_DELETE_TIMEOUT_SECS: u64 = 180;

/// Poll interval for the OpenShell readiness/deletion waits.
const OPENSHELL_POLL_INTERVAL_SECS: u64 = 2;

/// How long the `ls` probe that captures the source listing may take.
const SOURCE_LISTING_TIMEOUT_SECS: u64 = 30;

/// Where `agent.yaml` says this agent currently lives.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MigrationSource {
    /// `sandbox.mode: openshell` — an unmigrated agent whose home is still in
    /// the named (or derived) OpenShell sandbox.
    OpenShell { explicit_name: Option<String> },
    /// No `sandbox.mode:` key: the agent already runs in a microsandbox VM and
    /// the command is a no-op.
    AlreadyMigrated,
}

/// Minimal read-only view of the *unmigrated* `agent.yaml`.
///
/// The real parser (`right_agent_config`) deliberately rejects
/// `sandbox.mode: openshell` — that rejection is what stops an unmigrated agent
/// from starting — so this command cannot use it to read its own input.
#[derive(Debug, Deserialize)]
struct SourceAgentYaml {
    #[serde(default)]
    sandbox: Option<SourceSandbox>,
}

#[derive(Debug, Deserialize)]
struct SourceSandbox {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Classify an `agent.yaml` for migration.
///
/// Any `mode:` other than `openshell` is an error rather than a guess: `none`
/// is the retired sandboxless mode, whose files live on the host and need a
/// different migration, and an unknown value means the file was hand-edited.
pub(crate) fn migration_source(yaml: &str) -> miette::Result<MigrationSource> {
    let parsed: SourceAgentYaml = serde_saphyr::from_str(yaml)
        .map_err(|e| miette::miette!("agent.yaml is not valid YAML: {e}"))?;
    let Some(sandbox) = parsed.sandbox else {
        return Ok(MigrationSource::AlreadyMigrated);
    };
    match sandbox.mode.as_deref() {
        None => Ok(MigrationSource::AlreadyMigrated),
        Some("openshell") => Ok(MigrationSource::OpenShell {
            explicit_name: sandbox.name,
        }),
        Some("none") => Err(miette::miette!(
            help = "Sandboxless agents keep their files on the host; this command only moves an OpenShell sandbox.",
            "`sandbox.mode: none` is not an OpenShell agent"
        )),
        Some(other) => Err(miette::miette!(
            "unknown `sandbox.mode: {other}` in agent.yaml; only `openshell` can be migrated"
        )),
    }
}

/// Rewrite an unmigrated `agent.yaml` for the microsandbox world.
///
/// Drops the two retired keys (`mode:`, `policy_file:`) and writes the new
/// `sandbox.name`, leaving every other line — comments, providers, key order —
/// untouched. Deliberately *not*
/// `crate::wizard::update_agent_yaml_sandbox_name`: that helper preserves
/// `mode:`, which is precisely the line that makes the agent unstartable.
pub(crate) fn rewrite_agent_yaml_for_migration(yaml: &str, sandbox_name: &str) -> String {
    let name_line = format!("  name: \"{sandbox_name}\"");
    let mut lines: Vec<String> = Vec::new();
    let mut in_sandbox_block = false;
    let mut wrote_block = false;

    for line in yaml.lines() {
        if line.trim_end() == "sandbox:" {
            in_sandbox_block = true;
            wrote_block = true;
            lines.push("sandbox:".to_owned());
            lines.push(name_line.clone());
            continue;
        }
        if in_sandbox_block {
            if line.trim().is_empty() || line.starts_with(' ') || line.starts_with('\t') {
                let key = line.trim_start();
                if key.starts_with("mode:")
                    || key.starts_with("policy_file:")
                    || key.starts_with("name:")
                {
                    continue;
                }
                lines.push(line.to_owned());
                continue;
            }
            in_sandbox_block = false;
        }
        lines.push(line.to_owned());
    }

    if !wrote_block {
        lines.push("sandbox:".to_owned());
        lines.push(name_line);
    }

    let mut output = lines.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Everything the migration learned about the source before touching anything.
struct SourcePlan {
    /// OpenShell sandbox holding the agent's home.
    old_name: String,
    /// Deterministic microsandbox name the agent moves to.
    new_name: String,
    /// `agent.yaml` as it will be written once the restore is verified.
    migrated_yaml: String,
    /// The migrated config, parsed to build the create-time spec.
    migrated_config: right_agent::agent::types::AgentConfig,
}

/// Read `agent.yaml`, decide what the migration will do, and prove the
/// rewritten config parses — all before a single side effect.
fn plan_migration(agent_name: &str, yaml: &str) -> miette::Result<Option<SourcePlan>> {
    let explicit_name = match migration_source(yaml)? {
        MigrationSource::AlreadyMigrated => return Ok(None),
        MigrationSource::OpenShell { explicit_name } => explicit_name,
    };
    let old_name = legacy_openshell::resolve_sandbox_name(agent_name, explicit_name.as_deref());
    let new_name = right_sandbox::sandbox_name(agent_name);
    let migrated_yaml = rewrite_agent_yaml_for_migration(yaml, &new_name);
    let migrated_config: right_agent::agent::types::AgentConfig =
        serde_saphyr::from_str(&migrated_yaml).map_err(|e| {
            miette::miette!("the migrated agent.yaml would not parse — migration aborted: {e}")
        })?;
    Ok(Some(SourcePlan {
        old_name,
        new_name,
        migrated_yaml,
        migrated_config,
    }))
}

/// Download the agent's OpenShell home and capture what the source held.
///
/// The archive is written into `~/.right/backups/<agent>/<stamp>/` and kept:
/// it is the operator's independent copy of the pre-migration home, and it
/// costs nothing to leave behind.
///
/// The listing runs over the same SSH config as the archive stream rather
/// than through a second transport, so a sandbox that can be listed is one
/// that can be read.
async fn archive_openshell_home(
    old_name: &str,
    migration_dir: &Path,
) -> miette::Result<(PathBuf, Vec<String>)> {
    // The SSH config is transport scratch, not backup content: keep it in a
    // temp dir so the migration directory the operator is told to treat as a
    // backup holds only the archive.
    let ssh_dir =
        tempfile::tempdir().map_err(|e| miette::miette!("create ssh working directory: {e:#}"))?;
    let ssh_config = legacy_openshell::generate_ssh_config(old_name, ssh_dir.path()).await?;
    let ssh_host = legacy_openshell::ssh_host_for_sandbox(old_name);

    let listing = legacy_openshell::ssh_exec(
        &ssh_config,
        &ssh_host,
        &["ls", "-A", OPENSHELL_GUEST_HOME],
        SOURCE_LISTING_TIMEOUT_SECS,
    )
    .await
    .map_err(|error| {
        miette::miette!("list '{OPENSHELL_GUEST_HOME}' in sandbox '{old_name}': {error:#}")
    })?;
    let carried = carried_entries(&listing, MIGRATION_EXCLUDES);
    if carried.is_empty() {
        return Err(miette::miette!(
            "sandbox '{old_name}' has nothing to migrate under {OPENSHELL_GUEST_HOME}"
        ));
    }

    let archive = migration_dir.join("sandbox.tar.gz");
    legacy_openshell::ssh_tar_download(
        &ssh_config,
        &ssh_host,
        OPENSHELL_GUEST_HOME,
        &archive,
        MIGRATION_EXCLUDES,
        ARCHIVE_TIMEOUT_SECS,
    )
    .await?;

    Ok((archive, carried))
}

/// `right agent migrate-sandbox <agent>`.
///
/// Steps, in the only order that is safe:
/// 1. read `agent.yaml`; an already-migrated agent is a no-op success,
/// 2. archive the OpenShell home (kept as a backup) and record its listing,
/// 3. create the microsandbox VM from the same spec builder the bot uses,
/// 4. restore the archive and hand the home to the guest user,
/// 5. verify the restore — nothing destructive has happened yet,
/// 6. write the rewritten `agent.yaml`,
/// 7. delete the OpenShell sandbox.
///
/// A failure anywhere in 1–5 deletes the new sandbox and leaves the OpenShell
/// one and the original `agent.yaml` untouched, so the agent stays exactly as
/// runnable as it was and the command can simply be run again.
pub(crate) async fn cmd_agent_migrate_sandbox(home: &Path, agent_name: &str) -> miette::Result<()> {
    let theme = right_ui::detect();
    let agents_dir = right_config::agents_dir(home);
    let agent_dir = agents_dir.join(agent_name);
    if !agent_dir.is_dir() {
        return Err(miette::miette!(
            help = "List agents with: right agent list",
            "agent '{agent_name}' not found at {}",
            agent_dir.display()
        ));
    }
    let yaml_path = agent_dir.join("agent.yaml");
    let yaml = std::fs::read_to_string(&yaml_path)
        .map_err(|e| miette::miette!("read {}: {e:#}", yaml_path.display()))?;

    let Some(plan) = plan_migration(agent_name, &yaml)? else {
        println!(
            "{}",
            right_ui::status(right_ui::Glyph::Ok)
                .noun("agent")
                .verb("already migrated")
                .detail(agent_name)
                .render(theme)
        );
        return Ok(());
    };

    println!(
        "{}",
        right_ui::section(theme, &format!("agent migrate-sandbox: {agent_name}"))
    );

    // Preflight: both runtimes must be usable before anything is copied.
    legacy_openshell::preflight().await?;
    if !legacy_openshell::sandbox_exists(&plan.old_name).await? {
        return Err(miette::miette!(
            help = "Restore the agent from a backup instead: right agent restore",
            "OpenShell sandbox '{}' does not exist, so agent '{agent_name}' has no home to migrate",
            plan.old_name
        ));
    }
    right_sandbox::ensure_runtime_installed()
        .await
        .map_err(|error| miette::miette!("install the sandbox runtime: {error:#}"))?;
    right_sandbox::diagnose_host()
        .map_err(|error| miette::miette!("this host cannot run microVMs: {error:#}"))?;
    legacy_openshell::wait_for_ready(
        &plan.old_name,
        OPENSHELL_READY_TIMEOUT_SECS,
        OPENSHELL_POLL_INTERVAL_SECS,
    )
    .await?;

    // Providers are read before the source is deleted: their values are
    // unreadable by design, so their names are all the operator gets back.
    let attached_providers = legacy_openshell::list_attached_providers(&plan.old_name)
        .await
        .map_err(|error| {
            miette::miette!("list providers attached to '{}': {error:#}", plan.old_name)
        })?;

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let migration_dir =
        right_config::backups_dir(home, agent_name).join(format!("migrate-{stamp}"));
    std::fs::create_dir_all(&migration_dir)
        .map_err(|e| miette::miette!("create {}: {e:#}", migration_dir.display()))?;

    println!(
        "{}",
        right_ui::status(right_ui::Glyph::Info)
            .noun("openshell sandbox")
            .verb("archiving")
            .detail(&plan.old_name)
            .render(theme)
    );
    let (archive, carried) = archive_openshell_home(&plan.old_name, &migration_dir).await?;
    println!(
        "{}",
        right_ui::status(right_ui::Glyph::Ok)
            .noun("archive")
            .verb("written")
            .detail(archive.display().to_string())
            .render(theme)
    );

    let providers = right_providers::ProviderStore::open(home)
        .await
        .map_err(|error| miette::miette!("open provider store: {error:#}"))?;
    let spec = right_bot::agent_sandbox_spec_for(
        agent_name,
        &plan.new_name,
        &plan.migrated_config,
        &providers,
    )
    .await?;

    println!(
        "{}",
        right_ui::status(right_ui::Glyph::Info)
            .noun("sandbox")
            .verb("creating")
            .detail(&plan.new_name)
            .render(theme)
    );
    let sandbox = right_sandbox::SandboxHandle::create_or_attach(&spec)
        .await
        .map_err(|error| miette::miette!("create sandbox '{}': {error:#}", plan.new_name))?;

    // From here the new sandbox exists, so every failure rolls it back.
    let restored = async {
        sandbox
            .wait_ready(right_sandbox::DEFAULT_READY_TIMEOUT)
            .await
            .map_err(|error| {
                miette::miette!("sandbox '{}' never became ready: {error:#}", plan.new_name)
            })?;
        restore_archive(&sandbox, &archive).await?;
        let handed_to_guest = hand_home_to_guest_user(&sandbox).await?;
        verify_restore(&sandbox, &carried, handed_to_guest).await?;
        Ok::<bool, miette::Report>(handed_to_guest)
    }
    .await;

    let handed_to_guest = match restored {
        Ok(handed) => handed,
        Err(error) => {
            // The OpenShell sandbox and agent.yaml are still untouched, so
            // dropping the half-built microVM restores the exact prior state.
            right_sandbox::SandboxHandle::delete(&plan.new_name)
                .await
                .map_err(|cleanup| {
                    miette::miette!(
                        "migration failed: {error:#}; deleting the half-migrated sandbox '{}' also failed: {cleanup:#}",
                        plan.new_name
                    )
                })?;
            return Err(miette::miette!(
                "migration failed: {error:#}. Agent '{agent_name}' still runs from OpenShell sandbox '{}'; its archive is at {}",
                plan.old_name,
                archive.display()
            ));
        }
    };

    // Verified: the agent can now be pointed at the new sandbox.
    std::fs::write(&yaml_path, &plan.migrated_yaml)
        .map_err(|e| miette::miette!("write {}: {e:#}", yaml_path.display()))?;
    right_agent::agent::discovery::parse_agent_config(&agent_dir)?;

    // The migration itself is done and durable: the agent now runs from the
    // new sandbox whatever happens next. A delete failure is therefore
    // reported as leftover work with the exact command to finish it, not as a
    // failed migration the operator would re-run for nothing.
    let deleted = legacy_openshell::delete_sandbox_confirmed(
        &plan.old_name,
        OPENSHELL_DELETE_TIMEOUT_SECS,
        OPENSHELL_POLL_INTERVAL_SECS,
    )
    .await;
    if let Err(error) = &deleted {
        tracing::warn!(
            sandbox = %plan.old_name,
            error = %format!("{error:#}"),
            "migrated agent, but the old OpenShell sandbox could not be deleted"
        );
    }

    println!(
        "{}",
        migration_recap(
            &plan,
            &archive,
            handed_to_guest,
            &attached_providers,
            deleted.err(),
        )
        .render(theme)
    );
    Ok(())
}

/// Final report: what moved, what stayed behind, and what the operator still
/// has to do by hand.
fn migration_recap(
    plan: &SourcePlan,
    archive: &Path,
    handed_to_guest: bool,
    attached_providers: &[String],
    delete_failure: Option<miette::Report>,
) -> right_ui::Recap {
    let mut recap = right_ui::Recap::new("migrated")
        .ok("sandbox", &plan.new_name)
        .ok("archive", &archive.display().to_string());
    recap = match delete_failure {
        None => recap.ok("openshell sandbox", &format!("{} deleted", plan.old_name)),
        Some(error) => recap
            .warn(
                "openshell sandbox",
                &format!("'{}' could not be deleted: {error:#}", plan.old_name),
            )
            .next(&format!(
                "delete it by hand: openshell sandbox delete {}",
                plan.old_name
            )),
    };
    if !handed_to_guest {
        recap = recap.warn(
            "ownership",
            &format!(
                "migrated files are root-owned until provisioning creates the '{}' user",
                right_sandbox::GUEST_USER
            ),
        );
    }
    if !attached_providers.is_empty() {
        recap = recap
            .warn(
                "providers",
                &format!(
                    "credentials could not be carried (OpenShell redacts them): {}",
                    attached_providers.join(", ")
                ),
            )
            .next("re-add each provider from the dashboard: /providers");
    }
    recap
}

#[cfg(test)]
#[path = "migrate_sandbox_tests.rs"]
mod tests;
