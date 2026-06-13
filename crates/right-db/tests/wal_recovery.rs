//! End-to-end harness for `open_connection` self-healing on a real WAL-sidecar
//! desync. The desync cannot be synthesized deterministically and is not even
//! reliably reproducible from captured bytes — Turso may open the same sidecar
//! files cleanly on replay (experimental multiprocess WAL, tursodatabase/turso#769).
//! So this test runs only when `RIGHT_WAL_FIXTURE` points at a fixture dir
//! (data.db + data.db-{shm,tshm,wal}) and asserts only the robust postcondition:
//! `open_connection` returns a usable connection. On a fixture that still
//! triggers the desync, success proves recovery fired (open fails without it);
//! on one Turso opens directly, it succeeds trivially. Self-skips when unset.

use std::path::PathBuf;

#[tokio::test]
async fn open_connection_self_heals_wal_desync_fixture() {
    let Some(src) = std::env::var("RIGHT_WAL_FIXTURE").ok().map(PathBuf::from) else {
        eprintln!("RIGHT_WAL_FIXTURE unset — skipping live WAL-desync recovery test");
        return;
    };
    assert!(src.is_dir(), "RIGHT_WAL_FIXTURE must be a directory");

    let dir = tempfile::tempdir().unwrap();
    for name in ["data.db", "data.db-shm", "data.db-tshm", "data.db-wal"] {
        let from = src.join(name);
        if from.exists() {
            std::fs::copy(&from, dir.path().join(name)).unwrap();
        }
    }

    // Robust postcondition: a usable connection comes back. We do NOT assert the
    // sidecars were reset, because the fixture may not re-trigger the desync on
    // this run (see module doc) — recovery is unit-tested separately.
    let conn = right_db::open_connection(dir.path(), false)
        .await
        .expect("open_connection must yield a usable connection on a WAL-desync fixture");

    let n: i64 = conn
        .query_row("SELECT count(*) FROM cron_specs", (), |r| r.get(0))
        .await
        .expect("a known table must be readable");
    assert!(n >= 0);
}
