//! Restore — the journaled inverse of quarantine.
//!
//! Never overwrites: if the original path is occupied it reports a `Conflict`
//! (the UI resolves it) instead of clobbering. Otherwise it moves the
//! quarantined copy back, verifying it byte-identical for cross-volume moves,
//! and marks the entry `restored`.

use std::path::{Component, Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use crate::fsops::{self, MoveStrategy};
use crate::longpath::to_extended;

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("restore io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("restore journal error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("checksum mismatch restoring {path}")]
    ChecksumMismatch { path: String },
    #[error("quarantine entry {0} not found")]
    NotFound(i64),
    #[error("quarantine entry {0} is not restorable (status: {1})")]
    NotRestorable(i64, String),
    #[error("restore destination is protected or sensitive: {0}")]
    ProtectedDestination(String),
    #[error("restore destination became occupied: {path}")]
    DestinationOccupied { path: String },
}

impl From<fsops::FsMoveError> for RestoreError {
    fn from(error: fsops::FsMoveError) -> Self {
        match error {
            fsops::FsMoveError::Io(io) => RestoreError::Io(io),
            fsops::FsMoveError::ChecksumMismatch { path } => {
                RestoreError::ChecksumMismatch { path }
            }
            // A concurrent writer raced a file into the destination after the pre-move
            // exists() check but before the move. Surfaced as an error (nothing overwritten)
            // rather than silently clobbering the third-party file.
            fsops::FsMoveError::DestinationOccupied { path } => {
                RestoreError::DestinationOccupied { path }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    Restored {
        original_path: String,
        restored_path: String,
    },
    /// The original path is occupied; nothing was overwritten.
    Conflict {
        original_path: String,
        conflict_path: String,
    },
}

/// Restore a quarantined entry to its original path.
pub fn restore_entry(conn: &Connection, entry_id: i64) -> Result<RestoreOutcome, RestoreError> {
    restore_entry_inner(conn, entry_id, None)
}

/// Restore a quarantined entry under a user-selected destination folder,
/// preserving the stored relative layout from the journal manifest. The target
/// path is never overwritten.
pub fn restore_entry_to_folder(
    conn: &Connection,
    entry_id: i64,
    destination_root: &Path,
) -> Result<RestoreOutcome, RestoreError> {
    restore_entry_inner(conn, entry_id, Some(destination_root))
}

fn restore_entry_inner(
    conn: &Connection,
    entry_id: i64,
    destination_root: Option<&Path>,
) -> Result<RestoreOutcome, RestoreError> {
    let row = conn
        .query_row(
            "SELECT original_path, quarantine_path, status, manifest_json FROM quarantine_entry WHERE id = ?1",
            [entry_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let (original_path, quarantine_path, status, manifest_json) =
        row.ok_or(RestoreError::NotFound(entry_id))?;
    if status != "quarantined" {
        return Err(RestoreError::NotRestorable(entry_id, status));
    }

    let original = PathBuf::from(&original_path);
    let destination = match destination_root {
        Some(root) => destination_for_folder(root, &manifest_json, &original),
        None => original.clone(),
    };
    let destination_path = destination.to_string_lossy().to_string();
    // Restoring to the recorded original path is the journaled inverse of our own
    // quarantine move: the entry legitimately came from there under the user's
    // explicit confirmation (including the include_protected opt-in), so putting
    // the same verified bytes back is always allowed — otherwise a quarantined
    // secret (.env, credentials.json, .git/*) could never be restored while
    // final-remove stayed available, making deletion the only offered action.
    // The protected/sensitive refusal below only guards NEW destinations chosen
    // at restore time; the no-overwrite Conflict rule applies to both.
    if let Some(root) = destination_root {
        let root_str = root.to_string_lossy();
        // The chosen root gets the full protected/sensitive screening; the joined
        // destination gets LOCATION rules only (.git/.ssh components) — the entry's
        // own file name (e.g. credentials.json) is exactly what is being restored
        // and must not block it (protected_level_for_path folds the name heuristic
        // in, so it cannot be used on the joined path).
        if hangar_protect::is_sensitive_path(&root_str)
            || hangar_protect::protected_level_for_path(&root_str).is_some()
            || hangar_protect::is_strong_protected_path(&root_str)
            || has_protected_location_component(&destination_path)
        {
            return Err(RestoreError::ProtectedDestination(destination_path));
        }
    }
    // Extended-length form so an occupied long-path destination is detected (a bare
    // exists() on a >260 path can wrongly report "free" without LongPathsEnabled, which
    // would drop the no-overwrite Conflict guard). The move itself goes through
    // `fsops::move_path`, which is long-path-safe and re-checks the destination.
    if to_extended(&destination).exists() {
        // Never overwrite — surface the conflict for the user to resolve.
        return Ok(RestoreOutcome::Conflict {
            original_path,
            conflict_path: destination_path,
        });
    }

    let started_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO operation(kind, status, plan_json, created_at, started_at)
         VALUES('restore', 'executing', ?1, ?2, ?2)",
        params![
            serde_json::json!({
                "restore_entry": entry_id,
                "destination": destination_path,
            })
            .to_string(),
            started_at
        ],
    )?;
    let operation_id = conn.last_insert_rowid();

    let from = Path::new(&quarantine_path);
    let checksum_before = fsops::hash_file(from)?;
    let strategy = fsops::choose_strategy(from, &destination);
    let action = match strategy {
        MoveStrategy::Rename => "move",
        MoveStrategy::CopyDelete => "copy_delete",
    };
    conn.execute(
        "INSERT INTO operation_item(operation_id, action, from_path, to_path, checksum_before, status)
         VALUES(?1, ?2, ?3, ?4, ?5, 'pending')",
        params![
            operation_id,
            action,
            quarantine_path,
            destination_path,
            checksum_before
        ],
    )?;
    let item_id = conn.last_insert_rowid();

    match fsops::move_path(from, &destination, strategy) {
        Ok(bytes) => {
            // The held copy has physically moved. From this point on, any journal error
            // must remain an interrupted state so startup recovery can reconcile the
            // destination instead of treating the entry as a terminal failed move.
            conn.execute(
                "UPDATE operation SET status = 'verifying' WHERE id = ?1",
                [operation_id],
            )?;
            let checksum_after = match fsops::hash_file(&destination) {
                Ok(hash) => hash,
                Err(error) => {
                    let message = error.to_string();
                    if to_extended(&destination).exists() {
                        mark_restore_validation_failure_complete(
                            conn,
                            operation_id,
                            item_id,
                            entry_id,
                            bytes,
                            None,
                            &message,
                        )?;
                    } else {
                        // Neither the held path nor the destination is currently visible.
                        // Keep `verifying` so the Recovery area blocks subsequent mutations
                        // rather than falsely retiring a potentially missing file.
                        conn.execute(
                            "UPDATE operation SET error = ?2 WHERE id = ?1",
                            params![operation_id, message],
                        )?;
                    }
                    return Err(error.into());
                }
            };
            if checksum_before != checksum_after {
                let message = format!("checksum mismatch restoring {}", original_path);
                mark_restore_validation_failure_complete(
                    conn,
                    operation_id,
                    item_id,
                    entry_id,
                    bytes,
                    Some(&checksum_after),
                    &message,
                )?;
                return Err(RestoreError::ChecksumMismatch {
                    path: destination_path,
                });
            }
            conn.execute(
                "UPDATE operation_item SET status = 'done', bytes = ?2, checksum_after = ?3 WHERE id = ?1",
                params![item_id, bytes as i64, checksum_after],
            )?;
            conn.execute(
                "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
                [entry_id],
            )?;
            conn.execute(
                "UPDATE operation SET status = 'done', finished_at = ?2 WHERE id = ?1",
                params![operation_id, chrono::Utc::now().to_rfc3339()],
            )?;
            Ok(RestoreOutcome::Restored {
                original_path,
                restored_path: destination_path,
            })
        }
        Err(error) => {
            let message = error.to_string();
            conn.execute(
                "UPDATE operation_item SET status = 'failed' WHERE id = ?1",
                [item_id],
            )?;
            conn.execute(
                "UPDATE operation SET status = 'failed', finished_at = ?2, error = ?3 WHERE id = ?1",
                params![operation_id, chrono::Utc::now().to_rfc3339(), message],
            )?;
            Err(error.into())
        }
    }
}

fn destination_for_folder(root: &Path, manifest_json: &str, original: &Path) -> PathBuf {
    let relative = serde_json::from_str::<serde_json::Value>(manifest_json)
        .ok()
        .and_then(|value| {
            value
                .get("relative")
                .and_then(|relative| relative.as_str())
                .map(safe_relative_path)
        })
        .filter(|relative| !relative.as_os_str().is_empty())
        .or_else(|| original.file_name().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("restored-entry"));
    root.join(relative)
}

fn safe_relative_path(relative: &str) -> PathBuf {
    let normalized = relative.replace('\\', "/");
    let mut out = PathBuf::new();
    for component in Path::new(&normalized).components() {
        if let Component::Normal(part) = component {
            out.push(part);
        }
    }
    out
}

/// Location-based screen for restore-to-folder destinations: any `.git` / `.ssh`
/// path component refuses the write. Deliberately ignores name-based secret
/// heuristics — those describe the file being restored, not where it lands.
fn has_protected_location_component(path: &str) -> bool {
    path.replace('\\', "/")
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(".git") || part.eq_ignore_ascii_case(".ssh"))
}

/// A post-move validation warning is terminal only after the journal reflects the
/// physical truth: the held copy is gone and the destination now owns the entry. The
/// caller still receives an error and the activity log records `failed`, but Recover
/// must not offer a held copy that no longer exists.
fn mark_restore_validation_failure_complete(
    conn: &Connection,
    operation_id: i64,
    item_id: i64,
    entry_id: i64,
    bytes: u64,
    checksum_after: Option<&str>,
    message: &str,
) -> Result<(), RestoreError> {
    conn.execute(
        "UPDATE operation_item
         SET status = 'done', bytes = ?2, checksum_after = ?3
         WHERE id = ?1",
        params![item_id, bytes as i64, checksum_after],
    )?;
    conn.execute(
        "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
        [entry_id],
    )?;
    conn.execute(
        "UPDATE operation SET status = 'failed', finished_at = ?2, error = ?3 WHERE id = ?1",
        params![operation_id, chrono::Utc::now().to_rfc3339(), message],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{quarantine, QuarantineItem, QuarantineRequest};
    use std::fs;

    fn journaled_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        conn
    }

    fn quarantine_one(conn: &Connection, dir: &Path) -> (i64, PathBuf) {
        quarantine_named(conn, dir, "output/render.bin", false)
    }

    fn quarantine_named(
        conn: &Connection,
        dir: &Path,
        relative: &str,
        include_protected: bool,
    ) -> (i64, PathBuf) {
        let source = dir.join("project").join(relative);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"render payload").unwrap();
        quarantine(
            conn,
            QuarantineRequest {
                quarantine_root: &dir.join("quarantine"),
                items: vec![QuarantineItem {
                    source: source.clone(),
                    relative: relative.to_string(),
                    backup_hash: None,
                }],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: None,
                include_protected,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();
        let entry_id: i64 = conn
            .query_row(
                "SELECT id FROM quarantine_entry ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (entry_id, source)
    }

    #[test]
    fn post_move_validation_failure_records_the_physical_restore() {
        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('restore', 'verifying', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let operation_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', 'held.bin', 'restored.bin', 'pending')",
            [operation_id],
        )
        .unwrap();
        let item_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO quarantine_entry(operation_id, original_path, quarantine_path, status, manifest_json)
             VALUES(?1, 'restored.bin', 'held.bin', 'quarantined', '{}')",
            [operation_id],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();

        mark_restore_validation_failure_complete(
            &conn,
            operation_id,
            item_id,
            entry_id,
            17,
            Some("changed-after-move"),
            "post-move validation failed",
        )
        .unwrap();

        let operation: (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let item: (String, i64, Option<String>) = conn
            .query_row(
                "SELECT status, bytes, checksum_after FROM operation_item WHERE id = ?1",
                [item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(operation.0, "failed");
        assert_eq!(operation.1.as_deref(), Some("post-move validation failed"));
        assert_eq!(
            item,
            (
                "done".to_string(),
                17,
                Some("changed-after-move".to_string())
            )
        );
        assert_eq!(entry_status, "restored");
    }

    #[test]
    fn restore_moves_back_and_marks_restored() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, original) = quarantine_one(&conn, dir.path());
        assert!(!original.exists()); // quarantined away

        let outcome = restore_entry(&conn, entry_id).unwrap();
        assert_eq!(
            outcome,
            RestoreOutcome::Restored {
                original_path: original.to_string_lossy().to_string(),
                restored_path: original.to_string_lossy().to_string()
            }
        );
        assert!(original.exists());
        assert_eq!(fs::read(&original).unwrap(), b"render payload");

        let status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "restored");
    }

    #[test]
    fn restore_records_verified_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, _original) = quarantine_one(&conn, dir.path());

        restore_entry(&conn, entry_id).unwrap();

        let (status, checksum_before, checksum_after, bytes): (
            String,
            Option<String>,
            Option<String>,
            i64,
        ) = conn
            .query_row(
                "SELECT status, checksum_before, checksum_after, COALESCE(bytes, 0)
                 FROM operation_item
                 WHERE action IN ('move', 'copy_delete')
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(status, "done");
        assert_eq!(checksum_before, checksum_after);
        assert!(checksum_before.unwrap().len() >= 64);
        assert!(bytes > 0);
    }

    #[test]
    fn restore_never_overwrites_an_occupied_original() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, original) = quarantine_one(&conn, dir.path());

        // Something now occupies the original path.
        fs::write(&original, b"newer content").unwrap();

        let outcome = restore_entry(&conn, entry_id).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Conflict { .. }));
        // Original untouched; entry still quarantined.
        assert_eq!(fs::read(&original).unwrap(), b"newer content");
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
    fn restore_returns_protected_entry_to_its_original_path() {
        // A secret quarantined under the include_protected opt-in must be
        // restorable to the exact place it came from — otherwise the only
        // action offered on it would be final removal.
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, original) = quarantine_named(&conn, dir.path(), "credentials.json", true);
        assert!(!original.exists());

        let outcome = restore_entry(&conn, entry_id).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        assert_eq!(fs::read(&original).unwrap(), b"render payload");
    }

