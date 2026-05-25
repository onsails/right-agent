//! Shared Telegram alert dedup helpers. Both `memory_alerts` and
//! `learning_alerts` use these to enforce a 24-hour dedup window per
//! `alert_type` key against the `memory_alerts` SQLite table.

use std::path::Path;

use chrono::Utc;

/// Returns true iff an alert of this type has NOT been sent in the last 24
/// hours (i.e., it is safe to send).
pub(crate) async fn should_fire(db: &Path, alert_type: &str) -> bool {
    let conn = match right_db::open_connection(db, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("alerts::should_fire open failed: {e:#}");
            return false;
        }
    };
    let existing: Option<String> = match conn
        .query_row(
            "SELECT first_sent_at FROM memory_alerts WHERE alert_type = ?1",
            [alert_type],
            |r| r.get(0),
        )
        .await
    {
        Ok(v) => Some(v),
        Err(right_db::DbError::NotFound) => None,
        Err(e) => {
            tracing::warn!("alerts::should_fire query failed: {e:#}");
            return false;
        }
    };
    let Some(sent) = existing else {
        return true;
    };
    let parsed = match chrono::DateTime::parse_from_rfc3339(&sent) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("alerts::should_fire parse failed: {e:#}");
            return true;
        }
    };
    Utc::now().signed_duration_since(parsed.with_timezone(&Utc)) > chrono::Duration::hours(24)
}

/// Record that an alert of this type was sent. Idempotent via
/// `ON CONFLICT DO UPDATE`.
pub(crate) async fn record_fire(db: &Path, alert_type: &str) {
    match right_db::open_connection(db, false).await {
        Ok(conn) => {
            let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            if let Err(e) = conn
                .execute(
                    "INSERT INTO memory_alerts(alert_type, first_sent_at) VALUES (?1, ?2) \
                 ON CONFLICT(alert_type) DO UPDATE SET first_sent_at = excluded.first_sent_at",
                    [alert_type, &now],
                )
                .await
            {
                tracing::warn!("alerts::record_fire failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("alerts::record_fire open failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn should_fire_true_when_no_row_exists() {
        let dir = tempdir().unwrap();
        right_db::open_connection(dir.path(), true).await.unwrap();
        assert!(should_fire(dir.path(), "test_type").await);
    }

    #[tokio::test]
    async fn record_fire_then_should_fire_is_false() {
        let dir = tempdir().unwrap();
        right_db::open_connection(dir.path(), true).await.unwrap();
        record_fire(dir.path(), "test_type").await;
        assert!(!should_fire(dir.path(), "test_type").await);
    }
}
