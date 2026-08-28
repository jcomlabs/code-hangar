//! Permanent delete — **irreversible**.
//!
//! Gated by a `PermanentDelete` confirmation token so a programmatic caller
//! cannot skip the human confirmation handshake. Deletes the quarantined copy
//! and marks the entry `permanently_deleted`, recording the freed bytes.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use crate::bound_fs::{self, BoundFile, FileStamp};
use crate::confirm::{ConfirmAction, ConfirmTokenStore};

#[derive(Debug, Error)]
pub enum PurgeError {
    #[error("permanent delete requires a valid confirmation token")]
    ConfirmRequired,
    #[error("purge io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("purge journal error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("quarantine entry {0} not found")]
    NotFound(i64),
    #[error("quarantine entry {0} is not purgeable (status: {1})")]
    NotPurgeable(i64, String),
    #[error("permanent delete refused: quarantine entry {0} has no linked backup")]
    BackupRequired(i64),
    #[error("permanent delete refused: the backup for entry {0} is unusable: {1}")]
    BackupUnusable(i64, String),
    #[error("permanent delete refused: held copy is cloud-backed: {0}")]
    CloudSourceRefused(String),
    #[error("permanent delete refused: held copy is a reparse point: {0}")]
    SourceIsReparse(String),
}

#[derive(Debug, Clone, Copy)]
pub struct PurgeOutcome {
    pub freed_bytes: u64,
}

/// Permanently delete a quarantined entry. Consumes a single-use
/// `PermanentDelete` token; without a valid token nothing is touched.
pub fn permanent_delete_entry(
    conn: &Connection,
    tokens: &ConfirmTokenStore,
    token: &str,
    entry_id: i64,
) -> Result<PurgeOutcome, PurgeError> {
    if !tokens.consume(token, ConfirmAction::PermanentDelete) {
        return Err(PurgeError::ConfirmRequired);
    }

    permanent_delete_entry_after_confirmation(conn, entry_id)
}

