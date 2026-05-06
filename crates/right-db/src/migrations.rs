use rusqlite_migration::Migrations;

/// Per-agent SQLite migration registry. Single source of truth for
/// every table the `right` platform writes to `data.db`.
pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(Vec::new()));
