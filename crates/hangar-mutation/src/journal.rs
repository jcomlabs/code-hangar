//! Mutation journal schema.
//!
//! These tables are created only when the `mutation` feature is active; the
//! strict core lane never calls this, so a core-only build has no journal. The
//! journal records intended operations and per-item moves so an interrupted
//! mutation can be recovered (resumed or rolled back) on next launch.

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("mutation journal schema error: {0}")]
pub struct JournalError(#[from] rusqlite::Error);

const JOURNAL_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS operation (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  target_node_id INTEGER,
  target_fingerprint TEXT,
  backup_id INTEGER,
  recovered_bytes INTEGER,
  created_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  error TEXT
);

CREATE TABLE IF NOT EXISTS operation_item (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER NOT NULL REFERENCES operation(id) ON DELETE CASCADE,
  node_id INTEGER,
  action TEXT NOT NULL,          -- move | copy_delete | delete | copy | noop
  from_path TEXT,
  to_path TEXT,
  bytes INTEGER,
  checksum_before TEXT,
  checksum_after TEXT,
  status TEXT NOT NULL           -- pending | done | failed | skipped | rolled_back
);
CREATE INDEX IF NOT EXISTS idx_opitem_op ON operation_item(operation_id, status);

CREATE TABLE IF NOT EXISTS backup (
  id INTEGER PRIMARY KEY,
  level TEXT NOT NULL,
  destination TEXT NOT NULL,
  manifest_path TEXT NOT NULL,
  total_bytes INTEGER,
  verified INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS quarantine_entry (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER REFERENCES operation(id),
  original_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  size INTEGER,
  file_count INTEGER,
  risk_level TEXT,
  backup_id INTEGER REFERENCES backup(id),
  space_recovered INTEGER NOT NULL DEFAULT 0,
  scheduled_delete_at TEXT,
  status TEXT NOT NULL,          -- quarantined | restored | restore_content_mismatch | permanently_deleted
  manifest_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS edit_snapshot (
  id INTEGER PRIMARY KEY,
  node_id INTEGER NOT NULL,
  project_id INTEGER NOT NULL,
  path TEXT NOT NULL,
  backup_id INTEGER NOT NULL REFERENCES backup(id),
  bytes INTEGER NOT NULL,
  blake3_before TEXT NOT NULL,
  blake3_after TEXT,
  origin TEXT NOT NULL,          -- manual | value | ai_suggestion | ai_session | restore
  session_id TEXT,
  status TEXT NOT NULL,          -- prepared | saved
  created_at TEXT NOT NULL,
  restored_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_edit_snapshot_node ON edit_snapshot(node_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_edit_snapshot_created ON edit_snapshot(id DESC);
";

/// Idempotently create the mutation journal tables on `conn`.
pub fn ensure_journal_schema(conn: &Connection) -> Result<(), JournalError> {
    conn.execute_batch(JOURNAL_SCHEMA)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
    }

    #[test]
    fn creates_journal_tables_idempotently() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_journal_schema(&conn).unwrap();
        // Idempotent: running twice must not error.
        ensure_journal_schema(&conn).unwrap();

        for table in [
            "operation",
            "operation_item",
            "backup",
            "quarantine_entry",
            "edit_snapshot",
        ] {
            assert!(table_exists(&conn, table), "missing table {table}");
        }
    }
}
