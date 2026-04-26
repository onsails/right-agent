/// Save an auth token, replacing any existing one.
pub fn save_auth_token(conn: &rusqlite::Connection, token: &str) -> Result<(), rusqlite::Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM auth_tokens", [])?;
    tx.execute(
        "INSERT INTO auth_tokens (token) VALUES (?1)",
        rusqlite::params![token],
    )?;
    tx.commit()?;
    Ok(())
}

/// Get the stored auth token, if any.
pub fn get_auth_token(conn: &rusqlite::Connection) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT token FROM auth_tokens LIMIT 1")?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Delete the stored auth token.
pub fn delete_auth_token(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM auth_tokens", [])?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