/// The legacy single-entry executor kept for migration/recovery coverage.
///
/// Production callers cannot reach this through an action-only token:
/// `ConfirmTokenStore` deliberately refuses unscoped permanent-delete grants.
/// The shipping path is the immutable preview/group-bound batch executor in
/// `final_remove.rs`. Keeping the already-authorized body separate lets its
/// backup, identity and exact-handle invariants remain regression-tested
/// without reopening the retired unscoped authorization surface.
fn permanent_delete_entry_after_confirmation(
    conn: &Connection,
    entry_id: i64,
) -> Result<PurgeOutcome, PurgeError> {
    let row = conn
        .query_row(
            "SELECT quarantine_path, original_path, size, status, backup_id, operation_id FROM quarantine_entry WHERE id = ?1",
            [entry_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u64,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;
    let (quarantine_path, original_path, size, status, backup_id, quarantine_operation_id) =
        row.ok_or(PurgeError::NotFound(entry_id))?;
    if status != "quarantined" {
        return Err(PurgeError::NotPurgeable(entry_id, status));
    }
    bound_fs::validate_local_mutation_path(Path::new(&quarantine_path))?;
    bound_fs::validate_local_mutation_path(Path::new(&original_path))?;

    // Gate 3 — the central invariant: never permanently delete without a verified
    // backup that covers this exact file. The linked backup must still exist, be
    // verified, have a readable manifest, and contain this source path. Refusal
    // here is the backend enforcement (not UI discipline) of "backup before delete".
    let backup_id = backup_id
        .filter(|id| *id > 0)
        .ok_or(PurgeError::BackupRequired(entry_id))?;
    let backup = crate::load_verified_backup(conn, backup_id)
        .map_err(|err| PurgeError::BackupUnusable(entry_id, err.to_string()))?;
    // Content binding (not just a path-string match): the held copy's bytes must equal
    // what the verified backup recorded for this path. A path match alone is defeatable
    // by a stale/older/unrelated backup that merely lists the path — that must NOT
    // authorize deleting the current (different) content.
    let expected_hash = backup.hash_for(&original_path).ok_or_else(|| {
        PurgeError::BackupUnusable(
            entry_id,
            format!("backup {backup_id} does not cover {original_path}"),
        )
    })?;
    let expected_stamp = load_held_stamp(
        conn,
        quarantine_operation_id,
        &quarantine_path,
        expected_hash,
    )?;
    let mut held = BoundFile::open_for_move(Path::new(&quarantine_path))?;
    if held.verify_stamp(&expected_stamp).is_err() || held.verify_hash(expected_hash).is_err() {
        return Err(PurgeError::BackupUnusable(
            entry_id,
            "the held copy does not match the verified backup (content changed since backup)"
                .to_string(),
        ));
    }
    // ...and the backup must actually be able to restore it: the recorded payload file
    // must still exist on disk and still hash to its recorded value. A manifest can
    // outlive its payload (volume gone, file truncated, antivirus quarantine); deleting
    // the last live copy while the backup cannot restore is irreversible loss.
    let _backup_payload_guard = backup
        .bind_payload(&original_path)
        .map_err(|err| PurgeError::BackupUnusable(entry_id, err.to_string()))?;

    let started_at = chrono::Utc::now().to_rfc3339();
    // One durable pre-disposition intent: operation + identity-bound item +
    // quarantine-entry CAS commit atomically. Recovery can therefore never see
    // a `deleting` entry without its exact object proof, nor an executable purge
    // operation that has not yet claimed the entry.
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO operation(kind, status, plan_json, backup_id, created_at, started_at)
         VALUES('permanent_delete', 'executing', ?1, ?2, ?3, ?3)",
        params![
            format!("{{\"permanent_delete_entry\":{entry_id}}}"),
            backup_id,
            started_at
        ],
    )?;
    let operation_id = transaction.last_insert_rowid();
    transaction.execute(
        "INSERT INTO operation_item(
            operation_id, action, from_path, bytes, checksum_before,
            expected_volume_id, expected_file_id, expected_blake3,
            expected_modified_unix_seconds, status
         ) VALUES(?1, 'delete_bound', ?2, ?3, ?4, ?5, ?6, ?4, ?7, 'pending')",
        params![
            operation_id,
            quarantine_path,
            held.stamp().bytes as i64,
            expected_hash,
            held.stamp().volume_id,
            held.stamp().file_id,
            held.stamp().modified_unix_seconds,
        ],
    )?;
    let operation_item_id = transaction.last_insert_rowid();

    // Crash-safe ordering: flip the entry to 'deleting' in the journal BEFORE the
    // unlink, so a crash between the unlink and the final update is reconcilable on
    // recovery (the old order left a deleted file still marked 'quarantined').
    // The flip is a compare-and-set claim, not a blind update: a concurrent restore
    // (separate connection) may have flipped the entry to 'restored' after the
    // status read above — blindly proceeding would record 'permanently_deleted'
    // for an entry whose file was actually restored. Zero rows affected means the
    // entry changed underneath us; abort without touching anything.
    let claimed = transaction.execute(
        "UPDATE quarantine_entry SET status = 'deleting' WHERE id = ?1 AND status = 'quarantined'",
        [entry_id],
    )?;
    if claimed == 0 {
        let now_status: String = transaction
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "missing".to_string());
        transaction.rollback()?;
        return Err(PurgeError::NotPurgeable(entry_id, now_status));
    }
    transaction.commit()?;

    // Disposition is applied to the already verified handle. There is no second
    // name lookup and therefore no opportunity to delete a raced replacement.
    let removal = held.delete_exact(expected_hash);

    match removal {
        Ok(()) => {
            conn.execute(
                "UPDATE operation_item SET status = 'done' WHERE id = ?1",
                [operation_item_id],
            )?;
            conn.execute(
                "UPDATE quarantine_entry SET status = 'permanently_deleted', space_recovered = ?2 WHERE id = ?1",
                params![entry_id, size as i64],
            )?;
            conn.execute(
                "UPDATE operation SET status = 'done', recovered_bytes = ?2, finished_at = ?3 WHERE id = ?1",
                params![operation_id, size as i64, chrono::Utc::now().to_rfc3339()],
            )?;
            Ok(PurgeOutcome { freed_bytes: size })
        }
        Err(error) => {
            let message = error.to_string();
            // The unlink did not complete. Put the held copy back into the normal
            // recoverable state so the user can retry final removal or restore it.
            conn.execute(
                "UPDATE operation_item SET status = 'failed' WHERE id = ?1",
                [operation_item_id],
            )?;
            conn.execute(
                "UPDATE quarantine_entry SET status = 'quarantined' WHERE id = ?1",
                [entry_id],
            )?;
            conn.execute(
                "UPDATE operation SET status = 'failed', finished_at = ?2, error = ?3 WHERE id = ?1",
                params![operation_id, chrono::Utc::now().to_rfc3339(), message],
            )?;
            Err(PurgeError::Io(error))
        }
    }
}

