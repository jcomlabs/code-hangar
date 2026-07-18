//! Crash recovery — the payoff of journal-first execution.
//!
//! On launch, any operation left mid-flight (`executing` / `backup_running` /
//! `verifying`) is rolled back: every completed move recorded in the journal is
//! reversed, so no half-applied operation persists after a crash or kill. Items
//! are marked `rolled_back`, the affected quarantine entries are marked
//! `restored` (their content is back at the original path), and the operation
//! becomes `rolled_back`. If both the original and held paths exist, recovery
//! never guesses which copy should win: it preserves both and ensures the held
//! copy is visible in Recover. Resuming pending items is a possible future option;
//! conservative rollback is the safe default.
//!
//! A permanent delete interrupted mid-unlink is reconciled separately: the entry
//! was flipped to `deleting` before the unlink, so the on-disk presence of the
//! held copy decides whether it returns to `quarantined` (delete did not happen)
//! or settles as `permanently_deleted` (the unlink completed).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use crate::fsops;
use crate::longpath::to_extended;

/// Sentinel recorded on a restore operation when reconciliation can prove neither the held
/// copy nor the restore destination. Its presence marks the "already surfaced once" state, so
/// a later, user-driven recovery_resolve can give the otherwise-permanently-'verifying' op a
/// terminal exit instead of re-wedging it forever.
const RESTORE_AMBIGUOUS_ERROR: &str =
    "Recovery could not find either the held copy or the restore destination.";

/// Terminal message when a persistently-ambiguous restore is abandoned on the second,
/// deliberate recovery_resolve so it stops blocking every new mutation. Still fail-closed: no
/// entry is ever marked 'restored', so a vanished held copy remains visible as a loss.
const RESTORE_AMBIGUOUS_ABANDONED: &str =
    "Recovery could not find either the held copy or the restore destination; the interrupted restore was abandoned so it no longer blocks new operations.";

const RESTORE_CONTENT_MISMATCH_ERROR: &str =
    "Recovery found the restore destination, but its content does not match the verified bytes recorded before the restore.";

const RESTORE_CONTENT_MISMATCH_RESOLVED: &str =
    "Recovery found the restore destination with different content; the destination was preserved for manual review and the interrupted restore was closed without claiming success.";

/// On-disk existence check that is accurate for a >260-char path (a bare `Path::exists`
/// can wrongly report "absent" without LongPathsEnabled). Recovery reconciles purely by
/// on-disk truth, so a wrong answer here could mis-mark an entry — always normalize.
fn path_present(path: &Path) -> bool {
    to_extended(path).exists()
}

/// Prove a path is ABSENT, failing CLOSED on any ambiguity. `path_present`'s bare
/// `exists()` reports `false` on ANY IO/metadata error — a transiently unreadable
/// removable/network/UNC volume, or an AV-/lock-held path — so it cannot tell "gone" from
/// "unreachable". This returns `true` ONLY for a definitive `NotFound`; a present file, a
/// permission error, an unreachable volume, or any other error all count as "not provably
/// absent". Use it wherever a terminal, data-hiding claim (marking a held copy 'restored'
/// or 'permanently_deleted') hinges on the held copy being gone: never declare a
/// still-present-but-unreadable copy "gone" and hide the user's only good data.
fn path_provably_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(to_extended(path).as_ref()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound
    )
}

