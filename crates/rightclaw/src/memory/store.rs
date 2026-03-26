use rusqlite::{Connection, OptionalExtension};

use super::{guard, MemoryError};

/// A memory entry returned from the store.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub content: String,
    pub tags: Option<String>,
    pub stored_by: Option<String>,
    pub source_tool: Option<String>,
    pub created_at: String,
    pub importance: f64,
}

/// Store new memory content in the database.
///
/// - Rejects content matching any injection pattern (returns `Err(MemoryError::InjectionDetected)`).
/// - Inserts a row in `memories` and a `"store"` event in `memory_events`.
/// - Returns the new memory ID.
pub fn store_memory(
    conn: &Connection,
    content: &str,
    tags: Option<&str>,
    stored_by: Option<&str>,
    source_tool: Option<&str>,
) -> Result<i64, MemoryError> {
    if guard::has_injection(content) {
        return Err(MemoryError::InjectionDetected);
    }
    conn.execute(
        "INSERT INTO memories (content, tags, stored_by, source_tool) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![content, tags, stored_by, source_tool],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO memory_events (memory_id, event_type, actor) VALUES (?1, 'store', ?2)",
        rusqlite::params![id, stored_by],
    )?;
    Ok(id)
}

/// Recall memories matching `query` against tags or content (LIKE search).
///
/// Returns up to 50 non-deleted entries ordered by `created_at DESC`.
pub fn recall_memories(
    conn: &Connection,
    query: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, content, tags, stored_by, source_tool, created_at, importance \
         FROM memories \
         WHERE deleted_at IS NULL \
           AND (tags LIKE '%' || ?1 || '%' OR content LIKE '%' || ?1 || '%') \
         ORDER BY created_at DESC \
         LIMIT 50",
    )?;
    let entries = stmt
        .query_map([query], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                stored_by: row.get(3)?,
                source_tool: row.get(4)?,
                created_at: row.get(5)?,
                importance: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// Full-text search memories using FTS5 BM25 ranking.
///
/// Returns up to 50 non-deleted entries ranked by relevance (best match first).
/// Uses `memories_fts MATCH` — FTS5 query syntax applies.
pub fn search_memories(
    conn: &Connection,
    query: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.content, m.tags, m.stored_by, m.source_tool, m.created_at, m.importance \
         FROM memories m \
         JOIN memories_fts f ON m.id = f.rowid \
         WHERE memories_fts MATCH ?1 \
           AND m.deleted_at IS NULL \
         ORDER BY bm25(memories_fts) \
         LIMIT 50",
    )?;
    let entries = stmt
        .query_map([query], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                stored_by: row.get(3)?,
                source_tool: row.get(4)?,
                created_at: row.get(5)?,
                importance: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// List all non-deleted memories ordered by created_at DESC.
///
/// Returns up to `limit` entries starting at `offset`.
pub fn list_memories(
    conn: &Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, content, tags, stored_by, source_tool, created_at, importance \
         FROM memories \
         WHERE deleted_at IS NULL \
         ORDER BY created_at DESC, id DESC \
         LIMIT ?1 OFFSET ?2",
    )?;
    let entries = stmt
        .query_map(rusqlite::params![limit, offset], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                stored_by: row.get(3)?,
                source_tool: row.get(4)?,
                created_at: row.get(5)?,
                importance: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// Full-text search memories with explicit pagination.
///
/// Unlike `search_memories` (hardcoded LIMIT 50 for MCP), this function
/// accepts operator-controlled limit/offset for CLI pagination.
/// FTS5 BM25 ranking applies. FTS5 query syntax errors surface as MemoryError::Sqlite.
pub fn search_memories_paged(
    conn: &Connection,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.content, m.tags, m.stored_by, m.source_tool, m.created_at, m.importance \
         FROM memories m \
         JOIN memories_fts f ON m.id = f.rowid \
         WHERE memories_fts MATCH ?1 \
           AND m.deleted_at IS NULL \
         ORDER BY bm25(memories_fts) \
         LIMIT ?2 OFFSET ?3",
    )?;
    let entries = stmt
        .query_map(rusqlite::params![query, limit, offset], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                tags: row.get(2)?,
                stored_by: row.get(3)?,
                source_tool: row.get(4)?,
                created_at: row.get(5)?,
                importance: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// Hard-delete a memory entry by ID (operator bypass of soft-delete).
///
/// Removes the `memories` row entirely. `memory_events` rows are preserved
/// (the `memories_ad` trigger fires and removes the FTS index entry — this is correct).
///
/// Unlike `forget_memory`, this succeeds even if the row is already soft-deleted
/// (`deleted_at IS NOT NULL`) — operators can hard-delete any existing row.
///
/// Returns `Err(MemoryError::NotFound(id))` if no row exists with that id.
pub fn hard_delete_memory(conn: &Connection, id: i64) -> Result<(), MemoryError> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM memories WHERE id = ?1",
            [id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !exists {
        return Err(MemoryError::NotFound(id));
    }
    conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
    Ok(())
}

/// Soft-delete a memory by ID.
///
/// - Sets `deleted_at = datetime('now')` on the memory row.
/// - Inserts a `"forget"` event in `memory_events`.
/// - Returns `Err(MemoryError::NotFound(id))` if the memory doesn't exist or is already deleted.
pub fn forget_memory(
    conn: &Connection,
    id: i64,
    actor: Option<&str>,
) -> Result<(), MemoryError> {
    // Verify the row exists and is not already deleted
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM memories WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !exists {
        return Err(MemoryError::NotFound(id));
    }
    conn.execute(
        "UPDATE memories SET deleted_at = datetime('now') WHERE id = ?1",
        [id],
    )?;
    conn.execute(
        "INSERT INTO memory_events (memory_id, event_type, actor) VALUES (?1, 'forget', ?2)",
        rusqlite::params![id, actor],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
