use std::collections::HashMap;
use std::str::FromStr;

/// A cron job specification loaded from the database.
#[derive(Debug, Clone, PartialEq)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
}

/// Validate a cron job name: must match `^[a-z0-9][a-z0-9-]*$`.
pub fn validate_job_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("job name must not be empty".into());
    }
    let first = name.as_bytes()[0];
    if first == b'-' {
        return Err("job name must not start with a hyphen".into());
    }
    for ch in name.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
            return Err(format!(
                "job name contains invalid character '{ch}': only lowercase alphanumeric and hyphens allowed"
            ));
        }
    }
    Ok(())
}

/// Validate a 5-field cron schedule expression.
///
/// Returns `Ok(Some(warning))` if the minute field is a round value (0 or 30),
/// `Ok(None)` if valid with no warning, or `Err` if the expression is invalid.
pub fn validate_schedule(schedule: &str) -> Result<Option<String>, String> {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        return Err("schedule must not be empty".into());
    }

    // Convert 5-field to 7-field for the cron crate (seconds + year)
    let seven_field = format!("0 {} *", trimmed);
    cron::Schedule::from_str(&seven_field)
        .map_err(|e| format!("invalid cron schedule '{trimmed}': {e}"))?;

    // Check for round-minute warning
    let minute_field = trimmed.split_whitespace().next().unwrap_or("");
    let is_round = matches!(minute_field, "0" | "00" | "30");
    if is_round {
        Ok(Some(format!(
            "schedule runs at minute {minute_field} — consider offsetting to reduce thundering-herd"
        )))
    } else {
        Ok(None)
    }
}

/// Validate a lock TTL string (e.g. "30m", "1h").
pub fn validate_lock_ttl(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("lock_ttl must not be empty".into());
    }
    let (num_part, suffix) = s.split_at(s.len() - 1);
    if !matches!(suffix, "m" | "h") {
        return Err(format!(
            "lock_ttl must end with 'm' or 'h', got '{s}'"
        ));
    }
    num_part
        .parse::<i64>()
        .map_err(|_| format!("lock_ttl numeric part '{num_part}' is not a valid integer"))?;
    Ok(())
}

/// Load all cron specs from the `cron_specs` table.
///
/// Logs warnings for schedules that hit round minutes.
pub fn load_specs_from_db(conn: &rusqlite::Connection) -> HashMap<String, CronSpec> {
    let mut specs = HashMap::new();
    let mut stmt = match conn
        .prepare("SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd FROM cron_specs")
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to prepare cron_specs query: {e}");
            return specs;
        }
    };

    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("failed to query cron_specs: {e}");
            return specs;
        }
    };

    for row in rows {
        let (job_name, schedule, prompt, lock_ttl, max_budget_usd) = match row {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("skipping malformed cron_specs row: {e}");
                continue;
            }
        };

        if let Ok(Some(warning)) = validate_schedule(&schedule) {
            tracing::warn!(job = %job_name, "{warning}");
        }

        specs.insert(
            job_name,
            CronSpec {
                schedule,
                prompt,
                lock_ttl,
                max_budget_usd,
            },
        );
    }

    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_job_name_valid() {
        assert!(validate_job_name("health-check").is_ok());
        assert!(validate_job_name("a").is_ok());
        assert!(validate_job_name("deploy-check-123").is_ok());
    }

    #[test]
    fn validate_job_name_invalid() {
        assert!(validate_job_name("").is_err());
        assert!(validate_job_name("-leading").is_err());
        assert!(validate_job_name("UPPER").is_err());
        assert!(validate_job_name("has space").is_err());
        assert!(validate_job_name("under_score").is_err());
    }

    #[test]
    fn validate_schedule_valid() {
        assert!(validate_schedule("*/5 * * * *").is_ok());
        assert!(validate_schedule("17 9 * * 1-5").is_ok());
    }

    #[test]
    fn validate_schedule_invalid() {
        assert!(validate_schedule("not a cron").is_err());
        assert!(validate_schedule("").is_err());
    }

    #[test]
    fn validate_schedule_round_minutes_warning() {
        assert!(validate_schedule("0 9 * * *").unwrap().is_some());
        assert!(validate_schedule("30 9 * * *").unwrap().is_some());
        assert!(validate_schedule("17 9 * * *").unwrap().is_none());
    }

    #[test]
    fn validate_lock_ttl_valid() {
        assert!(validate_lock_ttl("30m").is_ok());
        assert!(validate_lock_ttl("1h").is_ok());
    }

    #[test]
    fn validate_lock_ttl_invalid() {
        assert!(validate_lock_ttl("bad").is_err());
        assert!(validate_lock_ttl("30").is_err());
        assert!(validate_lock_ttl("").is_err());
    }

    #[test]
    fn load_specs_from_db_empty() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS
            .to_latest(&mut conn)
            .unwrap();
        let specs = load_specs_from_db(&conn);
        assert!(specs.is_empty());
    }

    #[test]
    fn load_specs_from_db_returns_all() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::migrations::MIGRATIONS
            .to_latest(&mut conn)
            .unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
             VALUES ('job1', '*/5 * * * *', 'do stuff', 0.5, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, created_at, updated_at) \
             VALUES ('job2', '17 9 * * *', 'other', '1h', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let specs = load_specs_from_db(&conn);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs["job1"].schedule, "*/5 * * * *");
        assert_eq!(specs["job1"].max_budget_usd, 0.5);
        assert_eq!(specs["job2"].lock_ttl.as_deref(), Some("1h"));
    }
}
