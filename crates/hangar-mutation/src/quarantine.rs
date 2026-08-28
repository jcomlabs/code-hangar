//! Quarantine executor — the first operation that **moves** files.
//!
//! Quarantine relocates files into a quarantine root, preserving their relative
//! layout. It is reversible (restore is a later milestone) and journaled, so an
//! interrupted run can be recovered. Safety is enforced *in the executor*,
//! independent of whatever the plan claims:
//!
//! - **Protected Zones / sensitive files are never moved** (re-checked here).
//! - **Reparse points are never followed** (no-follow): symlinks/junctions are
//!   skipped rather than traversed.
//! - **Journal-first:** each item is recorded `pending` before it is touched.
//! - **Cross-volume safety:** the source is deleted only after its copy is
//!   re-verified with blake3.
//! - **Per-item isolation:** a locked/failed item is flagged and skipped; it
//!   does not abort the other items.
//!
//! `space_recovered` is truthful: a same-volume rename frees nothing until a
//! later permanent delete, so it reports `0`; a cross-volume copy+delete frees
//! the source-volume bytes.

#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;
use thiserror::Error;

use crate::bound_fs::{self, BoundDirectory, BoundFile, FileStamp, MoveKind};

#[derive(Debug, Error)]
pub enum QuarantineError {
    #[error("quarantine io error: {0}")]
    Io(#[from] io::Error),
    #[error("quarantine journal error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("checksum mismatch while moving {path}")]
    ChecksumMismatch { path: String },
    #[error("refusing to overwrite an occupied holding-area path: {path}")]
    DestinationOccupied { path: String },
    #[error("refusing to mutate cloud-backed path {path} ({kind})")]
    CloudFileRefused { path: String, kind: String },
    #[error("refusing to move reparse source {path} ({kind})")]
    SourceIsReparse { path: String, kind: String },
    #[error("the holding folder {path} is inside the folder being emptied")]
    HoldingInsideTarget { path: String },
    #[error("reparse-link removal is disabled until links have a faithful reversible journal")]
    ReparseRemovalDisabled,
}

#[derive(Debug, Clone)]
pub struct QuarantineItem {
    pub source: PathBuf,
    pub relative: String,
    pub expected_source_stamp: FileStamp,
    /// blake3 of the verified backup copy of this exact reviewed file.
    pub backup_hash: String,
}

pub struct QuarantineRequest<'a> {
    pub quarantine_root: &'a Path,
    pub items: Vec<QuarantineItem>,
    /// Operation Plan JSON that authorised this quarantine (recorded verbatim).
    pub plan_json: String,
    pub target_node_id: Option<i64>,
    pub target_fingerprint: Option<String>,
    /// The verified backup (`backup.id`) that covers every item — the Gate-3
    /// precondition for ever permanently deleting the held copies. Recorded on
    /// each `quarantine_entry`.
    pub backup_id: i64,
    /// When set, after the files are moved out, remove the now-empty directories of
    /// this subtree (deepest-first) so a whole project folder is actually gone from
    /// disk. Reparse-point dirs and dirs still holding skipped content are left
    /// intact; rollback recreates any removed dir when it moves a file back.
    pub cleanup_root: Option<PathBuf>,
    /// When true the user explicitly opted into emptying the folder: sensitive/protected
    /// files are moved (they were backed up first), instead of being skipped. Reparse
    /// points remain forbidden until reversible link journaling exists.
    pub include_protected: bool,
    /// Reparse points discovered by the caller. Any non-empty value rejects the
    /// entire request before journaling or touching an ordinary file.
    pub reparse_links: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemOutcome {
    /// Same-volume rename (frees no space until permanent delete).
    Moved,
    /// Cross-volume copy + verified delete (frees source-volume bytes).
    Copied,
    SkippedProtected,
    SkippedReparse,
    Failed,
}

#[derive(Debug, Clone)]
pub struct QuarantineEntryResult {
    pub original_path: String,
    pub quarantine_path: Option<String>,
    pub outcome: ItemOutcome,
    pub bytes: u64,
    pub space_recovered: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuarantineResult {
    pub operation_id: i64,
    pub entries: Vec<QuarantineEntryResult>,
    pub space_recovered: u64,
    pub moved: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Empty source directories removed after the move (recursive folder cleanup).
    pub removed_dirs: usize,
    /// Reparse points (junction/symlink) removed as links during an opt-in empty.
    pub removed_links: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveStrategy {
    CopyDelete,
}

/// Execute a quarantine over `request.items`, journaling the operation and each
/// item. Protected/sensitive paths and reparse points are refused here even if
/// the plan listed them. Returns per-item outcomes and the truthful recovered
/// bytes.
pub fn quarantine(
    conn: &Connection,
    request: QuarantineRequest<'_>,
) -> Result<QuarantineResult, QuarantineError> {
    // Link deletion is intentionally disabled until the journal can recreate the
    // exact symlink/junction type and target. Refuse the whole request before even
    // inspecting a supplied path, journaling, or moving an ordinary file.
    if !request.reparse_links.is_empty() {
        return Err(QuarantineError::ReparseRemovalDisabled);
    }
    bound_fs::validate_local_mutation_path(request.quarantine_root)?;
    if let Some(cleanup_root) = &request.cleanup_root {
        bound_fs::validate_local_mutation_path(cleanup_root)?;
    }
    for item in &request.items {
        bound_fs::validate_local_mutation_path(&item.source)?;
    }

    // Containment guard, symmetric to the backup engine's is_inside check: when the move will
    // recursively empty `cleanup_root` (a project/directory target), a holding root inside
    // that tree would relocate the held copies into the very folder being emptied — leaving it
    // non-empty and the holding area exposed to a later external delete. Refuse before
    // journaling anything.
    if let Some(cleanup_root) = &request.cleanup_root {
        if crate::backup::is_inside(request.quarantine_root, cleanup_root) {
            return Err(QuarantineError::HoldingInsideTarget {
                path: request.quarantine_root.to_string_lossy().to_string(),
            });
        }
    }
    let started_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO operation(kind, status, plan_json, target_node_id, target_fingerprint, backup_id, created_at, started_at)
         VALUES('quarantine', 'executing', ?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            request.plan_json,
            request.target_node_id,
            request.target_fingerprint,
            (request.backup_id > 0).then_some(request.backup_id),
            started_at
        ],
    )?;
    let operation_id = conn.last_insert_rowid();

    let mut entries = Vec::with_capacity(request.items.len());
    let mut moved = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut space_recovered = 0u64;
    let mut moved_originals: Vec<PathBuf> = Vec::new();

    for item in &request.items {
        let original = item.source.to_string_lossy().to_string();

        // Defense-in-depth: never move a Protected Zone or sensitive file UNLESS the
        // user explicitly opted into emptying the folder (in which case it was backed up
        // first — the move-time backup-coverage check enforces that upstream).
        if !request.include_protected && is_protected_or_sensitive(&item.source) {
            record_item(conn, operation_id, &original, None, "noop", "skipped")?;
            entries.push(skipped_entry(
                original,
                ItemOutcome::SkippedProtected,
                "protected or sensitive path",
            ));
            skipped += 1;
            continue;
        }

        // Namespace each operation's holding files under a per-operation subdir so two
        // moves of files that share a project-relative path (e.g. two projects each with
        // `output/render.bin`) into the same holding folder can never collide — a
        // collision would let the second move overwrite the first entry's only held copy.
        let dest = request
            .quarantine_root
            .join(format!("op-{operation_id}"))
            .join(normalize_relative(&item.relative));
        let dest_text = dest.to_string_lossy().to_string();
        // Bind the exact file before journaling the operation. The handle denies
        // path replacement, proves every ancestor is a plain directory and is
        // retained through rename/copy and (for cross-volume) source disposition.
        let mut bound = match BoundFile::open_for_move(&item.source) {
            Ok(bound) => bound,
            Err(err) => {
                record_item(
                    conn,
                    operation_id,
                    &original,
                    Some(&dest_text),
                    "move",
                    "failed",
                )?;
                failed += 1;
                entries.push(QuarantineEntryResult {
                    original_path: original,
                    quarantine_path: None,
                    outcome: ItemOutcome::Failed,
                    bytes: 0,
                    space_recovered: 0,
                    detail: Some(err.to_string()),
                });
                continue;
            }
        };
        if let Err(err) = bound
            .verify_stamp(&item.expected_source_stamp)
            .and_then(|_| bound.verify_hash(&item.backup_hash).map(|_| ()))
        {
            record_item(
                conn,
                operation_id,
                &original,
                Some(&dest_text),
                "move",
                "failed",
            )?;
            failed += 1;
            entries.push(QuarantineEntryResult {
                original_path: original,
                quarantine_path: None,
                outcome: ItemOutcome::Failed,
                bytes: 0,
                space_recovered: 0,
                detail: Some(err.to_string()),
            });
            continue;
        }
        let expected_hash = item.backup_hash.clone();
        let expected_stamp = bound.stamp().clone();
        // Journal-first: expected identity+hash are durable before any rename or
        // destination creation. Recovery may only act on this exact object.
        let item_id = record_bound_item(
            conn,
            operation_id,
            &original,
            &dest_text,
            &expected_stamp,
            &expected_hash,
        )?;

        match bound.prepare_move(&dest, &expected_hash) {
            Ok(mut prepared) => {
                let (action, outcome, recovered) = match prepared.kind() {
                    MoveKind::Rename => ("move", ItemOutcome::Moved, 0),
                };
                let bytes = prepared.bytes();
                set_bound_item_done(
                    conn,
                    item_id,
                    action,
                    prepared.destination_stamp(),
                    prepared.hash(),
                )?;
                let dest_text = dest.to_string_lossy().to_string();
                // CRASH-CONSISTENCY (R2-MED #3): journal the held copy BEFORE removing the source,
                // so at every instant at least one quarantine_entry-referenced copy of the bytes
                // exists on disk. The entry is inserted optimistically claiming `recovered` bytes
                // (a cross-volume move frees the source-volume bytes; a same-volume rename frees 0),
                // and ONLY after it is durable do we unlink the source below. Reversing this order
                // (unlink first) reopens the regression: a crash — or a SQLITE_BUSY/Err on this
                // INSERT — would leave the held copy on disk with no quarantine_entry, invisible to
                // Recover (load_stored_entries reads quarantine_entry only) and untracked.
                insert_quarantine_entry(
                    conn,
                    operation_id,
                    &original,
                    &dest_text,
                    &item.relative,
                    bytes,
                    recovered,
                    request.backup_id,
                    Some(&item.backup_hash),
                )?;
                // The held copy is now durably journaled. Remove the source: a same-volume rename
                // already relocated the bytes (no-op here); a cross-volume copy_delete unlinks the
                // verified original. Sol's honest error reporting is preserved — if the unlink
                // fails we keep BOTH copies and truthfully claim zero recovered bytes.
                let source_removal = match prepared.finalize_source_removal() {
                    Ok(()) => SourceRemovalResult {
                        recovered,
                        error: None,
                    },
                    Err(error) => SourceRemovalResult {
                        recovered: 0,
                        error: Some(format!(
                            "The held copy was verified, but the exact bound original could not be removed: {error}"
                        )),
                    },
                };
                if let Some(detail) = source_removal.error.as_deref() {
                    // Correct the already-durable entry to on-disk truth: both copies still exist,
                    // nothing was freed. This follow-up write only *edits* the entry — it never
                    // un-references the held copy, so the crash-consistency invariant above holds
                    // throughout (the source is never gone before a durable entry).
                    record_source_removal_failure(
                        conn,
                        operation_id,
                        &original,
                        &dest_text,
                        &item.relative,
                        bytes,
                        Some(&item.backup_hash),
                        detail,
                    )?;
                    failed += 1;
                } else {
                    moved += 1;
                    moved_originals.push(item.source.clone());
                }
                space_recovered += source_removal.recovered;
                entries.push(QuarantineEntryResult {
                    original_path: original,
                    quarantine_path: Some(dest_text),
                    outcome,
                    bytes,
                    space_recovered: source_removal.recovered,
                    detail: source_removal.error,
                });
            }
            Err(err) => {
                set_item_status(conn, item_id, "failed")?;
                failed += 1;
                entries.push(QuarantineEntryResult {
                    original_path: original,
                    quarantine_path: None,
                    outcome: ItemOutcome::Failed,
                    bytes: 0,
                    space_recovered: 0,
                    detail: Some(err.to_string()),
                });
            }
        }
    }

    // Link deletion is fail-closed at the function boundary above. Keep the
    // result field for wire compatibility, but this executor can no longer
    // report that a symlink/junction was irreversibly unlinked.
    let removed_links = 0usize;

    // Recursive folder cleanup: with the files moved out, remove the now-empty
    // source directories so a whole project folder is actually gone from disk. Only
    // empty, non-reparse dirs are removed, so a dir still holding a skipped
    // protected/reparse file survives; rollback recreates dirs as it moves files back.
    let (removed_dirs, cleanup_failed) = match &request.cleanup_root {
        Some(root) => cleanup_empty_dirs(conn, operation_id, &moved_originals, root)?,
        None => (0, 0),
    };
    failed += cleanup_failed;

    let status = if failed > 0 { "failed" } else { "done" };
    conn.execute(
        "UPDATE operation SET status = ?1, finished_at = ?2, recovered_bytes = ?3 WHERE id = ?4",
        params![
            status,
            chrono::Utc::now().to_rfc3339(),
            space_recovered as i64,
            operation_id
        ],
    )?;

    Ok(QuarantineResult {
        operation_id,
        entries,
        space_recovered,
        moved,
        skipped,
        failed,
        removed_dirs,
        removed_links,
    })
}

/// Remove the now-empty directories of a moved subtree, deepest-first. Every
/// directory is opened as a plain local directory with its entire ancestor chain
/// retained, journaled `pending`, and then disposed through that exact handle.
/// A non-empty or replaced directory is recorded as `failed`; callers therefore
/// cannot mistake incomplete cleanup for a complete project removal.
fn cleanup_empty_dirs(
    conn: &Connection,
    operation_id: i64,
    moved_originals: &[PathBuf],
    root: &Path,
) -> Result<(usize, usize), QuarantineError> {
    use std::collections::BTreeSet;
    // Namespace validation deliberately precedes every filesystem probe. Lexical
    // containment is safe here because absolute parent traversal was rejected and
    // BoundDirectory later proves each ancestor is a plain, local directory.
    bound_fs::validate_local_mutation_path(root)?;
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for original in moved_originals {
        bound_fs::validate_local_mutation_path(original)?;
        let mut cursor = original.parent();
        while let Some(dir) = cursor {
            if !crate::backup::is_inside(dir, root) && dir != root {
                break; // left the target subtree — never delete above the root
            }
            dirs.insert(dir.to_path_buf());
            if dir == root {
                break;
            }
            cursor = dir.parent();
        }
    }
    // Deepest paths first so a parent becomes empty once its children are removed.
    let mut ordered: Vec<PathBuf> = dirs.into_iter().collect();
    ordered.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));

