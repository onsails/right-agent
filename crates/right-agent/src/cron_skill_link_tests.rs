use super::*;
use right_db::Connection;

async fn conn() -> (tempfile::TempDir, Connection) {
    right_db::test_support::migrated_connection().await
}

async fn seed_job(conn: &Connection, job: &str) {
    conn.execute(
        "INSERT INTO cron_specs \
         (job_name, schedule, prompt, max_budget_usd, created_at, updated_at) \
         VALUES (?1, '17 9 * * *', 'do x', 2.0, '2026-06-15T00:00:00Z', '2026-06-15T00:00:00Z')",
        params![job],
    )
    .await
    .unwrap();
}

async fn seed_skill(conn: &Connection, name: &str, state: &str) {
    conn.execute(
        "INSERT INTO skill_lifecycle \
         (skill_name, state, created_by, created_at) \
         VALUES (?1, ?2, 'cron', '2026-06-15T00:00:00Z')",
        params![name, state],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn link_auto_is_idempotent() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    let skills = vec!["rightx-a".to_string()];
    link_auto(&c, "j", &skills).await.unwrap();
    link_auto(&c, "j", &skills).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-a"]);
}

#[tokio::test]
async fn link_agent_validates_job_and_skill() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    let err = link_agent(&c, "j", &["rightx-missing".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, LinkError::SkillNotFound(_)));
    seed_skill(&c, "rightx-a", "active").await;
    let err = link_agent(&c, "nope", &["rightx-a".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, LinkError::JobNotFound(_)));
    link_agent(&c, "j", &["rightx-a".into()]).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-a"]);
}

#[tokio::test]
async fn list_live_excludes_archived() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    seed_skill(&c, "rightx-live", "active").await;
    seed_skill(&c, "rightx-dead", "archived").await;
    link_auto(&c, "j", &["rightx-live".into(), "rightx-dead".into()])
        .await
        .unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap().len(), 2);
    assert_eq!(
        list_live_for_job(&c, "j").await.unwrap(),
        vec!["rightx-live"]
    );
}

#[tokio::test]
async fn redirect_moves_links_pk_safe() {
    let (_t, c) = conn().await;
    seed_job(&c, "j1").await;
    seed_job(&c, "j2").await;
    link_auto(&c, "j1", &["rightx-old".into()]).await.unwrap();
    link_auto(&c, "j2", &["rightx-old".into(), "rightx-new".into()])
        .await
        .unwrap();
    redirect_skill(&c, "rightx-old", "rightx-new")
        .await
        .unwrap();
    assert_eq!(list_for_job(&c, "j1").await.unwrap(), vec!["rightx-new"]);
    assert_eq!(list_for_job(&c, "j2").await.unwrap(), vec!["rightx-new"]);
}

#[tokio::test]
async fn redirect_skill_self_reference_is_noop() {
    // old == new: INSERT OR IGNORE is a no-op and the DELETE would wipe every
    // link, so the guard must short-circuit and leave the link intact.
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    link_auto(&c, "j", &["rightx-x".into()]).await.unwrap();
    redirect_skill(&c, "rightx-x", "rightx-x").await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-x"]);
}

#[tokio::test]
async fn unlink_and_drop() {
    let (_t, c) = conn().await;
    seed_job(&c, "j").await;
    link_auto(&c, "j", &["rightx-a".into(), "rightx-b".into()])
        .await
        .unwrap();
    unlink_agent(&c, "j", &["rightx-a".into()]).await.unwrap();
    assert_eq!(list_for_job(&c, "j").await.unwrap(), vec!["rightx-b"]);
    drop_skill(&c, "rightx-b").await.unwrap();
    assert!(list_for_job(&c, "j").await.unwrap().is_empty());
}