/// A cross-volume copy can be durable before the source unlink and before the
/// quarantine-entry insert. If the app stops in that window, both paths exist. Never
/// delete either copy during recovery; make the held one visible so it cannot become an
/// untracked orphan. A linked backup is retained only when its verified row still exists.
fn expose_ambiguous_held_copy(
    conn: &Connection,
    operation_id: i64,
    from_path: &str,
    to_path: &str,
    backup_id: Option<i64>,
) -> Result<(), RecoveryError> {
    let held = Path::new(to_path);
    let bytes = std::fs::metadata(to_extended(held).as_ref())
        .map(|metadata| metadata.len().min(i64::MAX as u64) as i64)
        .unwrap_or(0);
    let relative = Path::new(from_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "recovered-entry".to_string());
    let existing_entry = conn
        .query_row(
            "SELECT id, manifest_json FROM quarantine_entry
             WHERE operation_id = ?1 AND quarantine_path = ?2
             ORDER BY id LIMIT 1",
            params![operation_id, to_path],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((entry_id, manifest_json)) = existing_entry {
        // A crash after the optimistic entry insert but before source unlink leaves both
        // copies on disk. Preserve the existing backup/hash metadata while correcting the
        // entry to physical truth: the held copy is visible and no bytes were recovered.
        let mut manifest = serde_json::from_str::<serde_json::Value>(&manifest_json)
            .unwrap_or(serde_json::Value::Null);
        if !manifest.is_object() {
            manifest = serde_json::json!({
                "original_path": from_path,
                "quarantine_path": to_path,
                "relative": relative.clone(),
                "bytes": bytes,
                "backup_blake3": serde_json::Value::Null,
            });
        }
        manifest["space_recovered"] = serde_json::json!(0);
        manifest["recovery_reason"] =
            serde_json::json!("both original and held copies existed after interruption");
        conn.execute(
            "UPDATE quarantine_entry
             SET status = 'quarantined', space_recovered = 0, manifest_json = ?2
             WHERE id = ?1",
            params![entry_id, manifest.to_string()],
        )?;
        return Ok(());
    }

    let backup_id = match backup_id.filter(|id| *id > 0) {
        Some(id) => conn
            .query_row(
                "SELECT id FROM backup WHERE id = ?1 AND verified = 1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?,
        None => None,
    };
    let manifest = serde_json::json!({
        "original_path": from_path,
        "quarantine_path": to_path,
        "relative": relative,
        "bytes": bytes,
        "space_recovered": 0,
        "backup_blake3": serde_json::Value::Null,
        "recovery_reason": "both original and held copies existed after interruption"
    })
    .to_string();
    conn.execute(
        "INSERT INTO quarantine_entry(
             operation_id, original_path, quarantine_path, size, space_recovered,
             backup_id, status, manifest_json
         ) VALUES(?1, ?2, ?3, ?4, 0, ?5, 'quarantined', ?6)",
        params![operation_id, from_path, to_path, bytes, backup_id, manifest],
    )?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("recovery journal error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub recovered_operations: usize,
    pub rolled_back_items: usize,
}

/// Roll back every operation left interrupted in the journal. Safe to call on
/// every launch; a clean journal (no in-flight operations) is a no-op.
pub fn recover_interrupted(conn: &Connection) -> Result<RecoveryReport, RecoveryError> {
    let operations: Vec<(i64, String, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, kind, backup_id FROM operation
             WHERE status IN ('executing', 'backup_running', 'verifying')
               AND kind NOT IN ('permanent_delete', 'restore')
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };

    let mut report = RecoveryReport::default();
    for (op_id, kind, backup_id) in operations {
        let items: Vec<(i64, String, String)> = {
            // 'pending' items are included on purpose: quarantine is journal-first
            // (row inserted 'pending' → fs::rename → UPDATE 'done'), so a crash in
            // the window between the rename syscall and the 'done' update leaves an
            // APPLIED move recorded as 'pending'. Skipping those would strand the
            // file in the holding area with no rollback and no Recover entry. The
            // on-disk gates below make reversing a genuinely-unapplied pending item
            // a no-op (`to` does not exist), so including them is always safe.
            let mut stmt = conn.prepare(
                "SELECT id, from_path, to_path FROM operation_item
                 WHERE operation_id = ?1 AND status IN ('pending', 'done')
                   AND action IN ('move', 'copy_delete')
                   AND from_path IS NOT NULL AND to_path IS NOT NULL",
            )?;
            let rows = stmt.query_map([op_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };

        for (item_id, from_path, to_path) in items {
            let from = Path::new(&from_path);
            let to = Path::new(&to_path);
            // Reverse the move only if it actually applied (copy is at `to`) and
            // the original slot is free; best-effort, never overwrites. (`move_path`
            // is itself long-path-safe; these presence checks are normalized too.)
            if path_present(to) && !path_present(from) {
                let strategy = fsops::choose_strategy(to, from);
                let _ = fsops::move_path(to, from, strategy);
            }
            if kind == "quarantine" && path_present(from) && path_present(to) {
                expose_ambiguous_held_copy(conn, op_id, &from_path, &to_path, backup_id)?;
                conn.execute(
                    "UPDATE operation_item SET status = 'done' WHERE id = ?1",
                    [item_id],
                )?;
            }
            // Retire the journal item ONLY if the reverse actually landed the file back at its
            // original path. If it did not (held copy locked, cross-volume failure, slot raced),
            // leave the item 'done' so the held copy stays recoverable — never claim success.
            if path_present(from) && !path_present(to) {
                conn.execute(
                    "UPDATE operation_item SET status = 'rolled_back' WHERE id = ?1",
                    [item_id],
                )?;
                report.rolled_back_items += 1;
            }
        }

        // Mark a quarantine_entry 'restored' ONLY when on-disk truth confirms the file is back at
        // its original path AND the held copy is gone. Otherwise leave it 'quarantined' so the
        // held copy stays visible and recoverable in Recover. Marking an entry 'restored' while
        // its only copy is still in the holding area would hide the data and lie to the user
        // (the very data-loss this guards against).
        let entries: Vec<(i64, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, original_path, quarantine_path FROM quarantine_entry
                 WHERE operation_id = ?1 AND status = 'quarantined'",
            )?;
            let rows = stmt.query_map([op_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for (entry_id, original_path, quarantine_path) in entries {
            // The held copy must be PROVABLY gone (not merely `exists()==false`, which a
            // transiently unreadable holding volume also returns) before we mark the entry
            // 'restored' and stop surfacing the held copy — otherwise an unreachable-but-
            // present copy would be hidden as if it were the successfully restored original.
            let restored = path_present(Path::new(&original_path))
                && path_provably_absent(Path::new(&quarantine_path));
            if restored {
                conn.execute(
                    "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
                    [entry_id],
                )?;
            }
        }
        // The operation is terminal (we reversed everything we safely could); any entry that
        // could not be put back stays 'quarantined' and remains recoverable from the Recover view.
        conn.execute(
            "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
            params![op_id, chrono::Utc::now().to_rfc3339()],
        )?;
        report.recovered_operations += 1;
    }

    // Reconcile any permanent delete interrupted mid-unlink: the entry was flipped
    // to 'deleting' BEFORE the unlink, so the on-disk truth of the held copy decides
    // the outcome — gone means the delete completed, still present means it did not.
    let deleting: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, quarantine_path FROM quarantine_entry WHERE status = 'deleting'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (entry_id, quarantine_path) in deleting {
        // Only a PROVABLE absence settles a permanent delete as done. A transiently
        // unreadable held path (removable/network/lock) must NOT be recorded
        // 'permanently_deleted' — that would hide a file that is merely unreachable. Fail
        // closed to 'quarantined' (still visible/recoverable) until absence is proven.
        let resolved = if path_provably_absent(Path::new(&quarantine_path)) {
            "permanently_deleted"
        } else {
            "quarantined"
        };
        conn.execute(
            "UPDATE quarantine_entry SET status = ?2 WHERE id = ?1",
            params![entry_id, resolved],
        )?;
        report.rolled_back_items += 1;
    }

    // Close out any permanent-delete operations left 'executing'; the entry outcome
    // above is the source of truth, so the operation row is just finalised.
    let finalised = conn.execute(
        "UPDATE operation SET status = 'rolled_back', finished_at = ?1
         WHERE kind = 'permanent_delete' AND status = 'executing'",
        params![chrono::Utc::now().to_rfc3339()],
    )?;
    report.recovered_operations += finalised;

    // Reconcile interrupted restores by on-disk truth, keyed by the entry id the
    // restore recorded in its plan_json. A COMPLETED restore (file already at its
    // destination) must be finalised in place, never yanked back into quarantine (the
    // bug the generic rollback would cause — restore is excluded from it). Matching by
    // entry id, not quarantine_path: the path is not unique, so a path match could
    // touch (and re-wedge) the wrong entry.
    let restore_ops: Vec<(i64, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, plan_json, error FROM operation
             WHERE kind = 'restore' AND status IN ('executing', 'verifying')
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (op_id, plan_json, existing_error) in restore_ops {
        let now = chrono::Utc::now().to_rfc3339();
        let parsed = serde_json::from_str::<serde_json::Value>(&plan_json).ok();
        let entry_id = parsed
            .as_ref()
            .and_then(|value| value.get("restore_entry"))
            .and_then(serde_json::Value::as_i64);
        let destination = parsed
            .as_ref()
            .and_then(|value| value.get("destination"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // Matching by entry id, not quarantine_path: the path is not unique, so a path match
        // could touch (and re-wedge) the wrong entry. `held_path` is None when the entry row is
        // gone (e.g. a completed restore already consumed it).
        let held_row = entry_id
            .map(|eid| {
                conn.query_row(
                    "SELECT quarantine_path, manifest_json FROM quarantine_entry WHERE id = ?1",
                    [eid],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()
            })
            .transpose()?
            .flatten();
        let held_path = held_row.as_ref().map(|(path, _)| path.clone());
        // The verified content hash the quarantine recorded for this file (blake3 of the
        // backed-up copy == the original bytes the destination should now hold). Used to
        // content-verify the destination before finalizing 'restored'. `None` when the
        // quarantine had no linked backup, in which case we rely on provable-absence alone.
        let expected_hash = held_row
            .as_ref()
            .and_then(|(_, manifest)| manifest.as_deref())
            .and_then(|manifest| serde_json::from_str::<serde_json::Value>(manifest).ok())
            .and_then(|value| {
                value
                    .get("backup_blake3")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        // Reconcile purely by on-disk truth. "completed" requires the destination present AND
        // the held copy gone; a crash mid-copy (cross-volume restore is copy→verify→delete-
        // source) leaves a possibly-truncated file at the destination while the intact held
        // copy still sits in holding, so that case must NOT be finalized 'restored'.
        let destination_present = destination
            .as_deref()
            .map(|dest| path_present(Path::new(dest)))
            .unwrap_or(false);
        // "Held gone" must be PROVEN: a transiently-unreadable held path counts as PRESENT
        // so we roll back (keep it visible) instead of finalizing a restore we cannot
        // substantiate and hiding the only good copy.
        let held_present = held_path
            .as_deref()
            .map(|held| !path_provably_absent(Path::new(held)))
            .unwrap_or(false);
        // Belt-and-suspenders for the one residual provable-absence cannot cover: a cleanly
        // unmounted volume whose held path returns NotFound is indistinguishable from a real
        // absence. When the entry recorded a content hash, require the destination to hash to
        // it before finalizing 'restored'; a crash mid-copy leaves a truncated destination
        // that fails this check and stays fail-closed instead of enshrining corruption. With
        // no recorded hash, provable-absence carries the decision alone.
        let destination_content_ok = match (destination.as_deref(), expected_hash.as_deref()) {
            (Some(dest), Some(expected)) => {
                matches!(fsops::hash_file(Path::new(dest)), Ok(actual) if actual == expected)
            }
            _ => true,
        };
        let resolved = match entry_id {
            None => {
                // Malformed/legacy restore op — finalise so it cannot wedge recovery.
                conn.execute(
                    "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
                    params![op_id, now],
                )?;
                true
            }
            Some(eid) if destination_present && !held_present && destination_content_ok => {
                // Restore COMPLETED: the file is at its destination and no held copy remains
                // (the quarantine_entry row may itself be gone). Finalise in place — never yank
                // it back into quarantine, never wedge. This also fixes the over-trigger where a
                // present destination with a vanished entry row fell into the infinite
                // 'verifying' arm below. If the entry row still exists, mark it 'restored';
                // otherwise the operation finalisation is enough.
                conn.execute(
                    "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
                    [eid],
                )?;
                conn.execute(
                    "UPDATE operation_item SET status = 'done' WHERE operation_id = ?1",
                    [op_id],
                )?;
                conn.execute(
                    "UPDATE operation SET status = 'done', finished_at = ?2 WHERE id = ?1",
                    params![op_id, now],
                )?;
                true
            }
            Some(eid) if held_present => {
                // Restore did not complete — the held copy is still recoverable. Return the
                // entry to 'quarantined' (held copy visible again); a retry then surfaces the
                // occupied destination as a Conflict for the user to resolve.
                conn.execute(
                    "UPDATE quarantine_entry SET status = 'quarantined' WHERE id = ?1",
                    [eid],
                )?;
                conn.execute(
                    "UPDATE operation_item SET status = 'rolled_back' WHERE operation_id = ?1",
                    [op_id],
                )?;
                conn.execute(
                    "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
                    params![op_id, now],
                )?;
                true
            }
            Some(eid) if destination_present && !held_present && !destination_content_ok => {
                // The move reached its destination and the held path is gone, but the bytes no
                // longer match the verified hash. This is materially different from both paths
                // being absent: preserve the destination, surface the content mismatch once,
                // then let a second deliberate resolve close the operation without claiming the
                // file was restored or leaving a fake restorable held entry behind.
                if existing_error.as_deref() == Some(RESTORE_CONTENT_MISMATCH_ERROR) {
                    conn.execute(
                        "UPDATE quarantine_entry
                         SET status = 'restore_content_mismatch'
                         WHERE id = ?1",
                        [eid],
                    )?;
                    conn.execute(
                        "UPDATE operation_item SET status = 'done' WHERE operation_id = ?1",
                        [op_id],
                    )?;
                    conn.execute(
                        "UPDATE operation
                         SET status = 'failed', finished_at = ?2, error = ?3
                         WHERE id = ?1",
                        params![op_id, now, RESTORE_CONTENT_MISMATCH_RESOLVED],
                    )?;
                    true
                } else {
                    conn.execute(
                        "UPDATE operation SET status = 'verifying', error = ?2 WHERE id = ?1",
                        params![op_id, RESTORE_CONTENT_MISMATCH_ERROR],
                    )?;
                    false
                }
            }
            Some(_) => {
                // Genuine ambiguity: neither the held copy nor the restore destination is
                // visible. Stay FAIL-CLOSED — never claim a restore we cannot prove. But avoid
                // an infinite 'verifying' wedge that would block EVERY future mutation with no
                // escape: the first reconciliation records the condition and keeps blocking (so
                // the user is forced to notice it in Recovery); a subsequent, deliberate
                // recovery_resolve — the user acting on that surfaced state — then abandons the
                // unprovable restore terminally so it stops blocking new operations. The entry
                // (if any) is left 'quarantined' so a vanished held copy stays visible as a
                // loss; we never fabricate a 'restored' result.
                if existing_error.as_deref() == Some(RESTORE_AMBIGUOUS_ERROR) {
                    conn.execute(
                        "UPDATE operation
                         SET status = 'rolled_back', finished_at = ?2, error = ?3
                         WHERE id = ?1",
                        params![op_id, now, RESTORE_AMBIGUOUS_ABANDONED],
                    )?;
                    true
                } else {
                    conn.execute(
                        "UPDATE operation SET status = 'verifying', error = ?2 WHERE id = ?1",
                        params![op_id, RESTORE_AMBIGUOUS_ERROR],
                    )?;
                    false
                }
            }
        };
        if resolved {
            report.recovered_operations += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn journaled_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn rolls_back_an_interrupted_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("project/output/render.bin");
        let quarantined = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        // Simulate a crash mid-quarantine: the file was moved to quarantine
        // (done item) but the operation never finished.
        fs::write(&quarantined, b"render payload").unwrap();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('quarantine', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'done')",
            params![
                op_id,
                original.to_string_lossy(),
                quarantined.to_string_lossy()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO quarantine_entry(operation_id, original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, ?3, 'quarantined', '{}')",
            params![op_id, original.to_string_lossy(), quarantined.to_string_lossy()],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(report.rolled_back_items, 1);

        // The file is back at its original path; the quarantine copy is gone.
        assert!(original.exists());
        assert_eq!(fs::read(&original).unwrap(), b"render payload");
        assert!(!quarantined.exists());

        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "rolled_back");
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE operation_id = ?1",
                [op_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "restored");
    }

    #[test]
    fn rolls_back_an_applied_move_left_pending() {
        // Crash in the window between the rename syscall and the 'done' journal
        // update: the move APPLIED on disk but the item still reads 'pending'.
        // Recovery must reverse it — otherwise the file is stranded in holding
        // with no Recover entry and no rollback.
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("project/output/render.bin");
        let quarantined = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::write(&quarantined, b"render payload").unwrap();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('quarantine', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'pending')",
            params![
                op_id,
                original.to_string_lossy(),
                quarantined.to_string_lossy()
            ],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.rolled_back_items, 1);
        assert!(original.exists());
        assert!(!quarantined.exists());
        let item_status: String = conn
            .query_row(
                "SELECT status FROM operation_item WHERE operation_id = ?1",
                [op_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(item_status, "rolled_back");
    }

    #[test]
    fn exposes_a_cross_volume_copy_left_beside_the_original() {
        // Cross-volume quarantine copies and verifies before deleting the source. A
        // stop before the quarantine-entry insert leaves two good copies. Recovery
        // must not delete either and must not leave the held copy as an invisible orphan.
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/op-1/output/render.bin");
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::create_dir_all(held.parent().unwrap()).unwrap();
        fs::write(&original, b"original remains").unwrap();
        fs::write(&held, b"verified held copy").unwrap();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO backup(level, destination, manifest_path, verified, created_at)
             VALUES('core', 'backup', 'backup/manifest.json', 1, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let backup_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, backup_id, created_at)
             VALUES('quarantine', 'executing', '{}', ?1, '2026-01-01T00:00:00Z')",
            [backup_id],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'copy_delete', ?2, ?3, 'pending')",
            params![op_id, original.to_string_lossy(), held.to_string_lossy()],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert!(original.exists());
        assert!(held.exists());
        assert_eq!(fs::read(&original).unwrap(), b"original remains");
        assert_eq!(fs::read(&held).unwrap(), b"verified held copy");

        let entry: (String, String, Option<i64>) = conn
            .query_row(
                "SELECT status, quarantine_path, backup_id
                 FROM quarantine_entry WHERE operation_id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(entry.0, "quarantined");
        assert_eq!(entry.1, held.to_string_lossy());
        assert_eq!(entry.2, Some(backup_id));
        let item_status: String = conn
            .query_row(
                "SELECT status FROM operation_item WHERE operation_id = ?1",
                [op_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(item_status, "done");
    }

    #[test]
    fn corrects_optimistic_recovered_bytes_when_both_quarantine_copies_survive() {
        // Crash after quarantine_entry INSERT but before source unlink: the row still claims
        // cross-volume recovery, while both the source and held copy occupy disk. Recovery must
        // keep both copies and correct both the column and manifest to zero recovered bytes.
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/op-1/output/render.bin");
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::create_dir_all(held.parent().unwrap()).unwrap();
        fs::write(&original, b"same verified payload").unwrap();
        fs::write(&held, b"same verified payload").unwrap();
        let bytes = fs::metadata(&original).unwrap().len() as i64;

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('quarantine', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'copy_delete', ?2, ?3, 'done')",
            params![op_id, original.to_string_lossy(), held.to_string_lossy()],
        )
        .unwrap();
        let manifest = serde_json::json!({
            "original_path": original.to_string_lossy(),
            "quarantine_path": held.to_string_lossy(),
            "relative": "output/render.bin",
            "bytes": bytes,
            "space_recovered": bytes,
            "backup_blake3": "kept-hash",
        })
        .to_string();
        conn.execute(
            "INSERT INTO quarantine_entry(
                 operation_id, original_path, quarantine_path, size,
                 space_recovered, status, manifest_json
             ) VALUES(?1, ?2, ?3, ?4, ?4, 'quarantined', ?5)",
            params![
                op_id,
                original.to_string_lossy(),
                held.to_string_lossy(),
                bytes,
                manifest
            ],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();

        assert_eq!(report.recovered_operations, 1);
        assert!(original.exists());
        assert!(held.exists());
        let (status, recovered, manifest): (String, i64, String) = conn
            .query_row(
                "SELECT status, space_recovered, manifest_json
                 FROM quarantine_entry WHERE operation_id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(status, "quarantined");
        assert_eq!(recovered, 0);
        assert_eq!(manifest["space_recovered"], 0);
        assert_eq!(manifest["backup_blake3"], "kept-hash");
        assert_eq!(
            manifest["recovery_reason"],
            "both original and held copies existed after interruption"
        );
    }

    #[test]
    fn does_not_finalize_a_restore_whose_held_copy_remains() {
        // Crash mid-copy of a cross-volume restore: a (possibly truncated) file
        // sits at the destination while the intact held copy is still in holding.
        // Reconciliation must NOT mark this 'restored' — the entry returns to
        // 'quarantined' so the good copy stays visible; a retry surfaces the
        // occupied destination as a Conflict.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let quarantined = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
        fs::write(&destination, b"trunc").unwrap(); // partial copy
        fs::write(&quarantined, b"full held payload").unwrap(); // intact held copy

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'restored', '{}')",
            params![destination.to_string_lossy(), quarantined.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();

        recover_interrupted(&conn).unwrap();

        // Held copy untouched and visible again; nothing finalized as restored.
        assert!(quarantined.exists());
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "quarantined");
        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "rolled_back");
    }

    #[test]
    fn reconciles_an_interrupted_permanent_delete() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();

        // Case 1: crash AFTER flipping to 'deleting' but BEFORE the unlink — the
        // held copy still exists, so recovery returns it to 'quarantined'.
        let held_present = dir.path().join("held/present.bin");
        fs::create_dir_all(held_present.parent().unwrap()).unwrap();
        fs::write(&held_present, b"still here").unwrap();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES('orig/present.bin', ?1, 'deleting', '{}')",
            params![held_present.to_string_lossy()],
        )
        .unwrap();
        let present_id = conn.last_insert_rowid();

        // Case 2: crash AFTER the unlink but BEFORE the final update — the held copy
        // is gone, so recovery settles it as 'permanently_deleted'.
        let held_gone = dir.path().join("held/gone.bin");
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES('orig/gone.bin', ?1, 'deleting', '{}')",
            params![held_gone.to_string_lossy()],
        )
        .unwrap();
        let gone_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('permanent_delete', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();

        recover_interrupted(&conn).unwrap();

        let status = |id: i64| -> String {
            conn.query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(status(present_id), "quarantined");
        assert!(held_present.exists());
        assert_eq!(status(gone_id), "permanently_deleted");

        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "rolled_back");
    }

    #[test]
    fn finalizes_a_completed_restore_without_yanking_it_back() {
        let dir = tempfile::tempdir().unwrap();
        // A restore that completed the move: the file is at its destination and the
        // quarantine slot is empty. The operation crashed before being finalized.
        // The generic rollback would have reverse-moved it back into quarantine — the
        // HIGH-severity bug this guards against. Restore must be finalized in place.
        let destination = dir.path().join("project/output/render.bin");
        let quarantined = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"restored payload").unwrap();

        let conn = journaled_conn();
        // The entry exists first; the restore op references it by id in plan_json.
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'restored', '{}')",
            params![destination.to_string_lossy(), quarantined.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'done')",
            params![
                op_id,
                quarantined.to_string_lossy(),
                destination.to_string_lossy()
            ],
        )
        .unwrap();

        recover_interrupted(&conn).unwrap();

        // The restored file stays where the user restored it — never pulled back.
        assert!(destination.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"restored payload");
        assert!(!quarantined.exists());
        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "done");
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "restored");
    }

    #[test]
    fn rolls_an_incomplete_restore_back_to_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        // A restore that never reached its destination: the held copy is still in
        // quarantine. Recovery must return the entry to 'quarantined' (still usable).
        let destination = dir.path().join("project/output/render.bin");
        let quarantined = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&quarantined, b"still held").unwrap();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'restored', '{}')",
            params![destination.to_string_lossy(), quarantined.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'pending')",
            params![
                op_id,
                quarantined.to_string_lossy(),
                destination.to_string_lossy()
            ],
        )
        .unwrap();

        recover_interrupted(&conn).unwrap();

        assert!(quarantined.exists());
        assert!(!destination.exists());
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "quarantined");
        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "rolled_back");
    }

    #[test]
    fn does_not_finalize_a_restore_whose_destination_fails_content_verification() {
        // A crash mid cross-volume restore (copy -> verify -> delete-source) can leave a
        // TRUNCATED destination while the held copy is unlinked or on a now-unreadable
        // volume. On restart the held path is gone (NotFound) but the destination exists —
        // the exact case that must NOT be enshrined as 'restored'. The recorded
        // backup_blake3 no longer matches the truncated destination, so recovery stays
        // fail-closed instead of declaring the corrupt file the restored original.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/output/render.bin"); // never created -> gone
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"truncated-partial").unwrap();
        let expected = blake3::hash(b"the original whole file")
            .to_hex()
            .to_string();
        let manifest = serde_json::json!({ "backup_blake3": expected }).to_string();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'quarantined', ?3)",
            params![
                destination.to_string_lossy(),
                held.to_string_lossy(),
                manifest
            ],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'verifying', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'pending')",
            params![op_id, held.to_string_lossy(), destination.to_string_lossy()],
        )
        .unwrap();

        let first = recover_interrupted(&conn).unwrap();

        assert_eq!(first.recovered_operations, 0);
        let (op_status, op_error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(op_status, "verifying");
        assert_eq!(op_error.as_deref(), Some(RESTORE_CONTENT_MISMATCH_ERROR));
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "quarantined");
        assert_eq!(fs::read(&destination).unwrap(), b"truncated-partial");

        let second = recover_interrupted(&conn).unwrap();

        assert_eq!(second.recovered_operations, 1);
        let (op_status, op_error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(op_status, "failed");
        assert_eq!(op_error.as_deref(), Some(RESTORE_CONTENT_MISMATCH_RESOLVED));
        let (entry_status, item_status): (String, String) = conn
            .query_row(
                "SELECT quarantine_entry.status, operation_item.status
                 FROM quarantine_entry
                 JOIN operation_item ON operation_item.operation_id = ?2
                 WHERE quarantine_entry.id = ?1",
                params![entry_id, op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(entry_status, "restore_content_mismatch");
        assert_eq!(item_status, "done");
        assert_eq!(fs::read(&destination).unwrap(), b"truncated-partial");
        assert!(!held.exists());
    }

    #[test]
    fn finalizes_a_restore_whose_destination_matches_the_recorded_hash() {
        // The legitimate completed-restore case still finalizes with the new content gate:
        // the held copy is genuinely gone (NotFound) and the destination hashes to the
        // recorded backup_blake3.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/output/render.bin"); // never created -> gone
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let content = b"the original whole file";
        fs::write(&destination, content).unwrap();
        let expected = blake3::hash(content).to_hex().to_string();
        let manifest = serde_json::json!({ "backup_blake3": expected }).to_string();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'quarantined', ?3)",
            params![
                destination.to_string_lossy(),
                held.to_string_lossy(),
                manifest
            ],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'verifying', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'pending')",
            params![op_id, held.to_string_lossy(), destination.to_string_lossy()],
        )
        .unwrap();

        recover_interrupted(&conn).unwrap();

        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "done");
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "restored");
    }

    #[test]
    fn keeps_a_restore_blocking_when_both_copies_are_missing() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/output/render.bin");
        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'quarantined', '{}')",
            params![destination.to_string_lossy(), held.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('restore', 'verifying', ?1, '2026-01-01T00:00:00Z')",
            [plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
             VALUES(?1, 'move', ?2, ?3, 'pending')",
            params![op_id, held.to_string_lossy(), destination.to_string_lossy()],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 0);
        let operation: (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(operation.0, "verifying");
        assert!(operation
            .1
            .as_deref()
            .is_some_and(|error| error.contains("could not find either")));
    }

    #[test]
    fn restore_reconciliation_matches_by_id_not_shared_path() {
        // quarantine_path is not unique. An incomplete restore must roll back ONLY its
        // own entry by id — never every entry that happens to share the path, which
        // would re-wedge an unrelated entry already restored elsewhere.
        let dir = tempfile::tempdir().unwrap();
        let shared_q = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(shared_q.parent().unwrap()).unwrap();
        let conn = journaled_conn();

        // Entry A: already restored to another folder; its held copy at the shared
        // path is gone. It has no interrupted operation.
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES('a/orig.bin', ?1, 'restored', '{}')",
            params![shared_q.to_string_lossy()],
        )
        .unwrap();
        let a_id = conn.last_insert_rowid();

        // Entry B: shares the path; its restore is incomplete (held copy still present,
        // destination not yet written).
        fs::write(&shared_q, b"b held").unwrap();
        let b_dest = dir.path().join("project/b.bin");
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'restored', '{}')",
            params![b_dest.to_string_lossy(), shared_q.to_string_lossy()],
        )
        .unwrap();
        let b_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": b_id,
            "destination": b_dest.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();

        recover_interrupted(&conn).unwrap();

        let status = |id: i64| -> String {
            conn.query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        // B rolled back to quarantined; A untouched despite the shared path.
        assert_eq!(status(b_id), "quarantined");
        assert_eq!(status(a_id), "restored");
    }

    #[test]
    fn leaves_finished_operations_untouched() {
        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('quarantine', 'done', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report, RecoveryReport::default());
        let status: String = conn
            .query_row(
                "SELECT status FROM operation WHERE kind = 'quarantine'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "done");
    }

    #[test]
    fn finalizes_a_restore_whose_destination_survived_but_entry_row_vanished() {
        // Over-trigger fix: entry_id present in the plan, destination present on disk, but the
        // quarantine_entry row is gone (the completed restore consumed it). The restore
        // demonstrably finished — the file is at its destination — so recovery must FINALIZE it,
        // not fall into the fail-closed 'verifying' wedge that permanently blocks mutations.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"restored payload").unwrap();

        let conn = journaled_conn();
        // The entry existed then was removed as the restore completed.
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, 'holding/render.bin', 'restored', '{}')",
            params![destination.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        conn.execute("DELETE FROM quarantine_entry WHERE id = ?1", [entry_id])
            .unwrap();

        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'verifying', ?1, '2026-01-01T00:00:00Z')",
            params![plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        let op_status: String = conn
            .query_row("SELECT status FROM operation WHERE id = ?1", [op_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(op_status, "done");
        assert!(destination.exists());
    }

    #[test]
    fn abandons_a_persistently_stuck_restore_on_the_second_resolve() {
        // Both copies genuinely absent. The FIRST reconciliation stays fail-closed
        // ('verifying' + error, still blocking so the user is forced to notice it), but a
        // SECOND, deliberate recovery_resolve gives the otherwise-permanently-'verifying' op a
        // terminal exit so it stops blocking every future mutation. It is NEVER marked restored.
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/output/render.bin");
        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO quarantine_entry(original_path, quarantine_path, status, manifest_json)
             VALUES(?1, ?2, 'quarantined', '{}')",
            params![destination.to_string_lossy(), held.to_string_lossy()],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let plan = serde_json::json!({
            "restore_entry": entry_id,
            "destination": destination.to_string_lossy(),
        })
        .to_string();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('restore', 'verifying', ?1, '2026-01-01T00:00:00Z')",
            [plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();

        // First resolve: fail-closed, still blocking.
        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 0);
        let (status, error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "verifying");
        assert!(error
            .as_deref()
            .is_some_and(|error| error.contains("could not find either")));

        // Second resolve: the user has seen the surfaced state and re-resolves — terminal exit.
        let second = recover_interrupted(&conn).unwrap();
        assert_eq!(second.recovered_operations, 1);
        let (status, error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "rolled_back", "must no longer block new mutations");
        assert!(error
            .as_deref()
            .is_some_and(|error| error.contains("abandoned")));
        // Fail-closed preserved: the entry is never fabricated as 'restored'.
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entry_status, "quarantined");
    }
}