    let mut removed = 0usize;
    let mut failed = 0usize;
    for dir in ordered {
        bound_fs::validate_local_mutation_path(&dir)?;
        let dir_str = dir.to_string_lossy().to_string();
        // Journal before opening/disposition. A crash or failed handle proof
        // therefore leaves an explicit pending/failed record, never a silently
        // vanished opaque directory.
        let item_id = record_item(conn, operation_id, &dir_str, None, "remove_dir", "pending")?;
        let deletion = BoundDirectory::open_for_delete(&dir).and_then(BoundDirectory::delete_exact);
        match deletion {
            Ok(()) => {
                set_item_status(conn, item_id, "done")?;
                removed += 1;
            }
            Err(_) => {
                set_item_status(conn, item_id, "failed")?;
                failed += 1;
            }
        }
    }
    Ok((removed, failed))
}

fn is_protected_or_sensitive(path: &Path) -> bool {
    let text = path.to_string_lossy();
    hangar_protect::protected_level_for_path(&text).is_some()
        || hangar_protect::is_sensitive_path(&text)
        || hangar_protect::is_strong_protected_path(&text)
}

#[cfg(test)]
fn execute_move(
    from: &Path,
    to: &Path,
    strategy: MoveStrategy,
) -> Result<(ItemOutcome, u64, u64), QuarantineError> {
    match strategy {
        MoveStrategy::CopyDelete => {
            let mut source = BoundFile::open_for_move(from)?;
            let source_hash = source.hash()?;
            let bytes = source.stamp().bytes;
            let _held = bound_fs::copy_to_new(&mut source, to, &source_hash)?;
            // CRASH-CONSISTENCY: the source is NOT removed here. The caller removes it only after
            // the held copy is durably journaled (operation_item 'done' + quarantine_entry), so a
            // crash can never leave the source gone with the copy unrecorded (an unrecoverable
            // orphan). The verified copy is at `to`; the source still sits at `from`.
            Ok((ItemOutcome::Copied, bytes, bytes))
        }
    }
}

