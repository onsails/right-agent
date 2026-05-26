/// Add tar excludes for database files handled outside sandbox.tar.gz.
///
/// `data.db` is copied via a canonical VACUUM snapshot. `data.db-*` files are
/// runtime sidecars for SQLite/Turso and must not become durable backup state.
pub fn push_no_sandbox_database_tar_excludes(tar_args: &mut Vec<String>, agent_name: &str) {
    tar_args.push("--exclude=data.db".to_string());
    tar_args.push("--exclude=data.db-*".to_string());
    tar_args.push(format!("--exclude={agent_name}/data.db"));
    tar_args.push(format!("--exclude={agent_name}/data.db-*"));
}