fn load_held_stamp(
    conn: &Connection,
    operation_id: Option<i64>,
    quarantine_path: &str,
    expected_hash: &str,
) -> Result<FileStamp, PurgeError> {
    let operation_id = operation_id.ok_or_else(|| {
        PurgeError::BackupUnusable(0, "held copy has no originating operation".to_string())
    })?;
    let row = conn
        .query_row(
            "SELECT result_volume_id, result_file_id, bytes,
                    COALESCE(result_blake3, checksum_after),
                    result_modified_unix_seconds
             FROM operation_item
             WHERE operation_id = ?1 AND to_path = ?2 AND status = 'done'
             ORDER BY id DESC LIMIT 1",
            params![operation_id, quarantine_path],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;
    let (volume_id, file_id, bytes, hash, modified_unix_seconds) = row.ok_or_else(|| {
        PurgeError::BackupUnusable(0, "held copy has no identity-bound journal row".to_string())
    })?;
    let hash = hash.ok_or_else(|| {
        PurgeError::BackupUnusable(0, "held copy has no journaled hash".to_string())
    })?;
    if hash != expected_hash {
        return Err(PurgeError::BackupUnusable(
            0,
            "held-copy journal hash differs from verified backup".to_string(),
        ));
    }
    Ok(FileStamp {
        volume_id: volume_id.ok_or_else(|| {
            PurgeError::BackupUnusable(0, "held copy has no volume identity".to_string())
        })?,
        file_id: file_id.ok_or_else(|| {
            PurgeError::BackupUnusable(0, "held copy has no file identity".to_string())
        })?,
        bytes: bytes.unwrap_or(0).max(0) as u64,
        modified_unix_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        create_backup, load_verified_backup, quarantine, BackupItem, BackupLevel, BackupRequest,
        ConfirmTokenStore, QuarantineItem, QuarantineRequest,
    };
    use std::fs;
    use std::path::PathBuf;

    fn journaled_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        conn
    }

    fn reviewed_quarantine_item(source: &Path, relative: &str) -> QuarantineItem {
        let (expected_source_stamp, backup_hash) =
            crate::inspect_local_mutation_file(source).unwrap();
        QuarantineItem {
            source: source.to_path_buf(),
            relative: relative.to_string(),
            expected_source_stamp,
            backup_hash,
        }
    }

    fn quarantine_one(conn: &Connection, dir: &std::path::Path) -> (i64, PathBuf) {
        let source = dir.join("project/cache/blob.bin");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"cache blob payload").unwrap();
        let (expected_source_stamp, expected_source_hash) =
            crate::inspect_local_mutation_file(&source).unwrap();

        // Gate 3: a held copy is only purgeable if a verified backup covers it.
        // Make that backup first (before the move reads/relocates the source).
        let backup = create_backup(
            conn,
            BackupRequest {
                level: BackupLevel::Standard,
                // The project root is `project/`; the backup goes to a sibling so it
                // is not inside the source tree (the engine refuses that).
                source_root: &dir.join("project"),
                destination_root: &dir.join("backup"),
                items: vec![BackupItem {
                    source: source.clone(),
                    relative: "cache/blob.bin".to_string(),
                    expected_source_stamp,
                    expected_source_hash,
                }],
                plan_json: "{}".to_string(),
                allow_same_volume: true,
            },
        )
        .unwrap();
        let verified = load_verified_backup(conn, backup.backup_id).unwrap();
        let copy = verified.copy_for(&source.to_string_lossy()).unwrap();
        let backup_hash = copy.blake3.clone();
        let expected_source_stamp = copy.source_stamp().unwrap().clone();

        quarantine(
            conn,
            QuarantineRequest {
                quarantine_root: &dir.join("quarantine"),
                items: vec![QuarantineItem {
                    source: source.clone(),
                    relative: "cache/blob.bin".to_string(),
                    expected_source_stamp,
                    backup_hash,
                }],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: backup.backup_id,
                cleanup_root: None,
                include_protected: false,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();
        let (entry_id, quarantine_path): (i64, String) = conn
            .query_row(
                "SELECT id, quarantine_path FROM quarantine_entry LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        (entry_id, PathBuf::from(quarantine_path))
    }

    #[test]
    fn permanent_delete_requires_a_valid_token() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let tokens = ConfirmTokenStore::default();
        let (entry_id, quarantined) = quarantine_one(&conn, dir.path());

        // No / wrong token: refused, file intact.
        assert!(matches!(
            permanent_delete_entry(&conn, &tokens, "bogus", entry_id),
            Err(PurgeError::ConfirmRequired)
        ));
        assert!(quarantined.exists());
        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
    }

    #[test]
    fn already_authorized_legacy_body_removes_and_records() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, quarantined) = quarantine_one(&conn, dir.path());

        let outcome = permanent_delete_entry_after_confirmation(&conn, entry_id).unwrap();
        assert!(outcome.freed_bytes > 0);
        assert!(!quarantined.exists());

        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "permanently_deleted");

        // The compatibility entry point cannot mint/consume an unscoped
        // permanent-delete grant; only the preview-bound batch path can do so.
        let tokens = ConfirmTokenStore::default();
        assert!(tokens.issue(ConfirmAction::PermanentDelete).is_err());
        assert!(matches!(
            permanent_delete_entry(&conn, &tokens, "retired", entry_id),
            Err(PurgeError::ConfirmRequired)
        ));
    }

    #[test]
    fn permanent_delete_refused_without_a_verified_backup() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();

        // Quarantine WITHOUT a linked backup (backup_id 0 is stored as NULL).
        let source = dir.path().join("project/cache/blob.bin");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"unprotected payload").unwrap();
        quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &dir.path().join("quarantine"),
                items: vec![reviewed_quarantine_item(&source, "cache/blob.bin")],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: None,
                include_protected: false,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();
        let (entry_id, quarantine_path): (i64, String) = conn
            .query_row(
                "SELECT id, quarantine_path FROM quarantine_entry LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        // Gate 3 remains enforced inside the already-authorized legacy body:
        // with no verified backup linked, the held copy is untouched.
        assert!(matches!(
            permanent_delete_entry_after_confirmation(&conn, entry_id),
            Err(PurgeError::BackupRequired(_))
        ));
        assert!(PathBuf::from(&quarantine_path).exists());
        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
    }

    #[test]
    fn permanent_delete_refused_when_held_copy_does_not_match_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, quarantined) = quarantine_one(&conn, dir.path());

        // The held copy no longer matches what the verified backup recorded for this
        // path (a stale/unrelated/tampered backup, or content that changed). A
        // path-string coverage check alone would still authorize the delete — the
        // content binding must refuse it, leaving the held copy intact.
        fs::write(&quarantined, b"different bytes than the backup recorded").unwrap();

        assert!(matches!(
            permanent_delete_entry_after_confirmation(&conn, entry_id),
            Err(PurgeError::BackupUnusable(_, _))
        ));
        assert!(quarantined.exists());
        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
    }

    #[test]
    fn permanent_delete_refused_when_backup_payload_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, quarantined) = quarantine_one(&conn, dir.path());

        // The manifest still records the verified backup and the held copy still
        // matches its recorded hash — but the backup PAYLOAD is gone (volume gone,
        // truncation, antivirus). A manifest-only check would authorize the delete;
        // since the backup can no longer restore, the irreversible delete must refuse.
        fs::remove_file(dir.path().join("backup/cache/blob.bin")).unwrap();

        assert!(matches!(
            permanent_delete_entry_after_confirmation(&conn, entry_id),
            Err(PurgeError::BackupUnusable(_, _))
        ));
        assert!(quarantined.exists());
        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
    }

    #[cfg(windows)]
    #[test]
    fn permanent_delete_locked_file_returns_entry_to_quarantined() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, quarantined) = quarantine_one(&conn, dir.path());

        let _lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&quarantined)
            .unwrap();

        assert!(matches!(
            permanent_delete_entry_after_confirmation(&conn, entry_id),
            Err(PurgeError::Io(_))
        ));
        assert!(quarantined.exists());
        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
    }
}