#[derive(Debug)]
struct SourceRemovalResult {
    recovered: u64,
    error: Option<String>,
}

#[cfg(test)]
fn finalize_source_removal(action: &str, source: &Path, recovered: u64) -> SourceRemovalResult {
    if action != "copy_delete" {
        return SourceRemovalResult {
            recovered,
            error: None,
        };
    }
    match fs::remove_file(crate::longpath::to_extended(source).as_ref()) {
        Ok(()) => SourceRemovalResult {
            recovered,
            error: None,
        },
        Err(error) => SourceRemovalResult {
            recovered: 0,
            error: Some(format!(
                "The held copy was verified, but the original could not be removed: {error}"
            )),
        },
    }
}

fn record_item(
    conn: &Connection,
    operation_id: i64,
    from_path: &str,
    to_path: Option<&str>,
    action: &str,
    status: &str,
) -> Result<i64, QuarantineError> {
    conn.execute(
        "INSERT INTO operation_item(operation_id, action, from_path, to_path, status)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![operation_id, action, from_path, to_path, status],
    )?;
    Ok(conn.last_insert_rowid())
}

fn record_bound_item(
    conn: &Connection,
    operation_id: i64,
    from_path: &str,
    to_path: &str,
    expected: &FileStamp,
    expected_hash: &str,
) -> Result<i64, QuarantineError> {
    conn.execute(
        "INSERT INTO operation_item(
            operation_id, action, from_path, to_path, bytes, checksum_before,
            expected_volume_id, expected_file_id, expected_blake3,
            expected_modified_unix_seconds, status
         ) VALUES(?1, 'move', ?2, ?3, ?4, ?5, ?6, ?7, ?5, ?8, 'pending')",
        params![
            operation_id,
            from_path,
            to_path,
            expected.bytes as i64,
            expected_hash,
            expected.volume_id,
            expected.file_id,
            expected.modified_unix_seconds,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn set_bound_item_done(
    conn: &Connection,
    item_id: i64,
    action: &str,
    result: &FileStamp,
    result_hash: &str,
) -> Result<(), QuarantineError> {
    conn.execute(
        "UPDATE operation_item
         SET action = ?2, checksum_after = ?3,
             result_volume_id = ?4, result_file_id = ?5, result_blake3 = ?3,
             result_modified_unix_seconds = ?6,
             status = 'done'
         WHERE id = ?1",
        params![
            item_id,
            action,
            result_hash,
            result.volume_id,
            result.file_id,
            result.modified_unix_seconds,
        ],
    )?;
    Ok(())
}

fn set_item_status(conn: &Connection, item_id: i64, status: &str) -> Result<(), QuarantineError> {
    conn.execute(
        "UPDATE operation_item SET status = ?1 WHERE id = ?2",
        params![status, item_id],
    )?;
    Ok(())
}

/// Build the `quarantine_entry.manifest_json` blob. Shared by the initial insert and the
/// source-removal-failure correction so both stay in lock-step (the manifest's
/// `space_recovered` never contradicts the row's column). `removal_error`, when set, is a
/// durable note that the held copy was verified but the original could not be removed —
/// both copies remain on disk — surfaced for Recover / audit.
#[allow(clippy::too_many_arguments)]
fn quarantine_manifest_json(
    original_path: &str,
    quarantine_path: &str,
    relative: &str,
    bytes: u64,
    recovered: u64,
    backup_hash: Option<&str>,
    removal_error: Option<&str>,
) -> String {
    let mut manifest = serde_json::json!({
        "original_path": original_path,
        "quarantine_path": quarantine_path,
        "relative": relative,
        "bytes": bytes,
        "space_recovered": recovered,
        // blake3 of the verified backup copy of this file; permanent delete
        // checks it against the backup manifest before unlinking (Gate 3).
        "backup_blake3": backup_hash,
    });
    if let Some(error) = removal_error {
        manifest["source_removal_error"] = serde_json::Value::String(error.to_string());
    }
    manifest.to_string()
}

#[allow(clippy::too_many_arguments)]
fn insert_quarantine_entry(
    conn: &Connection,
    operation_id: i64,
    original_path: &str,
    quarantine_path: &str,
    relative: &str,
    bytes: u64,
    recovered: u64,
    backup_id: i64,
    backup_hash: Option<&str>,
) -> Result<(), QuarantineError> {
    let manifest = quarantine_manifest_json(
        original_path,
        quarantine_path,
        relative,
        bytes,
        recovered,
        backup_hash,
        None,
    );
    // Store NULL (not 0) when there is no linked backup, both to satisfy the FK to
    // backup(id) and so permanent delete treats it as "no backup" (Gate 3 refuses).
    let backup_id = (backup_id > 0).then_some(backup_id);
    conn.execute(
        "INSERT INTO quarantine_entry(
            operation_id, original_path, quarantine_path, size, space_recovered,
            backup_id, status, manifest_json, removal_group_id,
            removal_group_fingerprint
         ) VALUES(
            ?1, ?2, ?3, ?4, ?5, ?6, 'quarantined', ?7, ?8,
            (SELECT target_fingerprint FROM operation WHERE id = ?1)
         )",
        params![
            operation_id,
            original_path,
            quarantine_path,
            bytes as i64,
            recovered as i64,
            backup_id,
            manifest,
            format!("operation:{operation_id}"),
        ],
    )?;
    Ok(())
}

/// Correct an already-durable quarantine_entry after the source unlink failed: zero the
/// recovered bytes (both copies still exist, nothing was freed) and record the removal error
/// in the manifest. Targets the entry by (operation_id, quarantine_path), which is unique
/// within an operation — holding paths are namespaced per operation and a colliding
/// destination is refused before an entry is ever inserted. This never removes the row, so
/// the held copy stays referenced and recoverable at all times.
#[allow(clippy::too_many_arguments)]
fn record_source_removal_failure(
    conn: &Connection,
    operation_id: i64,
    original_path: &str,
    quarantine_path: &str,
    relative: &str,
    bytes: u64,
    backup_hash: Option<&str>,
    detail: &str,
) -> Result<(), QuarantineError> {
    let manifest = quarantine_manifest_json(
        original_path,
        quarantine_path,
        relative,
        bytes,
        0,
        backup_hash,
        Some(detail),
    );
    conn.execute(
        "UPDATE quarantine_entry SET space_recovered = 0, manifest_json = ?3
         WHERE operation_id = ?1 AND quarantine_path = ?2",
        params![operation_id, quarantine_path, manifest],
    )?;
    Ok(())
}

fn skipped_entry(
    original_path: String,
    outcome: ItemOutcome,
    detail: &str,
) -> QuarantineEntryResult {
    QuarantineEntryResult {
        original_path,
        quarantine_path: None,
        outcome,
        bytes: 0,
        space_recovered: 0,
        detail: Some(detail.to_string()),
    }
}

/// Normalize a manifest-relative path into a holding-area suffix that can never
/// escape it. Only `Normal` components survive: `..`, absolute roots and drive
/// prefixes are dropped (a `PathBuf::join` with an absolute path would REPLACE
/// the holding root entirely). This is enforced here in the executor — not just
/// at the API caller — mirroring the backup engine's `safe_dest` defense in
/// depth. An input with no usable components falls back to a fixed name so the
/// destination stays inside the per-operation folder.
fn normalize_relative(relative: &str) -> PathBuf {
    let trimmed = relative.replace('\\', "/");
    let mut out = PathBuf::new();
    for component in Path::new(trimmed.trim_start_matches('/')).components() {
        if let std::path::Component::Normal(part) = component {
            out.push(part);
        }
    }
    if out.as_os_str().is_empty() {
        out.push("quarantined-item");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journaled_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        conn
    }

    fn reviewed_item(source: &Path, relative: &str) -> QuarantineItem {
        let (expected_source_stamp, backup_hash) =
            crate::inspect_local_mutation_file(source).unwrap();
        QuarantineItem {
            source: source.to_path_buf(),
            relative: relative.to_string(),
            expected_source_stamp,
            backup_hash,
        }
    }

    #[test]
    fn normalize_relative_never_escapes_the_holding_area() {
        // Traversal and absolute inputs must reduce to safe in-area suffixes.
        assert_eq!(
            normalize_relative("output/render.bin"),
            PathBuf::from("output/render.bin")
        );
        assert_eq!(
            normalize_relative("../../etc/passwd"),
            PathBuf::from("etc/passwd")
        );
        assert_eq!(
            normalize_relative(r"C:\Windows\System32\evil.dll"),
            PathBuf::from("Windows/System32/evil.dll")
        );
        assert_eq!(
            normalize_relative(r"\\server\share\x"),
            PathBuf::from("server/share/x")
        );
        // Nothing usable → fixed in-area name, never an empty join.
        assert_eq!(
            normalize_relative("../.."),
            PathBuf::from("quarantined-item")
        );
        assert_eq!(normalize_relative(""), PathBuf::from("quarantined-item"));
    }

    #[test]
    fn quarantine_moves_same_volume_and_journals() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        fs::create_dir_all(project.join("output")).unwrap();
        let target = project.join("output/render.bin");
        fs::write(&target, b"render payload").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![reviewed_item(&target, "output/render.bin")],
                plan_json: "{\"schema\":\"operation_plan/1\"}".to_string(),
                target_node_id: Some(7),
                target_fingerprint: Some("fp".to_string()),
                backup_id: 0,
                cleanup_root: None,
                include_protected: false,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(result.moved, 1);
        assert_eq!(result.failed, 0);
        // Same-volume rename frees no space yet.
        assert_eq!(result.space_recovered, 0);
        assert_eq!(result.entries[0].outcome, ItemOutcome::Moved);

        // Source moved out, copy present in quarantine (under the per-operation
        // namespace), content preserved.
        assert!(!target.exists());
        let moved = fs::read(
            quarantine_root
                .join(format!("op-{}", result.operation_id))
                .join("output/render.bin"),
        )
        .unwrap();
        assert_eq!(moved, b"render payload");

        // Journal: operation done, one done item, one quarantine_entry.
        let op_status: String = conn
            .query_row(
                "SELECT status FROM operation WHERE id = ?1",
                [result.operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(op_status, "done");
        let done_items: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM operation_item WHERE operation_id = ?1 AND status = 'done'",
                [result.operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(done_items, 1);
        let entries: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM quarantine_entry WHERE operation_id = ?1 AND status = 'quarantined'",
                [result.operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(entries, 1);
    }

    #[test]
    fn quarantine_rejects_a_same_bytes_leaf_replacement_before_journaling_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("project/output/render.bin");
        let displaced = dir.path().join("reviewed-original.bin");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"identical bytes").unwrap();
        let reviewed = reviewed_item(&target, "output/render.bin");

        // Keep the reviewed object alive under another name so the filesystem cannot
        // reuse its file id, then put byte-identical content at the authorised path.
        fs::rename(&target, &displaced).unwrap();
        fs::write(&target, b"identical bytes").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &dir.path().join("holding"),
                items: vec![reviewed],
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

        assert_eq!(result.moved, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(fs::read(&target).unwrap(), b"identical bytes");
        assert_eq!(fs::read(&displaced).unwrap(), b"identical bytes");
        let held_entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM quarantine_entry", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(held_entries, 0);
    }

    #[test]
    fn quarantine_rejects_changed_bytes_after_a_stamp_only_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("project/output/render.bin");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"local mutation fixture").unwrap();
        let mut reviewed = reviewed_item(&target, "output/render.bin");
        let reviewed_hash = reviewed.backup_hash.clone();

        // Keep the same object and byte length, then refresh only the expected
        // identity/size/mtime stamp. This deterministically reaches the hash check
        // regardless of filesystem timestamp granularity and proves that the same
        // bound handle, not a path-based precheck, rejects changed source bytes.
        fs::write(&target, b"LOCAL mutation fixture").unwrap();
        let (changed_stamp, changed_hash) = crate::inspect_local_mutation_file(&target).unwrap();
        assert_eq!(reviewed.expected_source_stamp.bytes, changed_stamp.bytes);
        assert_ne!(reviewed_hash, changed_hash);
        reviewed.expected_source_stamp = changed_stamp;

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &dir.path().join("holding"),
                items: vec![reviewed],
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

        assert_eq!(result.moved, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(fs::read(&target).unwrap(), b"LOCAL mutation fixture");
        let held_entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM quarantine_entry", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(held_entries, 0);
    }

    #[cfg(windows)]
    #[test]
    fn directory_cleanup_is_pending_before_handle_disposition_and_blocks_replacement() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("journal.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        let project = dir.path().join("project");
        let target = project.join("a/render.bin");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"render").unwrap();

        let observed = Arc::new(AtomicBool::new(false));
        let observed_hook = Arc::clone(&observed);
        let hook_db = db_path.clone();
        crate::bound_fs::set_test_hook(Some(Arc::new(move |point, path| {
            if point != crate::bound_fs::RacePoint::AfterDirectoryBoundBeforeDisposition
                || observed_hook.swap(true, Ordering::SeqCst)
            {
                return;
            }
            let audit = Connection::open(&hook_db).unwrap();
            let pending: i64 = audit
                .query_row(
                    "SELECT COUNT(*) FROM operation_item
                     WHERE action = 'remove_dir' AND from_path = ?1 AND status = 'pending'",
                    [path.to_string_lossy().to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                pending, 1,
                "directory disposition ran before durable intent"
            );

            let replacement = path.with_extension("raced-away");
            assert!(
                fs::rename(path, &replacement).is_err(),
                "the bound directory was replaceable before disposition"
            );
            assert!(!replacement.exists());
        })));

        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &dir.path().join("holding"),
                items: vec![reviewed_item(&target, "a/render.bin")],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: Some(project.clone()),
                include_protected: false,
                reparse_links: Vec::new(),
            },
        );
        crate::bound_fs::set_test_hook(None);
        let result = result.unwrap();

        assert!(observed.load(Ordering::SeqCst));
        assert_eq!(result.failed, 0);
        assert_eq!(result.removed_dirs, 2);
        assert!(!project.exists());
        let unfinished: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM operation_item
                 WHERE operation_id = ?1 AND action = 'remove_dir' AND status != 'done'",
                [result.operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unfinished, 0);
    }

    #[test]
    fn journal_records_non_verbatim_paths() {
        // The `\\?\` extended-length form is used ONLY at the fs-call boundary; the journal
        // must keep the ordinary user-visible path so a later restore targets the real
        // original location. Assert every recorded path (operation_item + quarantine_entry)
        // is non-verbatim, regardless of platform (the prefix is a Windows concern, but the
        // check is a cheap, portable guard against a verbatim alias ever leaking into the DB).
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        fs::create_dir_all(project.join("output")).unwrap();
        let target = project.join("output/render.bin");
        fs::write(&target, b"render payload").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![reviewed_item(&target, "output/render.bin")],
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
        assert_eq!(result.moved, 1);

        let paths: Vec<String> = {
            let mut out = Vec::new();
            let mut stmt = conn
                .prepare("SELECT from_path, to_path FROM operation_item WHERE operation_id = ?1")
                .unwrap();
            let rows = stmt
                .query_map([result.operation_id], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                })
                .unwrap();
            for row in rows {
                let (from, to) = row.unwrap();
                out.extend(from);
                out.extend(to);
            }
            let (orig, held): (String, String) = conn
                .query_row(
                    "SELECT original_path, quarantine_path FROM quarantine_entry WHERE operation_id = ?1",
                    [result.operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            out.push(orig);
            out.push(held);
            out
        };
        assert!(!paths.is_empty());
        for path in &paths {
            assert!(
                !path.starts_with(r"\\?\"),
                "journal stored a verbatim path (restore would target the wrong location): {path}"
            );
        }
        // The recorded held path is the real, restorable location on disk.
        let held = &paths[paths.len() - 1];
        assert!(std::path::Path::new(held).exists());
    }

    #[test]
    fn quarantine_refuses_a_holding_root_inside_the_target() {
        // When the move will recursively empty the target (cleanup_root = the project), a
        // holding root inside that tree would relocate the held copies into the very folder
        // being emptied. Refuse before journaling anything.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        // Holding folder sits INSIDE the project being emptied.
        let quarantine_root = project.join(".holding");
        fs::create_dir_all(project.join("output")).unwrap();
        let target = project.join("output/render.bin");
        fs::write(&target, b"render payload").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![reviewed_item(&target, "output/render.bin")],
                plan_json: "{\"schema\":\"operation_plan/1\"}".to_string(),
                target_node_id: Some(7),
                target_fingerprint: Some("fp".to_string()),
                backup_id: 0,
                cleanup_root: Some(project.clone()),
                include_protected: false,
                reparse_links: Vec::new(),
            },
        );

        assert!(matches!(
            result,
            Err(QuarantineError::HoldingInsideTarget { .. })
        ));
        // Refused before journaling: no operation row written, source untouched.
        let ops: i64 = conn
            .query_row("SELECT COUNT(*) FROM operation", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ops, 0);
        assert!(target.exists());
    }

    #[test]
    fn two_operations_sharing_a_relative_path_do_not_collide() {
        // Two projects each have a file at the same project-relative path; both are
        // quarantined into the SAME holding folder. The per-operation namespace must
        // keep the held copies distinct so the second move cannot destroy the first
        // entry's only held copy.
        let dir = tempfile::tempdir().unwrap();
        let holding = dir.path().join("holding");
        let conn = journaled_conn();

        let make = |name: &str, bytes: &[u8]| {
            let src = dir.path().join(name).join("output/render.bin");
            fs::create_dir_all(src.parent().unwrap()).unwrap();
            fs::write(&src, bytes).unwrap();
            src
        };
        let a = make("projectA", b"AAAA");
        let b = make("projectB", b"BBBB");

        let run = |src: &std::path::Path| {
            quarantine(
                &conn,
                QuarantineRequest {
                    quarantine_root: &holding,
                    items: vec![reviewed_item(src, "output/render.bin")],
                    plan_json: "{}".to_string(),
                    target_node_id: None,
                    target_fingerprint: None,
                    backup_id: 0,
                    cleanup_root: None,
                    include_protected: false,
                    reparse_links: Vec::new(),
                },
            )
            .unwrap()
        };
        let ra = run(&a);
        let rb = run(&b);

        let held = |op_id: i64| -> String {
            conn.query_row(
                "SELECT quarantine_path FROM quarantine_entry WHERE operation_id = ?1",
                [op_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        let pa = held(ra.operation_id);
        let pb = held(rb.operation_id);

        // Distinct holding paths, each with its own correct bytes — no overwrite.
        assert_ne!(pa, pb);
        assert_eq!(fs::read(&pa).unwrap(), b"AAAA");
        assert_eq!(fs::read(&pb).unwrap(), b"BBBB");
    }

    #[test]
    fn quarantine_refuses_any_reparse_request_before_journaling_or_moving_files() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("listed-reparse");
        fs::create_dir_all(&regular).unwrap();
        fs::write(regular.join("must-survive.txt"), b"data").unwrap();
        let source = dir.path().join("ordinary.bin");
        fs::write(&source, b"must not move").unwrap();
        let conn = journaled_conn();

        // Link removal is not faithfully reversible yet. Even with a valid ordinary
        // file in the same request, refuse at the function boundary: no path probe,
        // journal row, holding object, or ordinary-file move may happen first.
        let error = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &dir.path().join("holding"),
                items: vec![reviewed_item(&source, "ordinary.bin")],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: None,
                include_protected: true,
                reparse_links: vec![regular.clone()],
            },
        )
        .unwrap_err();

        assert!(matches!(error, QuarantineError::ReparseRemovalDisabled));
        assert_eq!(fs::read(&source).unwrap(), b"must not move");
        assert!(regular.join("must-survive.txt").exists());
        let operation_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM operation", [], |row| row.get(0))
            .unwrap();
        assert_eq!(operation_count, 0);
    }

    #[test]
    fn cross_volume_copy_delete_verifies_then_frees() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("asset.bin");
        let to = dir.path().join("q/asset.bin");
        fs::write(&from, b"cross volume payload").unwrap();

        // Force the copy_delete strategy (cross-volume behaviour) on one volume.
        let (outcome, bytes, recovered) =
            execute_move(&from, &to, MoveStrategy::CopyDelete).unwrap();

        assert_eq!(outcome, ItemOutcome::Copied);
        assert_eq!(recovered, bytes);
        // Crash-consistency: execute_move copies + verifies but does NOT delete the source — the
        // caller removes it only AFTER the held copy is journaled, so a crash never leaves the
        // source gone with the copy unrecorded. The verified copy is at `to`; the source remains.
        assert!(from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"cross volume payload");
    }

    #[test]
    fn source_unlink_controls_cross_volume_recovered_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let removable = dir.path().join("removable.bin");
        fs::write(&removable, b"1234567").unwrap();

        let removed = finalize_source_removal("copy_delete", &removable, 7);
        assert_eq!(removed.recovered, 7);
        assert!(removed.error.is_none());
        assert!(!removable.exists());

        let missing = dir.path().join("could-not-remove.bin");
        let failed = finalize_source_removal("copy_delete", &missing, 99);
        assert_eq!(failed.recovered, 0);
        assert!(failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("original could not be removed")));
    }

    #[test]
    fn protected_and_sensitive_paths_are_never_moved() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine_root = dir.path().join("quarantine");
        let secret = dir.path().join("credentials.json");
        fs::write(&secret, b"{\"token\":\"x\"}").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![reviewed_item(&secret, "credentials.json")],
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

        assert_eq!(result.moved, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.entries[0].outcome, ItemOutcome::SkippedProtected);
        // The sensitive file is untouched.
        assert!(secret.exists());
        assert_eq!(fs::read(&secret).unwrap(), b"{\"token\":\"x\"}");
    }

    #[test]
    fn recursive_cleanup_removes_empty_dirs_but_keeps_dirs_with_skipped_content() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        // A normal file deep in the tree (will move out), and a sensitive file
        // (skipped) whose directory must survive.
        let deep = project.join("a/b/render.bin");
        fs::create_dir_all(deep.parent().unwrap()).unwrap();
        fs::write(&deep, b"render").unwrap();
        let secret = project.join("config/credentials.json");
        fs::create_dir_all(secret.parent().unwrap()).unwrap();
        fs::write(&secret, b"{}").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![
                    reviewed_item(&deep, "a/b/render.bin"),
                    reviewed_item(&secret, "config/credentials.json"),
                ],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: Some(project.clone()),
                include_protected: false,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(result.moved, 1); // render moved
        assert_eq!(result.skipped, 1); // credentials skipped (sensitive)
        assert!(result.removed_dirs >= 2); // project/a/b and project/a emptied

        // The moved file's now-empty directories are gone.
        assert!(!project.join("a/b").exists());
        assert!(!project.join("a").exists());
        // The skipped sensitive file and its directory survive.
        assert!(secret.exists());
        assert!(project.join("config").exists());
        // The project root is NOT removed — it still holds config/.
        assert!(project.exists());
    }

    #[test]
    fn empty_completely_moves_sensitive_files_and_clears_the_folder() {
        // With include_protected = true (the explicit opt-in), the sensitive file is
        // moved out like any other file and the whole project folder is emptied.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        let deep = project.join("a/b/render.bin");
        fs::create_dir_all(deep.parent().unwrap()).unwrap();
        fs::write(&deep, b"render").unwrap();
        let secret = project.join("config/credentials.json");
        fs::create_dir_all(secret.parent().unwrap()).unwrap();
        fs::write(&secret, b"{}").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![
                    reviewed_item(&deep, "a/b/render.bin"),
                    reviewed_item(&secret, "config/credentials.json"),
                ],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: Some(project.clone()),
                include_protected: true,
                reparse_links: Vec::new(),
            },
        )
        .unwrap();

        // Both files moved (nothing skipped); the sensitive file is now in the holding
        // area, not in the project.
        assert_eq!(result.moved, 2);
        assert_eq!(result.skipped, 0);
        assert!(!secret.exists());
        assert!(!deep.exists());
        // The whole project folder is gone from disk.
        assert!(!project.exists());
    }

    fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
        if !dir.exists() {
            return;
        }
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn held_copies_are_always_journaled_no_orphans() {
        // CRASH-CONSISTENCY invariant: no held copy may sit in the holding area without a
        // quarantine_entry row referencing it — Recover reads quarantine_entry only, so an
        // un-journaled held file is an invisible orphan. Quarantine must journal an entry for
        // every file it relocates, and every entry must point at a file that exists on disk.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        let a = project.join("a/one.bin");
        let b = project.join("b/two.bin");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, b"AAAA").unwrap();
        fs::write(&b, b"BBBB").unwrap();

        let conn = journaled_conn();
        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![
                    reviewed_item(&a, "a/one.bin"),
                    reviewed_item(&b, "b/two.bin"),
                ],
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
        assert_eq!(result.moved, 2);
        assert_eq!(result.failed, 0);

        // Every quarantine_entry references a held copy that exists on disk.
        let mut entry_paths: Vec<PathBuf> = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT quarantine_path FROM quarantine_entry WHERE operation_id = ?1")
                .unwrap();
            let rows = stmt
                .query_map([result.operation_id], |row| row.get::<_, String>(0))
                .unwrap();
            for row in rows {
                let path = PathBuf::from(row.unwrap());
                assert!(
                    path.exists(),
                    "entry references a missing held copy: {path:?}"
                );
                entry_paths.push(path);
            }
        }
        assert_eq!(entry_paths.len(), 2);

        // Conversely, every file physically in the holding area is referenced by an entry —
        // no orphaned held copy without a DB row.
        let mut held_files = Vec::new();
        collect_files(&quarantine_root, &mut held_files);
        assert_eq!(held_files.len(), 2, "unexpected held-file count");
        for held in &held_files {
            assert!(
                entry_paths.iter().any(|p| p == held),
                "held file has no quarantine_entry (orphan): {held:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn source_removal_failure_keeps_both_copies_and_journals_zero_recovered() {
        // Reproduce the cross-volume Ok-arm failure branch deterministically: execute_move
        // writes + verifies the held copy WITHOUT unlinking the source; quarantine() journals
        // the entry FIRST, then the source unlink fails. Both copies must remain, and the entry
        // must be present with space_recovered corrected to 0 (nothing was actually freed).
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("project/asset.bin");
        let dest = dir.path().join("holding/op-1/asset.bin");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"verified payload").unwrap();

        // Force the copy_delete path directly (as cross_volume_copy_delete_verifies_then_frees
        // does): the held copy is written + verified and the source is left in place.
        let (outcome, bytes, recovered) =
            execute_move(&source, &dest, MoveStrategy::CopyDelete).unwrap();
        assert_eq!(outcome, ItemOutcome::Copied);
        assert!(
            source.exists() && dest.exists(),
            "execute_move must not unlink the source"
        );

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('quarantine', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        // Journal the held copy FIRST with the optimistic recovered=bytes, mirroring the order
        // quarantine() now uses (insert before unlink).
        insert_quarantine_entry(
            &conn,
            op_id,
            &source.to_string_lossy(),
            &dest.to_string_lossy(),
            "asset.bin",
            bytes,
            recovered,
            0,
            None,
        )
        .unwrap();

        // Hold an exclusive handle (share_mode 0) so DeleteFileW hits a sharing violation and
        // the unlink fails while the source stays on disk (the same lock t04 uses to block a
        // rename). Modern std clears the read-only attribute before deleting, so an exclusive
        // handle — not a read-only bit — is the reliable way to force the failure.
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&source)
            .unwrap();

        let removal = finalize_source_removal("copy_delete", &source, recovered);
        assert!(removal.error.is_some(), "locked source unlink must fail");
        assert_eq!(removal.recovered, 0);
        record_source_removal_failure(
            &conn,
            op_id,
            &source.to_string_lossy(),
            &dest.to_string_lossy(),
            "asset.bin",
            bytes,
            None,
            removal.error.as_deref().unwrap(),
        )
        .unwrap();

        // Both copies remain; the entry is present with zeroed recovery and the failure note.
        assert!(source.exists(), "source must survive a failed unlink");
        assert!(dest.exists(), "held copy must survive");
        let (status, space, manifest): (String, i64, String) = conn
            .query_row(
                "SELECT status, space_recovered, manifest_json FROM quarantine_entry WHERE operation_id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "quarantined");
        assert_eq!(space, 0, "recovered bytes must be corrected to 0");
        assert!(manifest.contains("source_removal_error"));

        drop(lock);
    }

    #[test]
    fn a_failed_entry_insert_never_loses_the_bytes() {
        // Ordering guarantee: if the quarantine_entry INSERT fails mid-operation, quarantine()
        // returns Err BEFORE the source is unlinked (insert precedes finalize_source_removal),
        // so the bytes are never lost — they remain at the source (cross-volume: not yet
        // unlinked) or as the single held copy (same-volume rename already relocated them). The
        // operation stays 'executing' for recovery. A failed entry insert can never destroy the
        // only copy.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let quarantine_root = dir.path().join("quarantine");
        let src = project.join("payload.bin");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"irreplaceable").unwrap();

        let conn = journaled_conn();
        // Drop the entry table so insert_quarantine_entry fails AFTER the move + 'done' update.
        conn.execute("DROP TABLE quarantine_entry", []).unwrap();

        let result = quarantine(
            &conn,
            QuarantineRequest {
                quarantine_root: &quarantine_root,
                items: vec![reviewed_item(&src, "payload.bin")],
                plan_json: "{}".to_string(),
                target_node_id: None,
                target_fingerprint: None,
                backup_id: 0,
                cleanup_root: None,
                include_protected: false,
                reparse_links: Vec::new(),
            },
        );
        assert!(
            result.is_err(),
            "a failed entry insert must surface as an error"
        );

        // The bytes survive somewhere recoverable: at the source, or as the single held copy.
        let mut held_files = Vec::new();
        collect_files(&quarantine_root, &mut held_files);
        let held_survived = held_files
            .iter()
            .any(|p| fs::read(p).map(|b| b == b"irreplaceable").unwrap_or(false));
        let source_survived = src.exists() && fs::read(&src).unwrap() == b"irreplaceable";
        assert!(
            source_survived || held_survived,
            "the only copy of the bytes was lost on a failed entry insert"
        );

        // The operation was never finalized, so recovery can still reconcile it.
        let op_status: String = conn
            .query_row(
                "SELECT status FROM operation ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(op_status, "executing");
    }
}
