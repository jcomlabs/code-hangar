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
  expected_volume_id TEXT,
  expected_file_id TEXT,
  expected_blake3 TEXT,
  expected_modified_unix_seconds INTEGER,
  result_volume_id TEXT,
  result_file_id TEXT,
  result_blake3 TEXT,
  result_modified_unix_seconds INTEGER,
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

CREATE TABLE IF NOT EXISTS object_archive (
  id INTEGER PRIMARY KEY,
  backup_id INTEGER NOT NULL REFERENCES backup(id),
  quarantine_entry_id INTEGER REFERENCES quarantine_entry(id),
  removal_group_id TEXT,
  original_path TEXT NOT NULL,
  held_path TEXT,
  held_volume_id TEXT,
  held_file_id TEXT,
  held_bytes INTEGER,
  held_modified_unix_seconds INTEGER,
  held_content_blake3 TEXT,
  archive_path TEXT NOT NULL,
  source_volume_id TEXT NOT NULL,
  source_file_id TEXT NOT NULL,
  source_bytes INTEGER NOT NULL,
  source_modified_unix_seconds INTEGER,
  source_content_blake3 TEXT NOT NULL,
  archive_volume_id TEXT NOT NULL,
  archive_file_id TEXT NOT NULL,
  archive_bytes INTEGER NOT NULL,
  archive_modified_unix_seconds INTEGER,
  archive_blake3 TEXT NOT NULL,
  raw_backup_blake3 TEXT NOT NULL,
  semantic_blake3 TEXT NOT NULL,
  roundtrip_blake3 TEXT NOT NULL,
  stream_count INTEGER NOT NULL,
  security_stream_present INTEGER NOT NULL,
  cleanup_complete INTEGER NOT NULL,
  proof_schema TEXT NOT NULL,
  status TEXT NOT NULL,
  blocked_reason TEXT,
  verified_at TEXT,
  UNIQUE(backup_id, original_path)
);
CREATE INDEX IF NOT EXISTS idx_object_archive_backup ON object_archive(backup_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_object_archive_path_nocase
  ON object_archive(archive_path COLLATE NOCASE);
CREATE UNIQUE INDEX IF NOT EXISTS idx_object_archive_legacy_source_nocase
  ON object_archive(backup_id, original_path COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS object_archive_event (
  id INTEGER PRIMARY KEY,
  archive_id INTEGER REFERENCES object_archive(id),
  operation_id INTEGER REFERENCES operation(id),
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  reason_code TEXT,
  message TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS elevation_capability (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER NOT NULL REFERENCES operation(id),
  request_digest TEXT NOT NULL,
  transport_nonce TEXT,
  nonce_digest TEXT NOT NULL,
  helper_image_sha256 TEXT,
  status TEXT NOT NULL,
  issued_at TEXT NOT NULL,
  finished_at TEXT,
  consumed_at TEXT,
  error TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_elevation_capability_nonce
  ON elevation_capability(nonce_digest);
CREATE UNIQUE INDEX IF NOT EXISTS idx_elevation_capability_request
  ON elevation_capability(request_digest);
CREATE TABLE IF NOT EXISTS archive_cleanup (
  id INTEGER PRIMARY KEY,
  archive_id INTEGER REFERENCES object_archive(id),
  operation_id INTEGER REFERENCES operation(id),
  scratch_path TEXT NOT NULL,
  expected_volume_id TEXT,
  expected_file_id TEXT,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  finished_at TEXT,
  error TEXT
);

CREATE TABLE IF NOT EXISTS permanent_delete_batch (
  id INTEGER PRIMARY KEY,
  public_id TEXT,
  operation_id INTEGER NOT NULL REFERENCES operation(id),
  preview_id TEXT NOT NULL,
  preview_digest TEXT NOT NULL,
  selected_groups_json TEXT NOT NULL,
  requested_count INTEGER NOT NULL,
  eligible_count INTEGER NOT NULL DEFAULT 0,
  removed_count INTEGER NOT NULL DEFAULT 0,
  blocked_count INTEGER NOT NULL DEFAULT 0,
  failed_count INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  finished_at TEXT,
  error TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_permanent_delete_batch_public_id
  ON permanent_delete_batch(public_id) WHERE public_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS permanent_delete_batch_item (
  id INTEGER PRIMARY KEY,
  batch_id INTEGER NOT NULL REFERENCES permanent_delete_batch(id),
  operation_item_id INTEGER NOT NULL REFERENCES operation_item(id),
  quarantine_entry_id INTEGER NOT NULL REFERENCES quarantine_entry(id),
  archive_id INTEGER REFERENCES object_archive(id),
  removal_group_id TEXT NOT NULL,
  topology_group_id TEXT NOT NULL,
  held_path TEXT NOT NULL,
  expected_volume_id TEXT NOT NULL,
  expected_file_id TEXT NOT NULL,
  expected_bytes INTEGER NOT NULL,
  expected_modified_unix_seconds INTEGER,
  expected_content_blake3 TEXT NOT NULL,
  logical_bytes INTEGER NOT NULL,
  allocated_bytes INTEGER,
  elevation_capability_id INTEGER REFERENCES elevation_capability(id),
  capability_index INTEGER,
  archive_partial_path TEXT,
  archive_initial_volume_id TEXT,
  archive_initial_file_id TEXT,
  archive_initial_bytes INTEGER,
  archive_initial_modified_unix_seconds INTEGER,
  archive_final_path TEXT,
  archive_proof_volume_id TEXT,
  archive_proof_file_id TEXT,
  archive_proof_bytes INTEGER,
  archive_proof_modified_unix_seconds INTEGER,
  archive_proof_blake3 TEXT,
  archive_raw_backup_blake3 TEXT,
  archive_semantic_blake3 TEXT,
  archive_roundtrip_blake3 TEXT,
  archive_stream_count INTEGER,
  archive_security_stream_present INTEGER,
  archive_cleanup_complete INTEGER,
  archive_proof_schema TEXT,
  phase TEXT NOT NULL,
  status TEXT NOT NULL,
  reason_code TEXT,
  message TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(batch_id, quarantine_entry_id),
  UNIQUE(batch_id, operation_item_id)
);
CREATE INDEX IF NOT EXISTS idx_permanent_delete_batch_item_recovery
  ON permanent_delete_batch_item(batch_id, status, phase);

CREATE TABLE IF NOT EXISTS final_disposition_guardian (
  id INTEGER PRIMARY KEY,
  batch_item_id INTEGER NOT NULL UNIQUE
    REFERENCES permanent_delete_batch_item(id) ON DELETE CASCADE,
  operation_id INTEGER NOT NULL REFERENCES operation(id),
  nonce_digest TEXT NOT NULL UNIQUE,
  guardian_pid INTEGER NOT NULL CHECK(guardian_pid > 0),
  guardian_started_100ns INTEGER NOT NULL CHECK(guardian_started_100ns > 0),
  guardian_image_sha256 TEXT NOT NULL,
  expected_volume_id TEXT NOT NULL,
  expected_file_id TEXT NOT NULL,
  expected_bytes INTEGER NOT NULL CHECK(expected_bytes >= 0),
  expected_modified_unix_seconds INTEGER NOT NULL,
  receipt_path TEXT,
  receipt_volume_id TEXT,
  receipt_file_id TEXT,
  receipt_key_dpapi TEXT,
  receipt_cleanup_complete INTEGER NOT NULL DEFAULT 0
    CHECK(receipt_cleanup_complete IN (0, 1)),
  state TEXT NOT NULL CHECK(state IN (
    'handle_bound', 'arm_authorized_unproved', 'armed_unproved',
    'final_profile_proved_held', 'close_authorized',
    'guardian_handle_closed', 'parent_handle_closed',
    'cancelled_safe', 'cancellation_pending_retained'
  )),
  disposition_mode TEXT CHECK(
    disposition_mode IS NULL OR disposition_mode IN ('extendedOnClose', 'legacy')
  ),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_final_disposition_guardian_recovery
  ON final_disposition_guardian(state, operation_id);

CREATE TABLE IF NOT EXISTS permanent_delete_preview (
  preview_id TEXT PRIMARY KEY,
  preview_digest TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  entry_ids_json TEXT NOT NULL,
  topology_groups_json TEXT NOT NULL,
  target_count INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_permanent_delete_preview_expiry
  ON permanent_delete_preview(expires_at, consumed_at);

CREATE TABLE IF NOT EXISTS permanent_delete_preview_item (
  preview_id TEXT NOT NULL REFERENCES permanent_delete_preview(preview_id) ON DELETE CASCADE,
  quarantine_entry_id INTEGER NOT NULL REFERENCES quarantine_entry(id),
  removal_group_id TEXT NOT NULL,
  topology_group_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  eligibility TEXT NOT NULL,
  reason TEXT,
  PRIMARY KEY(preview_id, quarantine_entry_id),
  UNIQUE(preview_id, ordinal)
);
CREATE INDEX IF NOT EXISTS idx_permanent_delete_preview_item_group
  ON permanent_delete_preview_item(preview_id, topology_group_id);

CREATE TABLE IF NOT EXISTS mutation_space_effect (
  id INTEGER PRIMARY KEY,
  operation_id INTEGER NOT NULL REFERENCES operation(id),
  operation_item_id INTEGER REFERENCES operation_item(id),
  volume_id TEXT NOT NULL,
  lifecycle_stage TEXT NOT NULL,
  logical_bytes INTEGER,
  allocated_bytes INTEGER,
  free_space_delta_observed INTEGER,
  created_at TEXT NOT NULL
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
    // Existing mutation databases predate handle-bound recovery metadata. Add
    // the nullable columns idempotently; legacy rows stay intentionally
    // untrusted and recovery will refuse to mutate them by pathname alone.
    for (column, ty) in [
        ("expected_volume_id", "TEXT"),
        ("expected_file_id", "TEXT"),
        ("expected_blake3", "TEXT"),
        ("expected_modified_unix_seconds", "INTEGER"),
        ("result_volume_id", "TEXT"),
        ("result_file_id", "TEXT"),
        ("result_blake3", "TEXT"),
        ("result_modified_unix_seconds", "INTEGER"),
    ] {
        ensure_column(conn, "operation_item", column, ty)?;
    }
    for (column, ty) in [
        ("removal_group_id", "TEXT"),
        ("removal_group_fingerprint", "TEXT"),
    ] {
        ensure_column(conn, "quarantine_entry", column, ty)?;
    }
    for (column, ty) in [
        ("eligible_count", "INTEGER NOT NULL DEFAULT 0"),
        ("removed_count", "INTEGER NOT NULL DEFAULT 0"),
        ("blocked_count", "INTEGER NOT NULL DEFAULT 0"),
        ("failed_count", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        ensure_column(conn, "permanent_delete_batch", column, ty)?;
    }
    for (column, ty) in [
        (
            "quarantine_entry_id",
            "INTEGER REFERENCES quarantine_entry(id)",
        ),
        ("removal_group_id", "TEXT"),
        ("held_path", "TEXT"),
        ("held_volume_id", "TEXT"),
        ("held_file_id", "TEXT"),
        ("held_bytes", "INTEGER"),
        ("held_modified_unix_seconds", "INTEGER"),
        ("held_content_blake3", "TEXT"),
    ] {
        ensure_column(conn, "object_archive", column, ty)?;
    }
    ensure_column(conn, "elevation_capability", "consumed_at", "TEXT")?;
    ensure_column(conn, "elevation_capability", "transport_nonce", "TEXT")?;
    for (column, ty) in [
        ("expected_bytes", "INTEGER"),
        ("expected_modified_unix_seconds", "INTEGER"),
        ("receipt_path", "TEXT"),
        ("receipt_volume_id", "TEXT"),
        ("receipt_file_id", "TEXT"),
        ("receipt_key_dpapi", "TEXT"),
        ("receipt_cleanup_complete", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        ensure_column(conn, "final_disposition_guardian", column, ty)?;
    }
    for (column, ty) in [
        (
            "elevation_capability_id",
            "INTEGER REFERENCES elevation_capability(id)",
        ),
        ("capability_index", "INTEGER"),
        ("archive_partial_path", "TEXT"),
        ("archive_initial_volume_id", "TEXT"),
        ("archive_initial_file_id", "TEXT"),
        ("archive_initial_bytes", "INTEGER"),
        ("archive_initial_modified_unix_seconds", "INTEGER"),
        ("archive_final_path", "TEXT"),
        ("archive_proof_volume_id", "TEXT"),
        ("archive_proof_file_id", "TEXT"),
        ("archive_proof_bytes", "INTEGER"),
        ("archive_proof_modified_unix_seconds", "INTEGER"),
        ("archive_proof_blake3", "TEXT"),
        ("archive_raw_backup_blake3", "TEXT"),
        ("archive_semantic_blake3", "TEXT"),
        ("archive_roundtrip_blake3", "TEXT"),
        ("archive_stream_count", "INTEGER"),
        ("archive_security_stream_present", "INTEGER"),
        ("archive_cleanup_complete", "INTEGER"),
        ("archive_proof_schema", "TEXT"),
    ] {
        ensure_column(conn, "permanent_delete_batch_item", column, ty)?;
    }
    ensure_column(conn, "permanent_delete_batch", "public_id", "TEXT")?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_object_archive_entry
           ON object_archive(quarantine_entry_id) WHERE quarantine_entry_id IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_object_archive_path_nocase
           ON object_archive(archive_path COLLATE NOCASE);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_object_archive_legacy_source_nocase
           ON object_archive(backup_id, original_path COLLATE NOCASE);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_elevation_capability_nonce
           ON elevation_capability(nonce_digest);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_elevation_capability_request
           ON elevation_capability(request_digest);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_elevation_capability_transport_nonce
           ON elevation_capability(transport_nonce) WHERE transport_nonce IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_permanent_delete_batch_public_id
           ON permanent_delete_batch(public_id) WHERE public_id IS NOT NULL;",
    )?;
    Ok(())
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    ty: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"))?;
    }
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

    fn index_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
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

        let mut columns = conn.prepare("PRAGMA table_info(operation_item)").unwrap();
        let columns: Vec<String> = columns
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for column in [
            "expected_volume_id",
            "expected_file_id",
            "expected_blake3",
            "expected_modified_unix_seconds",
            "result_volume_id",
            "result_file_id",
            "result_blake3",
            "result_modified_unix_seconds",
        ] {
            assert!(
                columns.iter().any(|found| found == column),
                "missing {column}"
            );
        }
        for table in [
            "object_archive",
            "object_archive_event",
            "elevation_capability",
            "archive_cleanup",
            "permanent_delete_batch",
            "permanent_delete_batch_item",
            "final_disposition_guardian",
            "permanent_delete_preview",
            "permanent_delete_preview_item",
            "mutation_space_effect",
        ] {
            assert!(table_exists(&conn, table), "missing table {table}");
        }

        let mut batch_columns = conn
            .prepare("PRAGMA table_info(permanent_delete_batch)")
            .unwrap();
        let batch_columns: Vec<String> = batch_columns
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(batch_columns.iter().any(|column| column == "public_id"));
        let columns_for = |table: &str| -> Vec<String> {
            let mut statement = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let elevation_columns = columns_for("elevation_capability");
        assert!(elevation_columns
            .iter()
            .any(|column| column == "transport_nonce"));
        let item_columns = columns_for("permanent_delete_batch_item");
        for column in [
            "elevation_capability_id",
            "capability_index",
            "archive_partial_path",
            "archive_initial_volume_id",
            "archive_initial_file_id",
            "archive_initial_bytes",
            "archive_initial_modified_unix_seconds",
            "archive_final_path",
            "archive_proof_volume_id",
            "archive_proof_file_id",
            "archive_proof_bytes",
            "archive_proof_modified_unix_seconds",
            "archive_proof_blake3",
            "archive_raw_backup_blake3",
            "archive_semantic_blake3",
            "archive_roundtrip_blake3",
            "archive_stream_count",
            "archive_security_stream_present",
            "archive_cleanup_complete",
            "archive_proof_schema",
        ] {
            assert!(
                item_columns.iter().any(|found| found == column),
                "missing {column}"
            );
        }
        let guardian_columns = columns_for("final_disposition_guardian");
        for column in [
            "batch_item_id",
            "operation_id",
            "nonce_digest",
            "guardian_pid",
            "guardian_started_100ns",
            "guardian_image_sha256",
            "expected_volume_id",
            "expected_file_id",
            "expected_bytes",
            "expected_modified_unix_seconds",
            "receipt_path",
            "receipt_volume_id",
            "receipt_file_id",
            "receipt_key_dpapi",
            "receipt_cleanup_complete",
            "state",
            "disposition_mode",
            "error",
        ] {
            assert!(
                guardian_columns.iter().any(|found| found == column),
                "missing guardian column {column}"
            );
        }
        let guardian_schema: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'final_disposition_guardian'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(guardian_schema.contains("cancellation_pending_retained"));
        assert!(guardian_schema.contains("extendedOnClose"));
        for index in [
            "idx_object_archive_entry",
            "idx_object_archive_path_nocase",
            "idx_object_archive_legacy_source_nocase",
            "idx_elevation_capability_nonce",
            "idx_elevation_capability_request",
            "idx_elevation_capability_transport_nonce",
            "idx_permanent_delete_batch_public_id",
            "idx_final_disposition_guardian_recovery",
        ] {
            assert!(index_exists(&conn, index), "missing index {index}");
        }
    }

    #[test]
    fn upgrades_legacy_elevation_capability_with_recovery_nonce() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE elevation_capability (
               id INTEGER PRIMARY KEY,
               operation_id INTEGER NOT NULL,
               request_digest TEXT NOT NULL,
               nonce_digest TEXT NOT NULL,
               helper_image_sha256 TEXT,
               status TEXT NOT NULL,
               issued_at TEXT NOT NULL,
               finished_at TEXT,
               error TEXT
             );",
        )
        .unwrap();

        ensure_journal_schema(&conn).unwrap();
        ensure_journal_schema(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(elevation_capability)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "transport_nonce"));
        assert!(columns.iter().any(|column| column == "consumed_at"));
        assert!(index_exists(
            &conn,
            "idx_elevation_capability_transport_nonce"
        ));

        conn.execute(
            "INSERT INTO elevation_capability(
                operation_id, request_digest, transport_nonce, nonce_digest,
                status, issued_at
             ) VALUES(1, 'request-a', ?1, 'digest-a', 'issued', 'now')",
            ["ab".repeat(32)],
        )
        .unwrap();
        let duplicate = conn.execute(
            "INSERT INTO elevation_capability(
                operation_id, request_digest, transport_nonce, nonce_digest,
                status, issued_at
             ) VALUES(2, 'request-b', ?1, 'digest-b', 'issued', 'now')",
            ["ab".repeat(32)],
        );
        assert!(duplicate.is_err(), "transport nonces must remain unique");
    }
}
