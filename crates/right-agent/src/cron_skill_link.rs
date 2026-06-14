//! Explicit cron ↔ skill links (`cron_skill_links` table). Forward direction
//! (a cron's skills) is the runtime use; the reverse index serves curator
//! cleanup. Writes that touch 2+ rows run in one immediate transaction.

use right_db::{Connection, DbError, params};

/// Error surface for the validated agent-facing link/unlink operations.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("job '{0}' not found")]
    JobNotFound(String),
    #[error("skill '{0}' not found or archived")]
    SkillNotFound(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Upsert links with `origin='auto'` (platform). Idempotent; never overwrites an
/// existing agent link. No validation — callers pass skills they just authored.
pub async fn link_auto(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<(), DbError> {
    if skill_names.is_empty() {
        return Ok(());
    }
    let now = now();
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES (?1, ?2, 'auto', ?3)",
            params![job_name, s, &now],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Validate the job and each skill exist, then upsert `origin='agent'` links.
pub async fn link_agent(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<String, LinkError> {
    ensure_job_exists(conn, job_name).await?;
    for s in skill_names {
        ensure_skill_live(conn, s).await?;
    }
    let now = now();
    // Job/skill liveness is validated above, outside this tx; a concurrent archive
    // between check and insert is intentionally tolerated since `list_live_for_job`
    // re-checks liveness at read time, leaving any stale link inert.
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
             VALUES (?1, ?2, 'agent', ?3)",
            params![job_name, s, &now],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(format!(
        "Linked {} skill(s) to cron '{job_name}'.",
        skill_names.len()
    ))
}

/// Remove the given links from a job. Unknown links are ignored.
pub async fn unlink_agent(
    conn: &Connection,
    job_name: &str,
    skill_names: &[String],
) -> Result<String, LinkError> {
    ensure_job_exists(conn, job_name).await?;
    let tx = conn.transaction().await?;
    for s in skill_names {
        tx.execute(
            "DELETE FROM cron_skill_links WHERE job_name = ?1 AND skill_name = ?2",
            params![job_name, s],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(format!(
        "Unlinked {} skill(s) from cron '{job_name}'.",
        skill_names.len()
    ))
}

/// All linked skill names for a job (no liveness filter).
pub async fn list_for_job(conn: &Connection, job_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT skill_name FROM cron_skill_links WHERE job_name = ?1 ORDER BY skill_name",
        params![job_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Linked skill names for a job, excluding archived skills. Used at fire time.
pub async fn list_live_for_job(conn: &Connection, job_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT l.skill_name FROM cron_skill_links l \
         JOIN skill_lifecycle s ON s.skill_name = l.skill_name \
         WHERE l.job_name = ?1 AND s.state != 'archived' \
         ORDER BY l.skill_name",
        params![job_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Jobs linking a given skill (reverse index; curator cleanup).
pub async fn jobs_for_skill(conn: &Connection, skill_name: &str) -> Result<Vec<String>, DbError> {
    conn.query_all(
        "SELECT job_name FROM cron_skill_links WHERE skill_name = ?1 ORDER BY job_name",
        params![skill_name],
        |row| row.get::<_, String>(0),
    )
    .await
}

/// Repoint every link from `old` to `new` (skill absorbed). PK-safe: copy then
/// drop, ignoring rows where the job already links `new`.
pub async fn redirect_skill(conn: &Connection, old: &str, new: &str) -> Result<(), DbError> {
    let tx = conn.transaction().await?;
    tx.execute(
        "INSERT OR IGNORE INTO cron_skill_links (job_name, skill_name, origin, created_at) \
         SELECT job_name, ?2, origin, created_at FROM cron_skill_links WHERE skill_name = ?1",
        params![old, new],
    )
    .await?;
    tx.execute(
        "DELETE FROM cron_skill_links WHERE skill_name = ?1",
        params![old],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Drop all links to a skill (skill retired without a successor).
pub async fn drop_skill(conn: &Connection, skill_name: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM cron_skill_links WHERE skill_name = ?1",
        params![skill_name],
    )
    .await?;
    Ok(())
}

async fn ensure_job_exists(conn: &Connection, job_name: &str) -> Result<(), LinkError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cron_specs WHERE job_name = ?1",
            params![job_name],
            |r| r.get(0),
        )
        .await?;
    if n == 0 {
        return Err(LinkError::JobNotFound(job_name.to_owned()));
    }
    Ok(())
}

async fn ensure_skill_live(conn: &Connection, skill_name: &str) -> Result<(), LinkError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_lifecycle WHERE skill_name = ?1 AND state != 'archived'",
            params![skill_name],
            |r| r.get(0),
        )
        .await?;
    if n == 0 {
        return Err(LinkError::SkillNotFound(skill_name.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "cron_skill_link_tests.rs"]
mod tests;