    #[test]
    fn restore_to_folder_allows_sensitive_file_name_in_neutral_root() {
        // The entry's own name (credentials.json) is exactly what is being
        // restored; only the chosen root is screened for sensitivity.
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, _original) = quarantine_named(&conn, dir.path(), "credentials.json", true);
        let restore_root = dir.path().join("elsewhere");

        let outcome = restore_entry_to_folder(&conn, entry_id, &restore_root).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        assert!(restore_root.join("credentials.json").exists());
    }

    #[test]
    fn restore_to_folder_still_refuses_protected_root() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, _original) = quarantine_one(&conn, dir.path());
        let protected_root = dir.path().join(".ssh");

        let error = restore_entry_to_folder(&conn, entry_id, &protected_root).unwrap_err();
        assert!(matches!(error, RestoreError::ProtectedDestination(_)));
        // Entry untouched — still restorable elsewhere.
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
    fn restore_to_folder_preserves_safe_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let (entry_id, original) = quarantine_one(&conn, dir.path());
        let restore_root = dir.path().join("elsewhere");

        let outcome = restore_entry_to_folder(&conn, entry_id, &restore_root).unwrap();
        let restored = restore_root.join("output").join("render.bin");

        assert_eq!(
            outcome,
            RestoreOutcome::Restored {
                original_path: original.to_string_lossy().to_string(),
                restored_path: restored.to_string_lossy().to_string()
            }
        );
        assert!(!original.exists());
        assert_eq!(fs::read(restored).unwrap(), b"render payload");
    }
}
