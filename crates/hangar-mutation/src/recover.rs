//! Crash recovery — the payoff of journal-first execution.
//!
//! On launch, operations left mid-flight (`executing` / `backup_running` /
//! `verifying`) are reconciled only from durable file identity and content proof.
//! A proved completed move can be reversed; ambiguous, legacy path-only, or
//! result-uncommitted states remain visible and mutation-blocking. If both the
//! original and held paths exist, recovery never guesses which copy should win:
//! it preserves both. Resuming pending items is a possible future option;
//! conservative, identity-bound rollback is the safe default.
//!
//! A permanent delete interrupted mid-unlink is reconciled separately: the entry
//! was flipped to `deleting` before the unlink, so the on-disk presence of the
//! held copy, checked against its delete proof, can return to `quarantined` when
//! still exact. Absence settles as `permanently_deleted` only when no durable
//! `unprovedFinalProfile` marker remains and, for guardian-mediated rows, a
//! proved close authorization was committed. Earlier/cancelled guardian states
//! keep absence visible and ambiguous.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use crate::bound_fs::{self, BoundFile, BoundObjectProof, CommittedObjectArchive, FileStamp};
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

const RESTORE_ZERO_ITEM_MANUAL: &str =
    "Legacy interrupted restore had no committed journal item, so no filesystem mutation could be proven; it was closed without claiming rollback and must be retried manually.";

const RESTORE_RESULT_IDENTITY_MISSING: &str =
    "Recovery found a restore destination, but the committed pending item has no durable result identity; all copies were preserved for manual review.";

/// On-disk presence check that first rejects non-local/unsafe journal paths, uses
/// extended-length form, and distinguishes definitive absence from IO ambiguity.
fn path_present(path: &Path) -> std::io::Result<bool> {
    bound_fs::validate_local_mutation_path(path)?;
    match std::fs::symlink_metadata(to_extended(path).as_ref()) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Prove a path is ABSENT, failing CLOSED on any ambiguity. This returns `true`
/// ONLY for a definitive `NotFound`; a present file, a
/// permission error, an unreachable volume, or any other error all count as "not provably
/// absent". Use it wherever a terminal, data-hiding claim (marking a held copy 'restored'
/// or 'permanently_deleted') hinges on the held copy being gone: never declare a
/// still-present-but-unreadable copy "gone" and hide the user's only good data.
fn path_provably_absent(path: &Path) -> std::io::Result<bool> {
    bound_fs::validate_local_mutation_path(path)?;
    match std::fs::symlink_metadata(to_extended(path).as_ref()) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
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
    bytes: u64,
) -> Result<(), RecoveryError> {
    let bytes = bytes.min(i64::MAX as u64) as i64;
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

#[derive(Debug)]
struct RecoveryItem {
    id: i64,
    from_path: String,
    to_path: String,
    expected: Option<FileStamp>,
    expected_hash: Option<String>,
    result: Option<FileStamp>,
    result_hash: Option<String>,
}

#[derive(Debug)]
struct RestoreJournalProof {
    expected: Option<FileStamp>,
    expected_hash: Option<String>,
    result: Option<FileStamp>,
    result_hash: Option<String>,
}

fn stamp_from_columns(
    volume_id: Option<String>,
    file_id: Option<String>,
    bytes: Option<i64>,
) -> Option<FileStamp> {
    Some(FileStamp {
        volume_id: volume_id?,
        file_id: file_id?,
        bytes: bytes?.max(0) as u64,
        modified_unix_seconds: None,
    })
}

enum BoundRecovery {
    RolledBack,
    BothCopies { bytes: u64 },
    Unresolved(String),
}

#[derive(Debug)]
struct BatchDeleteRecoveryItem {
    batch_item_id: i64,
    operation_item_id: i64,
    entry_id: i64,
    held_path: String,
    expected_stamp: Option<FileStamp>,
    expected_hash: Option<String>,
    item_status: String,
    entry_status: String,
    archive_id: Option<i64>,
    archive_path: Option<String>,
    archive_stamp: Option<FileStamp>,
    archive_hash: Option<String>,
    raw_backup_hash: Option<String>,
    semantic_hash: Option<String>,
    roundtrip_hash: Option<String>,
    stream_count: Option<i64>,
    security_stream_present: bool,
    cleanup_complete: bool,
    proof_schema: Option<String>,
    archive_status: Option<String>,
    logical_bytes: u64,
    held_volume_id: Option<String>,
    item_phase: String,
    backup_id: Option<i64>,
    original_path: String,
    removal_group_id: String,
    backup_destination: Option<String>,
    elevation_capability_id: Option<i64>,
    transport_nonce: Option<String>,
    nonce_digest: Option<String>,
    capability_index: Option<i64>,
    archive_partial_path: Option<String>,
    archive_initial_stamp: Option<FileStamp>,
    archive_final_path: Option<String>,
    pending_archive_stamp: Option<FileStamp>,
    pending_archive_hash: Option<String>,
    pending_raw_backup_hash: Option<String>,
    pending_semantic_hash: Option<String>,
    pending_roundtrip_hash: Option<String>,
    pending_stream_count: Option<i64>,
    pending_security_stream_present: Option<i64>,
    pending_cleanup_complete: Option<i64>,
    pending_proof_schema: Option<String>,
    item_reason_code: Option<String>,
    guardian_operation_id: Option<i64>,
    guardian_nonce_digest: Option<String>,
    guardian_expected_volume_id: Option<String>,
    guardian_expected_file_id: Option<String>,
    guardian_expected_bytes: Option<i64>,
    guardian_expected_modified_unix_seconds: Option<i64>,
    guardian_pid: Option<i64>,
    guardian_started_100ns: Option<i64>,
    guardian_image_sha256: Option<String>,
    guardian_receipt_path: Option<String>,
    guardian_receipt_volume_id: Option<String>,
    guardian_receipt_file_id: Option<String>,
    guardian_receipt_key_dpapi: Option<String>,
    guardian_receipt_cleanup_complete: Option<i64>,
    guardian_state: Option<String>,
    guardian_disposition_mode: Option<String>,
    guardian_error: Option<String>,
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn batch_archive_is_bound(item: &BatchDeleteRecoveryItem) -> Result<bool, String> {
    let (
        Some(archive_id),
        Some(path),
        Some(stamp),
        Some(hash),
        Some(raw),
        Some(semantic),
        Some(roundtrip),
    ) = (
        item.archive_id,
        item.archive_path.as_deref(),
        item.archive_stamp.as_ref(),
        item.archive_hash.as_deref(),
        item.raw_backup_hash.as_deref(),
        item.semantic_hash.as_deref(),
        item.roundtrip_hash.as_deref(),
    )
    else {
        return Ok(false);
    };
    if archive_id <= 0
        || item.proof_schema.as_deref() != Some(crate::OBJECT_ARCHIVE_PROOF_SCHEMA)
        || item.archive_status.as_deref() != Some("ready")
        || !item.security_stream_present
        || !item.cleanup_complete
        || item
            .stream_count
            .is_none_or(|count| !(1..=4_096).contains(&count))
        || !is_hex_64(hash)
        || !is_hex_64(raw)
        || !is_hex_64(semantic)
        || semantic != roundtrip
    {
        return Ok(false);
    }
    let path = Path::new(path);
    bound_fs::validate_local_mutation_path(path)
        .map_err(|error| format!("unsafe object_archive/2 recovery path: {error}"))?;
    CommittedObjectArchive::open_existing(path, stamp, hash)
        .map(|_| true)
        .map_err(|error| format!("object_archive/2 recovery payload could not be rebound: {error}"))
}

enum HeldRecoveryTruth {
    ExactPresent,
    ProvablyAbsent,
    Ambiguous(String),
}

fn held_recovery_truth(item: &BatchDeleteRecoveryItem) -> HeldRecoveryTruth {
    let (Some(stamp), Some(hash)) = (item.expected_stamp.as_ref(), item.expected_hash.as_deref())
    else {
        return HeldRecoveryTruth::Ambiguous(
            "batch delete item has no complete held identity/hash proof".to_string(),
        );
    };
    if !is_hex_64(hash) || stamp.modified_unix_seconds.is_none() {
        return HeldRecoveryTruth::Ambiguous(
            "batch delete item has an invalid held identity/hash/mtime proof".to_string(),
        );
    }
    let path = Path::new(&item.held_path);
    let original_error = match BoundObjectProof::open_for_archive(path, stamp, hash) {
        Ok(_) => return HeldRecoveryTruth::ExactPresent,
        Err(error) => error,
    };

    // A containing directory can legitimately acquire a new live stamp after
    // this immutable batch deletes its planned children. The exact guardian
    // binding captures that post-child stamp in the same transaction as delete
    // intent. It is safe to use only as an additional *presence* authority:
    // success rolls back and preserves bytes, while absence still requires the
    // separately authenticated durable close receipt below.
    if guardian_journal_is_present(item) {
        let guardian_stamp = match (
            item.guardian_expected_volume_id.as_deref(),
            item.guardian_expected_file_id.as_deref(),
            item.guardian_expected_bytes,
            item.guardian_expected_modified_unix_seconds,
        ) {
            (Some(volume_id), Some(file_id), Some(bytes), Some(modified))
                if volume_id == stamp.volume_id && file_id == stamp.file_id =>
            {
                match u64::try_from(bytes) {
                    Ok(bytes) => Some(FileStamp {
                        volume_id: volume_id.to_string(),
                        file_id: file_id.to_string(),
                        bytes,
                        modified_unix_seconds: Some(modified),
                    }),
                    Err(_) => {
                        return HeldRecoveryTruth::Ambiguous(
                            "guardian live target length is outside the Windows range".to_string(),
                        );
                    }
                }
            }
            (Some(_), Some(_), Some(_), Some(_)) => {
                return HeldRecoveryTruth::Ambiguous(
                    "guardian live target identity differs from the batch FileId authority"
                        .to_string(),
                );
            }
            _ => {
                return HeldRecoveryTruth::Ambiguous(
                    "guardian live target presence authority is incomplete".to_string(),
                );
            }
        };
        if let Some(guardian_stamp) = guardian_stamp.filter(|candidate| candidate != stamp) {
            match BoundObjectProof::open_for_archive(path, &guardian_stamp, hash) {
                Ok(_) => return HeldRecoveryTruth::ExactPresent,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return HeldRecoveryTruth::Ambiguous(format!(
                        "held object matches neither the archived nor guardian-bound live stamp: {error}"
                    ));
                }
            }
        }
    }

    if original_error.kind() == std::io::ErrorKind::NotFound {
        match path_provably_absent(path) {
            Ok(true) => HeldRecoveryTruth::ProvablyAbsent,
            Ok(false) => HeldRecoveryTruth::Ambiguous(
                "held path reappeared while recovery was proving absence".to_string(),
            ),
            Err(error) => HeldRecoveryTruth::Ambiguous(format!(
                "held absence could not be proven safely: {error}"
            )),
        }
    } else {
        HeldRecoveryTruth::Ambiguous(format!(
            "held object no longer matches its exact identity/hash proof: {original_error}"
        ))
    }
}

/// Returns true only when the guardian durably MAC-authenticated its exact
/// close receipt before closing the duplicate and that exact guardian process
/// is now proved dead. A pipe ACK accelerates the parent path but is not the
/// durable authority: receipt loss/tamper and live/unknown process identity
/// always remain fail-closed.
fn guardian_close_was_authorized(
    item: &BatchDeleteRecoveryItem,
    operation_id: i64,
) -> Result<bool, String> {
    if !guardian_journal_is_present(item) {
        return Ok(false);
    }
    let (
        Some(bound_operation_id),
        Some(volume_id),
        Some(file_id),
        Some(target_bytes),
        Some(target_modified_unix_seconds),
        Some(guardian_pid),
        Some(guardian_started_100ns),
        Some(guardian_image_sha256),
        Some(state),
    ) = (
        item.guardian_operation_id,
        item.guardian_expected_volume_id.as_deref(),
        item.guardian_expected_file_id.as_deref(),
        item.guardian_expected_bytes,
        item.guardian_expected_modified_unix_seconds,
        item.guardian_pid,
        item.guardian_started_100ns,
        item.guardian_image_sha256.as_deref(),
        item.guardian_state.as_deref(),
    )
    else {
        return Err("final-disposition guardian journal is only partially bound".to_string());
    };
    let expected = item
        .expected_stamp
        .as_ref()
        .ok_or_else(|| "guardian journal has no batch-item identity to compare".to_string())?;
    if bound_operation_id != operation_id
        || volume_id != expected.volume_id
        || file_id != expected.file_id
    {
        return Err(
            "final-disposition guardian journal does not match the operation/FileId authority"
                .to_string(),
        );
    }
    match state {
        "close_authorized" | "guardian_handle_closed" | "parent_handle_closed" => {
            let mode_label = item
                .guardian_disposition_mode
                .as_deref()
                .ok_or_else(|| format!("guardian close state {state} has no disposition mode"))?;
            #[cfg(windows)]
            {
                let mode =
                    crate::bound_fs::WindowsDeleteDispositionMode::from_journal_label(mode_label)
                        .ok_or_else(|| {
                        format!("guardian close state {state} has invalid disposition mode")
                    })?;
                let (
                    Some(receipt_path),
                    Some(receipt_volume_id),
                    Some(receipt_file_id),
                    Some(protected_key_hex),
                    Some(0),
                ) = (
                    item.guardian_receipt_path.as_deref(),
                    item.guardian_receipt_volume_id.as_deref(),
                    item.guardian_receipt_file_id.as_deref(),
                    item.guardian_receipt_key_dpapi.as_deref(),
                    item.guardian_receipt_cleanup_complete,
                )
                else {
                    return Err(format!(
                        "guardian close state {state} has no complete, uncleaned durable receipt authority"
                    ));
                };
                let pid = u32::try_from(guardian_pid).map_err(|_| {
                    "guardian journal PID is outside the Windows process-id range".to_string()
                })?;
                let started = u64::try_from(guardian_started_100ns).map_err(|_| {
                    "guardian journal start time is outside the Windows FILETIME range".to_string()
                })?;
                let target_bytes = u64::try_from(target_bytes).map_err(|_| {
                    "guardian journal target length is outside the Windows range".to_string()
                })?;
                match crate::elevated_transport::exact_guardian_liveness(
                    pid,
                    started,
                    guardian_image_sha256,
                ) {
                    Ok(crate::elevated_transport::ExactGuardianLiveness::Alive) => {
                        return Ok(false);
                    }
                    Ok(crate::elevated_transport::ExactGuardianLiveness::Terminated) => {}
                    Err(error) => {
                        return Err(format!(
                            "exact guardian process termination could not be proved: {error}"
                        ));
                    }
                }
                let receipt_authority = crate::elevated_transport::GuardianReceiptAuthority {
                    path: PathBuf::from(receipt_path),
                    initial_stamp: FileStamp {
                        volume_id: receipt_volume_id.to_string(),
                        file_id: receipt_file_id.to_string(),
                        bytes: 0,
                        modified_unix_seconds: None,
                    },
                    protected_key_hex: protected_key_hex.to_string(),
                };
                let receipt_expected = crate::elevated_transport::GuardianCloseReceiptExpectation {
                    operation_id,
                    batch_item_id: item.batch_item_id,
                    nonce_digest: item
                        .guardian_nonce_digest
                        .as_ref()
                        .ok_or_else(|| {
                            "guardian journal has no nonce digest for receipt authentication"
                                .to_string()
                        })?
                        .clone(),
                    guardian_pid: pid,
                    guardian_started_100ns: started,
                    guardian_image_sha256: guardian_image_sha256.to_string(),
                    target_stamp: FileStamp {
                        volume_id: volume_id.to_string(),
                        file_id: file_id.to_string(),
                        bytes: target_bytes,
                        modified_unix_seconds: Some(target_modified_unix_seconds),
                    },
                    disposition_mode: mode,
                };
                crate::elevated_transport::verify_guardian_close_receipt(
                    &receipt_authority,
                    &receipt_expected,
                )
                .map_err(|error| format!("guardian durable close receipt is invalid: {error}"))?;

                Ok(true)
            }
            #[cfg(not(windows))]
            {
                let _ = (
                    mode_label,
                    guardian_pid,
                    guardian_started_100ns,
                    guardian_image_sha256,
                );
                Ok(false)
            }
        }
        "handle_bound"
        | "arm_authorized_unproved"
        | "armed_unproved"
        | "final_profile_proved_held"
        | "cancelled_safe"
        | "cancellation_pending_retained" => Ok(false),
        _ => Err(format!("guardian journal has unknown state {state}")),
    }
}

fn guardian_journal_is_present(item: &BatchDeleteRecoveryItem) -> bool {
    item.guardian_operation_id.is_some()
        || item.guardian_nonce_digest.is_some()
        || item.guardian_expected_volume_id.is_some()
        || item.guardian_expected_file_id.is_some()
        || item.guardian_expected_bytes.is_some()
        || item.guardian_expected_modified_unix_seconds.is_some()
        || item.guardian_pid.is_some()
        || item.guardian_started_100ns.is_some()
        || item.guardian_image_sha256.is_some()
        || item.guardian_receipt_path.is_some()
        || item.guardian_receipt_volume_id.is_some()
        || item.guardian_receipt_file_id.is_some()
        || item.guardian_receipt_key_dpapi.is_some()
        || item.guardian_receipt_cleanup_complete.is_some()
        || item.guardian_state.is_some()
        || item.guardian_disposition_mode.is_some()
        || item.guardian_error.is_some()
}

fn load_batch_delete_recovery_items(
    conn: &Connection,
    batch_id: i64,
) -> Result<Vec<BatchDeleteRecoveryItem>, RecoveryError> {
    let mut statement = conn.prepare(
        "SELECT bi.id, bi.operation_item_id, bi.quarantine_entry_id, bi.held_path,
                bi.expected_volume_id, bi.expected_file_id, bi.expected_bytes,
                bi.expected_modified_unix_seconds, bi.expected_content_blake3,
                bi.status, qe.status, bi.archive_id,
                oa.archive_path, oa.archive_volume_id, oa.archive_file_id,
                oa.archive_bytes, oa.archive_modified_unix_seconds,
                oa.archive_blake3, oa.raw_backup_blake3, oa.semantic_blake3,
                oa.roundtrip_blake3, oa.stream_count,
                COALESCE(oa.security_stream_present, 0),
                COALESCE(oa.cleanup_complete, 0), oa.proof_schema, oa.status,
                bi.logical_bytes, bi.expected_volume_id, bi.phase,
                qe.backup_id, qe.original_path, bi.removal_group_id,
                b.destination, bi.elevation_capability_id,
                ec.transport_nonce, ec.nonce_digest, bi.capability_index,
                bi.archive_partial_path,
                bi.archive_initial_volume_id, bi.archive_initial_file_id,
                bi.archive_initial_bytes, bi.archive_initial_modified_unix_seconds,
                bi.archive_final_path,
                bi.archive_proof_volume_id, bi.archive_proof_file_id,
                bi.archive_proof_bytes, bi.archive_proof_modified_unix_seconds,
                bi.archive_proof_blake3, bi.archive_raw_backup_blake3,
                bi.archive_semantic_blake3, bi.archive_roundtrip_blake3,
                bi.archive_stream_count, bi.archive_security_stream_present,
                bi.archive_cleanup_complete, bi.archive_proof_schema,
                bi.reason_code, fdg.operation_id, fdg.nonce_digest, fdg.expected_volume_id,
                fdg.expected_file_id, fdg.expected_bytes,
                fdg.expected_modified_unix_seconds,
                fdg.guardian_pid, fdg.guardian_started_100ns,
                fdg.guardian_image_sha256, fdg.receipt_path, fdg.receipt_volume_id,
                fdg.receipt_file_id, fdg.receipt_key_dpapi,
                fdg.receipt_cleanup_complete, fdg.state, fdg.disposition_mode, fdg.error
         FROM permanent_delete_batch_item bi
         JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
         LEFT JOIN backup b ON b.id = qe.backup_id
         LEFT JOIN elevation_capability ec
           ON ec.id = bi.elevation_capability_id
         LEFT JOIN object_archive oa ON oa.id = bi.archive_id
         LEFT JOIN final_disposition_guardian fdg ON fdg.batch_item_id = bi.id
         WHERE bi.batch_id = ?1
         ORDER BY bi.id",
    )?;
    let rows = statement.query_map([batch_id], |row| {
        Ok(BatchDeleteRecoveryItem {
            batch_item_id: row.get(0)?,
            operation_item_id: row.get(1)?,
            entry_id: row.get(2)?,
            held_path: row.get(3)?,
            expected_stamp: Some(FileStamp {
                volume_id: row.get(4)?,
                file_id: row.get(5)?,
                bytes: row.get::<_, i64>(6)?.max(0) as u64,
                modified_unix_seconds: row.get(7)?,
            }),
            expected_hash: row.get(8)?,
            item_status: row.get(9)?,
            entry_status: row.get(10)?,
            archive_id: row.get(11)?,
            archive_path: row.get(12)?,
            archive_stamp: match (
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<i64>>(15)?,
            ) {
                (Some(volume_id), Some(file_id), Some(bytes)) => Some(FileStamp {
                    volume_id,
                    file_id,
                    bytes: bytes.max(0) as u64,
                    modified_unix_seconds: row.get(16)?,
                }),
                _ => None,
            },
            archive_hash: row.get(17)?,
            raw_backup_hash: row.get(18)?,
            semantic_hash: row.get(19)?,
            roundtrip_hash: row.get(20)?,
            stream_count: row.get(21)?,
            security_stream_present: row.get::<_, i64>(22)? == 1,
            cleanup_complete: row.get::<_, i64>(23)? == 1,
            proof_schema: row.get(24)?,
            archive_status: row.get(25)?,
            logical_bytes: row.get::<_, i64>(26)?.max(0) as u64,
            held_volume_id: row.get(27)?,
            item_phase: row.get(28)?,
            backup_id: row.get(29)?,
            original_path: row.get(30)?,
            removal_group_id: row.get(31)?,
            backup_destination: row.get(32)?,
            elevation_capability_id: row.get(33)?,
            transport_nonce: row.get(34)?,
            nonce_digest: row.get(35)?,
            capability_index: row.get(36)?,
            archive_partial_path: row.get(37)?,
            archive_initial_stamp: match (
                row.get::<_, Option<String>>(38)?,
                row.get::<_, Option<String>>(39)?,
                row.get::<_, Option<i64>>(40)?,
            ) {
                (Some(volume_id), Some(file_id), Some(bytes)) => Some(FileStamp {
                    volume_id,
                    file_id,
                    bytes: bytes.max(0) as u64,
                    modified_unix_seconds: row.get(41)?,
                }),
                _ => None,
            },
            archive_final_path: row.get(42)?,
            pending_archive_stamp: match (
                row.get::<_, Option<String>>(43)?,
                row.get::<_, Option<String>>(44)?,
                row.get::<_, Option<i64>>(45)?,
            ) {
                (Some(volume_id), Some(file_id), Some(bytes)) => Some(FileStamp {
                    volume_id,
                    file_id,
                    bytes: bytes.max(0) as u64,
                    modified_unix_seconds: row.get(46)?,
                }),
                _ => None,
            },
            pending_archive_hash: row.get(47)?,
            pending_raw_backup_hash: row.get(48)?,
            pending_semantic_hash: row.get(49)?,
            pending_roundtrip_hash: row.get(50)?,
            pending_stream_count: row.get(51)?,
            pending_security_stream_present: row.get(52)?,
            pending_cleanup_complete: row.get(53)?,
            pending_proof_schema: row.get(54)?,
            item_reason_code: row.get(55)?,
            guardian_operation_id: row.get(56)?,
            guardian_nonce_digest: row.get(57)?,
            guardian_expected_volume_id: row.get(58)?,
            guardian_expected_file_id: row.get(59)?,
            guardian_expected_bytes: row.get(60)?,
            guardian_expected_modified_unix_seconds: row.get(61)?,
            guardian_pid: row.get(62)?,
            guardian_started_100ns: row.get(63)?,
            guardian_image_sha256: row.get(64)?,
            guardian_receipt_path: row.get(65)?,
            guardian_receipt_volume_id: row.get(66)?,
            guardian_receipt_file_id: row.get(67)?,
            guardian_receipt_key_dpapi: row.get(68)?,
            guardian_receipt_cleanup_complete: row.get(69)?,
            guardian_state: row.get(70)?,
            guardian_disposition_mode: row.get(71)?,
            guardian_error: row.get(72)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Debug, Clone)]
struct PendingArchiveProof {
    stamp: FileStamp,
    hash: String,
    raw_backup_hash: String,
    semantic_hash: String,
    roundtrip_hash: String,
    stream_count: i64,
}

struct ExistingArchiveRecoveryRow {
    id: i64,
    path: String,
    volume_id: String,
    file_id: String,
    bytes: i64,
    modified_unix_seconds: Option<i64>,
    archive_hash: String,
    raw_backup_hash: String,
    semantic_hash: String,
    roundtrip_hash: String,
    stream_count: i64,
    security_stream_present: i64,
    cleanup_complete: i64,
    proof_schema: String,
    status: String,
}

enum ExactPartialTruth {
    Absent,
    Exact(BoundFile),
    Ambiguous(String),
}

enum ExactFinalTruth {
    Absent,
    Exact(Box<CommittedObjectArchive>),
    Ambiguous(String),
}

enum PendingArchiveRecovery {
    NoIntent,
    SafeToBlock,
    Ambiguous(String),
}

fn pending_archive_proof(
    item: &BatchDeleteRecoveryItem,
) -> Result<Option<PendingArchiveProof>, String> {
    let any = item.pending_archive_stamp.is_some()
        || item.pending_archive_hash.is_some()
        || item.pending_raw_backup_hash.is_some()
        || item.pending_semantic_hash.is_some()
        || item.pending_roundtrip_hash.is_some()
        || item.pending_stream_count.is_some()
        || item.pending_security_stream_present.is_some()
        || item.pending_cleanup_complete.is_some()
        || item.pending_proof_schema.is_some();
    if !any {
        return Ok(None);
    }
    let (
        Some(stamp),
        Some(hash),
        Some(raw_backup_hash),
        Some(semantic_hash),
        Some(roundtrip_hash),
        Some(stream_count),
        Some(security_stream_present),
        Some(cleanup_complete),
        Some(proof_schema),
    ) = (
        item.pending_archive_stamp.clone(),
        item.pending_archive_hash.clone(),
        item.pending_raw_backup_hash.clone(),
        item.pending_semantic_hash.clone(),
        item.pending_roundtrip_hash.clone(),
        item.pending_stream_count,
        item.pending_security_stream_present,
        item.pending_cleanup_complete,
        item.pending_proof_schema.as_deref(),
    )
    else {
        return Err("pending archive proof is only partially journaled".to_string());
    };
    if stamp.volume_id.is_empty()
        || stamp.file_id.is_empty()
        || stamp.bytes == 0
        || stamp.modified_unix_seconds.is_none()
        || !is_hex_64(&hash)
        || !is_hex_64(&raw_backup_hash)
        || !is_hex_64(&semantic_hash)
        || semantic_hash != roundtrip_hash
        || !(1..=4_096).contains(&stream_count)
        || security_stream_present != 1
        || cleanup_complete != 1
        || proof_schema != crate::OBJECT_ARCHIVE_PROOF_SCHEMA
    {
        return Err("pending archive proof is invalid or incomplete".to_string());
    }
    if let Some(initial) = item.archive_initial_stamp.as_ref() {
        if initial.volume_id.is_empty()
            || initial.file_id.is_empty()
            || !initial.same_object(&stamp)
        {
            return Err(
                "pending archive proof does not identify the journaled CREATE_NEW object"
                    .to_string(),
            );
        }
    }
    Ok(Some(PendingArchiveProof {
        stamp,
        hash,
        raw_backup_hash,
        semantic_hash,
        roundtrip_hash,
        stream_count,
    }))
}

fn exact_partial_truth(path: &Path, proof: &PendingArchiveProof) -> ExactPartialTruth {
    match BoundFile::open_for_move(path) {
        Ok(mut file) => {
            if let Err(error) = file.verify_stamp(&proof.stamp) {
                return ExactPartialTruth::Ambiguous(format!(
                    "partial archive path contains a different object: {error}"
                ));
            }
            if let Err(error) = file.verify_hash(&proof.hash) {
                return ExactPartialTruth::Ambiguous(format!(
                    "partial archive path contains different bytes: {error}"
                ));
            }
            ExactPartialTruth::Exact(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match path_provably_absent(path) {
                Ok(true) => ExactPartialTruth::Absent,
                Ok(false) => ExactPartialTruth::Ambiguous(
                    "partial archive path reappeared while proving absence".to_string(),
                ),
                Err(error) => ExactPartialTruth::Ambiguous(format!(
                    "partial archive absence could not be proved: {error}"
                )),
            }
        }
        Err(error) => ExactPartialTruth::Ambiguous(format!(
            "partial archive could not be rebound by exact handle: {error}"
        )),
    }
}

fn exact_final_truth(path: &Path, proof: &PendingArchiveProof) -> ExactFinalTruth {
    match CommittedObjectArchive::open_existing(path, &proof.stamp, &proof.hash) {
        Ok(archive) => ExactFinalTruth::Exact(Box::new(archive)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match path_provably_absent(path) {
                Ok(true) => ExactFinalTruth::Absent,
                Ok(false) => ExactFinalTruth::Ambiguous(
                    "final archive path reappeared while proving absence".to_string(),
                ),
                Err(error) => ExactFinalTruth::Ambiguous(format!(
                    "final archive absence could not be proved: {error}"
                )),
            }
        }
        Err(error) => ExactFinalTruth::Ambiguous(format!(
            "final archive could not be rebound by FileId/hash: {error}"
        )),
    }
}

fn journal_recovered_partial_cleanup(
    conn: &Connection,
    operation_id: i64,
    item: &BatchDeleteRecoveryItem,
    partial_path: &Path,
) -> Result<(), RecoveryError> {
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'archive_partial_cleaned', updated_at = ?2
         WHERE id = ?1 AND status IN ('planned', 'interrupted', 'blocked')
           AND archive_id IS NULL",
        params![item.batch_item_id, now],
    )?;
    transaction.execute(
        "INSERT INTO object_archive_event(
            archive_id, operation_id, kind, status, reason_code, message, created_at
         ) VALUES(NULL, ?1, 'recovery_partial_cleanup', 'done',
                  'interrupted', ?2, ?3)",
        params![
            operation_id,
            format!(
                "Recovery disposed the exact FileId/hash-bound partial archive at {} without promoting it or continuing held-object deletion.",
                partial_path.display()
            ),
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn reconcile_promoted_archive(
    conn: &Connection,
    operation_id: i64,
    item: &BatchDeleteRecoveryItem,
    archive: &CommittedObjectArchive,
    proof: &PendingArchiveProof,
) -> Result<(), RecoveryError> {
    let (Some(backup_id), Some(expected), Some(held_hash)) = (
        item.backup_id,
        item.expected_stamp.as_ref(),
        item.expected_hash.as_deref(),
    ) else {
        return Err(rusqlite::Error::InvalidQuery.into());
    };
    if backup_id <= 0 || !is_hex_64(held_hash) {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let existing: Option<ExistingArchiveRecoveryRow> = transaction
        .query_row(
            "SELECT id, archive_path, archive_volume_id, archive_file_id,
                    archive_bytes, archive_modified_unix_seconds,
                    archive_blake3, raw_backup_blake3, semantic_blake3,
                    roundtrip_blake3, stream_count, security_stream_present,
                    cleanup_complete, proof_schema, status
             FROM object_archive WHERE quarantine_entry_id = ?1",
            [item.entry_id],
            |row| {
                Ok(ExistingArchiveRecoveryRow {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    volume_id: row.get(2)?,
                    file_id: row.get(3)?,
                    bytes: row.get(4)?,
                    modified_unix_seconds: row.get(5)?,
                    archive_hash: row.get(6)?,
                    raw_backup_hash: row.get(7)?,
                    semantic_hash: row.get(8)?,
                    roundtrip_hash: row.get(9)?,
                    stream_count: row.get(10)?,
                    security_stream_present: row.get(11)?,
                    cleanup_complete: row.get(12)?,
                    proof_schema: row.get(13)?,
                    status: row.get(14)?,
                })
            },
        )
        .optional()?;
    let archive_id = if let Some(existing) = existing {
        if Path::new(&existing.path) != archive.path()
            || existing.volume_id != archive.stamp().volume_id
            || existing.file_id != archive.stamp().file_id
            || existing.bytes.max(0) as u64 != archive.stamp().bytes
            || existing.modified_unix_seconds != archive.stamp().modified_unix_seconds
            || existing.archive_hash != proof.hash
            || existing.raw_backup_hash != proof.raw_backup_hash
            || existing.semantic_hash != proof.semantic_hash
            || existing.roundtrip_hash != proof.roundtrip_hash
            || existing.stream_count != proof.stream_count
            || existing.security_stream_present != 1
            || existing.cleanup_complete != 1
            || existing.proof_schema != crate::OBJECT_ARCHIVE_PROOF_SCHEMA
            || existing.status != "ready"
        {
            return Err(rusqlite::Error::InvalidQuery.into());
        }
        existing.id
    } else {
        transaction.execute(
            "INSERT INTO object_archive(
                backup_id, quarantine_entry_id, removal_group_id,
                original_path, held_path, held_volume_id, held_file_id,
                held_bytes, held_modified_unix_seconds, held_content_blake3,
                archive_path, source_volume_id, source_file_id, source_bytes,
                source_modified_unix_seconds, source_content_blake3,
                archive_volume_id, archive_file_id, archive_bytes,
                archive_modified_unix_seconds, archive_blake3,
                raw_backup_blake3, semantic_blake3, roundtrip_blake3,
                stream_count, security_stream_present, cleanup_complete,
                proof_schema, status, verified_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                      ?11, ?6, ?7, ?8, ?9, ?10, ?12, ?13, ?14, ?15,
                      ?16, ?17, ?18, ?19, ?20, 1, 1, ?21, 'ready', ?22)",
            params![
                backup_id,
                item.entry_id,
                item.removal_group_id,
                item.original_path,
                item.held_path,
                expected.volume_id,
                expected.file_id,
                expected.bytes as i64,
                expected.modified_unix_seconds,
                held_hash,
                archive.path().to_string_lossy(),
                archive.stamp().volume_id,
                archive.stamp().file_id,
                archive.stamp().bytes as i64,
                archive.stamp().modified_unix_seconds,
                proof.hash,
                proof.raw_backup_hash,
                proof.semantic_hash,
                proof.roundtrip_hash,
                proof.stream_count,
                crate::OBJECT_ARCHIVE_PROOF_SCHEMA,
                now,
            ],
        )?;
        transaction.last_insert_rowid()
    };
    let linked = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET archive_id = ?2, phase = 'archive_ready', updated_at = ?3
         WHERE id = ?1 AND status IN ('planned', 'interrupted', 'blocked')
           AND archive_id IS NULL
           AND phase IN ('archive_proof_persisted',
                         'archive_recovery_pending', 'blocked')",
        params![item.batch_item_id, archive_id, now],
    )?;
    if linked != 1 {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    transaction.execute(
        "INSERT INTO object_archive_event(
            archive_id, operation_id, kind, status, reason_code, message, created_at
         ) VALUES(?1, ?2, 'recovery_promotion_reconciled', 'done',
                  'interrupted', ?3, ?4)",
        params![
            archive_id,
            operation_id,
            "Recovery found the exact already-promoted object_archive/2 payload and linked it without continuing held-object deletion.",
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn recover_pending_archive_intent(
    conn: &Connection,
    operation_id: i64,
    item: &BatchDeleteRecoveryItem,
) -> Result<PendingArchiveRecovery, RecoveryError> {
    let any_layout = item.elevation_capability_id.is_some()
        || item.transport_nonce.is_some()
        || item.nonce_digest.is_some()
        || item.capability_index.is_some()
        || item.archive_partial_path.is_some()
        || item.archive_final_path.is_some()
        || item.archive_initial_stamp.is_some();
    if !any_layout {
        return Ok(PendingArchiveRecovery::NoIntent);
    }
    if !matches!(
        item.item_phase.as_str(),
        "archive_path_intent"
            | "archive_container_bound"
            | "archive_proof_persisted"
            | "archive_partial_cleaned"
            | "archive_ready"
            | "archive_recovery_pending"
            | "blocked"
    ) {
        return Ok(PendingArchiveRecovery::Ambiguous(format!(
            "archive layout is inconsistent with batch phase {}",
            item.item_phase
        )));
    }
    let (
        Some(elevation_id),
        Some(nonce),
        Some(nonce_digest),
        Some(index),
        Some(destination),
        Some(final_path_text),
    ) = (
        item.elevation_capability_id,
        item.transport_nonce.as_deref(),
        item.nonce_digest.as_deref(),
        item.capability_index,
        item.backup_destination.as_deref(),
        item.archive_final_path.as_deref(),
    )
    else {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "archive intent is only partially journaled".to_string(),
        ));
    };
    if elevation_id <= 0
        || !is_hex_64(nonce)
        || !is_hex_64(nonce_digest)
        || blake3::hash(nonce.as_bytes()).to_hex().as_str() != nonce_digest
        || !(0..=u32::MAX as i64).contains(&index)
    {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "archive intent nonce/index authority is invalid".to_string(),
        ));
    }
    let elevation_authority: i64 = conn.query_row(
        "SELECT COUNT(*) FROM elevation_capability
         WHERE id = ?1 AND operation_id = ?2
           AND transport_nonce = ?3 AND nonce_digest = ?4",
        params![elevation_id, operation_id, nonce, nonce_digest],
        |row| row.get(0),
    )?;
    let backup_authority: i64 = match item.backup_id {
        Some(backup_id) if backup_id > 0 => conn.query_row(
            "SELECT COUNT(*) FROM backup
             WHERE id = ?1 AND verified = 1 AND destination = ?2",
            params![backup_id, destination],
            |row| row.get(0),
        )?,
        _ => 0,
    };
    if elevation_authority != 1 || backup_authority != 1 {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "archive intent is not bound to this operation and verified backup".to_string(),
        ));
    }
    let destination = Path::new(destination);
    if let Err(error) = bound_fs::validate_local_mutation_path(destination) {
        return Ok(PendingArchiveRecovery::Ambiguous(format!(
            "archive destination is unsafe: {error}"
        )));
    }
    let index = index as u32;
    let derived_partial = crate::archive_path_for_capability(destination, nonce, index);
    let derived_final = destination
        .join(crate::OBJECT_ARCHIVE_DIRECTORY_NAME)
        .join(format!("entry-{:016x}.chobj", item.entry_id));
    let final_path = Path::new(final_path_text);
    if final_path != derived_final || bound_fs::validate_local_mutation_path(final_path).is_err() {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "persisted final archive path is not backup/entry-derived".to_string(),
        ));
    }
    let Some(partial_path_text) = item.archive_partial_path.as_deref() else {
        // Reverification of an already committed archive creates no partial
        // archive container. The existing object_archive row remains the only
        // authority, and the batch item can be safely closed without mutation.
        return Ok(PendingArchiveRecovery::SafeToBlock);
    };
    let partial_path = Path::new(partial_path_text);
    if partial_path != derived_partial
        || bound_fs::validate_local_mutation_path(partial_path).is_err()
    {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "persisted partial archive path is not nonce/index-derived".to_string(),
        ));
    }
    let proof = match pending_archive_proof(item) {
        Ok(Some(proof)) => proof,
        Ok(None) => return match path_provably_absent(partial_path) {
            Ok(true) => Ok(PendingArchiveRecovery::SafeToBlock),
            Ok(false) => Ok(PendingArchiveRecovery::Ambiguous(
                "partial archive exists without a journaled FileId/hash proof; it was preserved"
                    .to_string(),
            )),
            Err(error) => Ok(PendingArchiveRecovery::Ambiguous(format!(
                "unproved partial archive presence is ambiguous: {error}"
            ))),
        },
        Err(error) => return Ok(PendingArchiveRecovery::Ambiguous(error)),
    };
    if item.archive_initial_stamp.is_none() {
        return Ok(PendingArchiveRecovery::Ambiguous(
            "new partial archive proof has no journaled initial FileId".to_string(),
        ));
    }

    let partial = exact_partial_truth(partial_path, &proof);
    let final_archive = exact_final_truth(final_path, &proof);
    match (partial, final_archive) {
        (ExactPartialTruth::Absent, ExactFinalTruth::Absent) => {
            Ok(PendingArchiveRecovery::SafeToBlock)
        }
        (ExactPartialTruth::Exact(file), ExactFinalTruth::Absent) => {
            if let Err(error) = file.delete_exact(&proof.hash) {
                return Ok(PendingArchiveRecovery::Ambiguous(format!(
                    "exact partial archive disposition was not provably completed: {error}"
                )));
            }
            journal_recovered_partial_cleanup(conn, operation_id, item, partial_path)?;
            Ok(PendingArchiveRecovery::SafeToBlock)
        }
        (ExactPartialTruth::Absent, ExactFinalTruth::Exact(archive)) => {
            reconcile_promoted_archive(conn, operation_id, item, &archive, &proof)?;
            Ok(PendingArchiveRecovery::SafeToBlock)
        }
        (ExactPartialTruth::Ambiguous(reason), _)
        | (_, ExactFinalTruth::Ambiguous(reason)) => {
            Ok(PendingArchiveRecovery::Ambiguous(reason))
        }
        (ExactPartialTruth::Exact(_), ExactFinalTruth::Exact(_)) => {
            Ok(PendingArchiveRecovery::Ambiguous(
                "both partial and final archive paths contain proof-matching objects; both were preserved"
                    .to_string(),
            ))
        }
    }
}

fn pending_scratch_cleanup_reason(
    conn: &Connection,
    operation_id: i64,
    item: &BatchDeleteRecoveryItem,
) -> Result<String, RecoveryError> {
    let (Some(nonce), Some(nonce_digest), Some(index), Some(destination)) = (
        item.transport_nonce.as_deref(),
        item.nonce_digest.as_deref(),
        item.capability_index,
        item.backup_destination.as_deref(),
    ) else {
        return Ok(
            "Scratch cleanup is pending but its nonce/index journal is incomplete; no path was touched."
                .to_string(),
        );
    };
    if !is_hex_64(nonce)
        || !is_hex_64(nonce_digest)
        || blake3::hash(nonce.as_bytes()).to_hex().as_str() != nonce_digest
        || !(0..=u32::MAX as i64).contains(&index)
    {
        return Ok(
            "Scratch cleanup is pending with invalid nonce/index authority; no path was touched."
                .to_string(),
        );
    }
    let root = Path::new(destination);
    if let Err(error) = bound_fs::validate_local_mutation_path(root) {
        return Ok(format!(
            "Scratch cleanup root is unsafe ({error}); no path was touched."
        ));
    }
    let expected = root.join(crate::scratch_leaf_for_capability(nonce, index as u32));
    if let Err(error) = bound_fs::validate_local_mutation_path(&expected) {
        return Ok(format!(
            "Derived scratch cleanup path is unsafe ({error}); no path was touched."
        ));
    }
    let rows: i64 = conn.query_row(
        "SELECT COUNT(*) FROM archive_cleanup
         WHERE operation_id = ?1 AND scratch_path = ?2
           AND status = 'pending_identity_proof'
           AND expected_volume_id IS NULL AND expected_file_id IS NULL",
        params![operation_id, expected.to_string_lossy()],
        |row| row.get(0),
    )?;
    Ok(if rows == 1 {
        format!(
            "Scratch cleanup remains pending at {} because no exact FileId/hash proof was captured; recovery preserved it and did not unblock the batch.",
            expected.display()
        )
    } else {
        "Scratch cleanup journal is missing or duplicated; recovery preserved every path and kept the batch interrupted."
            .to_string()
    })
}

fn block_unstarted_batch_item(
    conn: &Connection,
    item: &BatchDeleteRecoveryItem,
) -> Result<(), RecoveryError> {
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let item_updated = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archive_recovery_closed' ELSE 'blocked' END,
             status = 'blocked', reason_code = 'interrupted',
             message = 'Recovery closed an archive/preflight item before any parent disposition began.',
             updated_at = ?2
         WHERE id = ?1 AND status IN ('planned', 'ready', 'interrupted', 'blocked')",
        params![item.batch_item_id, now],
    )?;
    let operation_updated = transaction.execute(
        "UPDATE operation_item SET status = 'skipped'
         WHERE id = ?1 AND status IN ('pending', 'skipped')",
        [item.operation_item_id],
    )?;
    if item_updated != 1 || operation_updated != 1 || item.entry_status != "quarantined" {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    transaction.commit()?;
    Ok(())
}

fn roll_back_batch_delete_intent(
    conn: &Connection,
    item: &BatchDeleteRecoveryItem,
) -> Result<(), RecoveryError> {
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let entry = transaction.execute(
        "UPDATE quarantine_entry SET status = 'quarantined'
         WHERE id = ?1 AND status = 'deleting' AND quarantine_path = ?2",
        params![item.entry_id, item.held_path],
    )?;
    let batch_item = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'blocked', status = 'blocked', reason_code = 'interrupted',
             message = 'Recovery proved the exact held object still present; parent disposition did not complete.',
             updated_at = ?2
         WHERE id = ?1 AND status IN ('deleting', 'interrupted')",
        params![item.batch_item_id, now],
    )?;
    let operation_item = transaction.execute(
        "UPDATE operation_item SET status = 'rolled_back'
         WHERE id = ?1 AND status IN ('deleting', 'failed')",
        [item.operation_item_id],
    )?;
    if entry != 1 || batch_item != 1 || operation_item != 1 {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    transaction.commit()?;
    Ok(())
}

fn settle_batch_delete_absent(
    conn: &Connection,
    operation_id: i64,
    item: &BatchDeleteRecoveryItem,
) -> Result<(), RecoveryError> {
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let entry = transaction.execute(
        "UPDATE quarantine_entry SET status = 'permanently_deleted'
         WHERE id = ?1 AND status IN ('deleting', 'permanently_deleted')
           AND quarantine_path = ?2",
        params![item.entry_id, item.held_path],
    )?;
    let batch_item = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'finished', status = 'deleted', reason_code = NULL,
             message = 'Recovery proved the archived held object absent after a durable delete intent.',
             updated_at = ?2
         WHERE id = ?1 AND status IN ('deleting', 'interrupted', 'deleted')",
        params![item.batch_item_id, now],
    )?;
    let operation_item = transaction.execute(
        "UPDATE operation_item SET status = 'done'
         WHERE id = ?1 AND status IN ('deleting', 'failed', 'done')",
        [item.operation_item_id],
    )?;
    if entry != 1 || batch_item != 1 || operation_item != 1 {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    if let Some(volume_id) = item.held_volume_id.as_deref() {
        transaction.execute(
            "INSERT INTO mutation_space_effect(
                operation_id, operation_item_id, volume_id, lifecycle_stage,
                logical_bytes, allocated_bytes, free_space_delta_observed, created_at
             ) SELECT ?1, ?2, ?3, 'holding_object_removed', ?4, NULL, NULL, ?5
               WHERE NOT EXISTS (
                 SELECT 1 FROM mutation_space_effect
                 WHERE operation_item_id = ?2 AND lifecycle_stage = 'holding_object_removed'
               )",
            params![
                operation_id,
                item.operation_item_id,
                volume_id,
                item.logical_bytes as i64,
                now,
            ],
        )?;
    }
    transaction.commit()?;
    cleanup_terminal_guardian_receipt(conn, item)?;
    Ok(())
}

fn cleanup_terminal_guardian_receipt(
    conn: &Connection,
    item: &BatchDeleteRecoveryItem,
) -> Result<(), RecoveryError> {
    if item.guardian_receipt_cleanup_complete != Some(0) {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let (Some(path), Some(volume_id), Some(file_id)) = (
            item.guardian_receipt_path.as_deref(),
            item.guardian_receipt_volume_id.as_deref(),
            item.guardian_receipt_file_id.as_deref(),
        ) else {
            return Ok(());
        };
        let path = Path::new(path);
        let stamp = FileStamp {
            volume_id: volume_id.to_string(),
            file_id: file_id.to_string(),
            bytes: 0,
            modified_unix_seconds: None,
        };
        let cleanup = match bound_fs::cleanup_exact_guardian_receipt(path, &stamp) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match path_provably_absent(path) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err("receipt path reappeared during cleanup".to_string()),
                    Err(probe) => Err(format!(
                        "receipt absence could not be proved after cleanup failure: {probe}"
                    )),
                }
            }
            Err(error) => Err(error.to_string()),
        };
        let now = chrono::Utc::now().to_rfc3339();
        match cleanup {
            Ok(()) => {
                conn.execute(
                    "UPDATE final_disposition_guardian
                     SET receipt_cleanup_complete = 1, updated_at = ?2
                     WHERE batch_item_id = ?1 AND receipt_cleanup_complete = 0",
                    params![item.batch_item_id, now],
                )?;
            }
            Err(error) => {
                let bounded: String = error.chars().take(1024).collect();
                conn.execute(
                    "UPDATE final_disposition_guardian
                     SET error = COALESCE(error || '; ', '') || ?2, updated_at = ?3
                     WHERE batch_item_id = ?1 AND receipt_cleanup_complete = 0",
                    params![
                        item.batch_item_id,
                        format!("terminal receipt cleanup remains pending: {bounded}"),
                        now,
                    ],
                )?;
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (conn, item);
    }
    Ok(())
}

fn finish_recovered_delete_batch(
    conn: &Connection,
    batch_id: i64,
    operation_id: i64,
    requested_count: i64,
    ambiguity: Option<&str>,
) -> Result<bool, RecoveryError> {
    let (rows, deleted, blocked, failed, nonterminal): (i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                    SUM(CASE WHEN status = 'deleted' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status NOT IN ('deleted', 'blocked', 'failed') THEN 1 ELSE 0 END)
             FROM permanent_delete_batch_item WHERE batch_id = ?1",
        [batch_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        },
    )?;
    let count_mismatch = rows != requested_count;
    if ambiguity.is_some() || nonterminal > 0 || count_mismatch {
        let reason = ambiguity.unwrap_or(if count_mismatch {
            "Recovery found a permanent-delete batch whose requested/item counts differ."
        } else {
            "Recovery left at least one permanent-delete item ambiguous."
        });
        let transaction = conn.unchecked_transaction()?;
        transaction.execute(
            "UPDATE permanent_delete_batch SET status = 'interrupted', error = ?2
             WHERE id = ?1",
            params![batch_id, reason],
        )?;
        transaction.execute(
            "UPDATE operation SET status = 'interrupted', error = ?2
             WHERE id = ?1",
            params![operation_id, reason],
        )?;
        transaction.commit()?;
        return Ok(false);
    }
    let status = if requested_count > 0 && deleted == requested_count {
        "completed"
    } else if deleted > 0 {
        "partial"
    } else {
        "failed"
    };
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch
         SET eligible_count = ?2, removed_count = ?3, blocked_count = ?4,
             failed_count = ?5, status = ?6, finished_at = ?7,
             error = CASE WHEN ?6 = 'failed'
                          THEN 'Recovery closed the batch without deleting a held object.'
                          ELSE error END
         WHERE id = ?1",
        params![
            batch_id,
            rows - blocked,
            deleted,
            blocked,
            failed,
            status,
            now,
        ],
    )?;
    transaction.execute(
        "UPDATE operation SET status = ?2, finished_at = ?3,
             error = CASE WHEN ?2 = 'failed'
                          THEN 'Recovery closed the batch without deleting a held object.'
                          ELSE error END
         WHERE id = ?1",
        params![operation_id, status, now],
    )?;
    transaction.commit()?;
    Ok(true)
}

fn recover_permanent_delete_batches(
    conn: &Connection,
    report: &mut RecoveryReport,
) -> Result<(), RecoveryError> {
    let batches: Vec<(i64, i64, i64)> = {
        let mut statement = conn.prepare(
            "SELECT b.id, b.operation_id, b.requested_count
             FROM permanent_delete_batch b
             JOIN operation o ON o.id = b.operation_id
             WHERE NOT (
                (b.status = 'completed' AND o.status = 'completed') OR
                (b.status = 'partial' AND o.status = 'partial') OR
                (b.status = 'cancelled' AND o.status = 'cancelled') OR
                (b.status = 'failed' AND o.status = 'failed')
              ) OR EXISTS (
                 SELECT 1 FROM permanent_delete_batch_item bi
                 JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
                 WHERE bi.batch_id = b.id
                   AND (bi.status NOT IN ('deleted', 'blocked', 'failed')
                        OR qe.status = 'deleting')
             ) OR EXISTS (
                 SELECT 1 FROM permanent_delete_batch_item bi
                 WHERE bi.batch_id = b.id AND bi.archive_id IS NULL
                   AND bi.archive_proof_blake3 IS NOT NULL
                   AND bi.phase != 'archive_recovery_closed'
             ) OR EXISTS (
                 SELECT 1 FROM permanent_delete_batch_item bi
                 JOIN final_disposition_guardian fdg ON fdg.batch_item_id = bi.id
                 JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
                 WHERE bi.batch_id = b.id AND bi.status = 'deleted'
                   AND qe.status = 'permanently_deleted'
                   AND fdg.receipt_cleanup_complete = 0
              ) OR b.requested_count != (
                SELECT COUNT(*) FROM permanent_delete_batch_item bi
                WHERE bi.batch_id = b.id
             )
             ORDER BY b.id",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (batch_id, operation_id, requested_count) in batches {
        let items = load_batch_delete_recovery_items(conn, batch_id)?;
        let mut ambiguity = None::<String>;
        for item in &items {
            if item.item_phase == "scratch_cleanup_pending" && item.entry_status == "quarantined" {
                ambiguity = Some(format!(
                    "Batch item {}: {}",
                    item.batch_item_id,
                    pending_scratch_cleanup_reason(conn, operation_id, item)?
                ));
                break;
            }
            // A helper proof persisted before promotion remains recovery work
            // even if an in-process DB error subsequently labelled the item or
            // batch terminal. Reconcile/clean the archive object first, never
            // the held object, then close this item as blocked.
            if item.entry_status == "quarantined"
                && item.archive_id.is_none()
                && item.pending_archive_hash.is_some()
            {
                match recover_pending_archive_intent(conn, operation_id, item)? {
                    PendingArchiveRecovery::SafeToBlock => {
                        if let Err(error) = block_unstarted_batch_item(conn, item) {
                            ambiguity = Some(format!(
                                "Recovery could not close archive-pending batch item {} atomically: {error}",
                                item.batch_item_id
                            ));
                            break;
                        }
                        continue;
                    }
                    PendingArchiveRecovery::NoIntent => {
                        ambiguity = Some(format!(
                            "Batch item {} has an archive proof without its nonce/index intent.",
                            item.batch_item_id
                        ));
                        break;
                    }
                    PendingArchiveRecovery::Ambiguous(reason) => {
                        ambiguity = Some(format!(
                            "Batch item {} archive recovery remains ambiguous: {reason}",
                            item.batch_item_id
                        ));
                        break;
                    }
                }
            }
            match item.item_status.as_str() {
                "planned" | "ready" if item.entry_status == "quarantined" => {
                    if item.item_status == "planned" && item.archive_id.is_none() {
                        match recover_pending_archive_intent(conn, operation_id, item)? {
                            PendingArchiveRecovery::NoIntent
                            | PendingArchiveRecovery::SafeToBlock => {}
                            PendingArchiveRecovery::Ambiguous(reason) => {
                                ambiguity = Some(format!(
                                    "Batch item {} archive recovery remains ambiguous: {reason}",
                                    item.batch_item_id
                                ));
                                break;
                            }
                        }
                    }
                    if let Err(error) = block_unstarted_batch_item(conn, item) {
                        ambiguity = Some(format!(
                            "Recovery could not close unstarted batch item {} atomically: {error}",
                            item.batch_item_id
                        ));
                        break;
                    }
                }
                "deleting" | "interrupted" | "deleted" => {
                    match batch_archive_is_bound(item) {
                        Ok(true) => {}
                        Ok(false) => {
                            ambiguity = Some(format!(
                                "Batch item {} has no complete bound object_archive/2 proof.",
                                item.batch_item_id
                            ));
                            break;
                        }
                        Err(error) => {
                            ambiguity = Some(format!(
                                "Batch item {} archive proof is ambiguous: {error}",
                                item.batch_item_id
                            ));
                            break;
                        }
                    }
                    let terminal_success_committed =
                        item.item_status == "deleted" && item.entry_status == "permanently_deleted";
                    if terminal_success_committed {
                        cleanup_terminal_guardian_receipt(conn, item)?;
                        continue;
                    }
                    match held_recovery_truth(item) {
                        HeldRecoveryTruth::ExactPresent if item.entry_status == "deleting" => {
                            if let Err(error) = roll_back_batch_delete_intent(conn, item) {
                                ambiguity = Some(format!(
                                    "Recovery could not roll back batch item {} atomically: {error}",
                                    item.batch_item_id
                                ));
                                break;
                            }
                            cleanup_terminal_guardian_receipt(conn, item)?;
                            report.rolled_back_items += 1;
                        }
                        HeldRecoveryTruth::ProvablyAbsent
                            if matches!(
                                item.entry_status.as_str(),
                                "deleting" | "permanently_deleted"
                            ) =>
                        {
                            let guardian_close_authorized =
                                match guardian_close_was_authorized(item, operation_id) {
                                    Ok(authority) => authority,
                                    Err(error) => {
                                        ambiguity = Some(format!(
                                            "Batch item {} guardian proof is ambiguous: {error}",
                                            item.batch_item_id
                                        ));
                                        break;
                                    }
                                };
                            let final_profile_proved = guardian_journal_is_present(item)
                                && guardian_close_authorized
                                && item.item_reason_code.as_deref()
                                    == Some(crate::final_remove::PROVED_FINAL_PROFILE_REASON_CODE);
                            if !final_profile_proved {
                                ambiguity = Some(format!(
                                    "Batch item {} disappeared before both the final profile and guardian close authority were durably proved; recovery preserved the archive and refused to claim deletion.",
                                    item.batch_item_id
                                ));
                                break;
                            }
                            if let Err(error) = settle_batch_delete_absent(conn, operation_id, item)
                            {
                                ambiguity = Some(format!(
                                    "Recovery could not settle batch item {} atomically: {error}",
                                    item.batch_item_id
                                ));
                                break;
                            }
                        }
                        HeldRecoveryTruth::Ambiguous(reason) => {
                            ambiguity = Some(format!(
                                "Batch item {} remains ambiguous: {reason}",
                                item.batch_item_id
                            ));
                            break;
                        }
                        _ => {
                            ambiguity = Some(format!(
                                "Batch item {} has contradictory entry/item/held state.",
                                item.batch_item_id
                            ));
                            break;
                        }
                    }
                }
                "blocked" | "failed" if item.entry_status == "quarantined" => {}
                _ => {
                    ambiguity = Some(format!(
                        "Batch item {} has unsupported recovery state item={} entry={}.",
                        item.batch_item_id, item.item_status, item.entry_status
                    ));
                    break;
                }
            }
        }
        if finish_recovered_delete_batch(
            conn,
            batch_id,
            operation_id,
            requested_count,
            ambiguity.as_deref(),
        )? {
            report.recovered_operations += 1;
        }
    }
    Ok(())
}

fn recover_bound_move(item: &RecoveryItem) -> BoundRecovery {
    let (expected, expected_hash) = match (&item.expected, item.expected_hash.as_deref()) {
        (Some(stamp), Some(hash)) => (stamp, hash),
        _ => {
            return BoundRecovery::Unresolved(
                "Recovery refused a legacy path-only move without file identity and hash"
                    .to_string(),
            )
        }
    };
    let from = Path::new(&item.from_path);
    let to = Path::new(&item.to_path);
    if let Err(error) = bound_fs::validate_local_mutation_path(from) {
        return BoundRecovery::Unresolved(format!(
            "Recovery refused an unsafe original path: {error}"
        ));
    }
    if let Err(error) = bound_fs::validate_local_mutation_path(to) {
        return BoundRecovery::Unresolved(format!(
            "Recovery refused an unsafe destination path: {error}"
        ));
    }

    // A same-volume rename preserves volume+file identity. Therefore a pending
    // item whose destination proves the exact expected identity, bytes and hash
    // is reconcilable even if the result columns were not committed before the
    // crash. A cross-volume copy has a different file id and stays unresolved.
    if item.result.is_none() {
        match bound_fs::path_matches(to, expected, expected_hash) {
            Ok(true) => {}
            Ok(false) => match path_present(to) {
                Ok(true) => {
                    return BoundRecovery::Unresolved(
                        "Recovery found a destination for a pending move without a matching same-volume object identity; all copies were preserved for manual review"
                            .to_string(),
                    )
                }
                Ok(false) => {}
                Err(error) => {
                    return BoundRecovery::Unresolved(format!(
                        "Recovery could not safely probe the unproved destination: {error}"
                    ))
                }
            },
            Err(error) => {
                return BoundRecovery::Unresolved(format!(
                    "Recovery could not prove the pending destination identity: {error}"
                ))
            }
        }
    }
    let result_stamp = item.result.as_ref().unwrap_or(expected);
    let result_hash = item.result_hash.as_deref().unwrap_or(expected_hash);
    let from_matches = match bound_fs::path_matches(from, expected, expected_hash) {
        Ok(matches) => matches,
        Err(error) => {
            return BoundRecovery::Unresolved(format!(
                "Recovery could not prove the original identity: {error}"
            ))
        }
    };
    let to_matches = match bound_fs::path_matches(to, result_stamp, result_hash) {
        Ok(matches) => matches,
        Err(error) => {
            return BoundRecovery::Unresolved(format!(
                "Recovery could not prove the held identity: {error}"
            ))
        }
    };
    if from_matches && to_matches {
        return BoundRecovery::BothCopies {
            bytes: result_stamp.bytes,
        };
    }
    if from_matches {
        return BoundRecovery::RolledBack;
    }
    if !to_matches {
        return BoundRecovery::Unresolved(
            "Recovery found neither expected identity; no path was mutated".to_string(),
        );
    }

    let held = match BoundFile::open_for_move(to) {
        Ok(held) => held,
        Err(error) => {
            return BoundRecovery::Unresolved(format!(
                "Recovery could not bind the held file: {error}"
            ))
        }
    };
    if let Err(error) = held.verify_stamp(result_stamp) {
        return BoundRecovery::Unresolved(format!("Recovery held identity changed: {error}"));
    }
    let mut reverse = match held.prepare_move(from, result_hash) {
        Ok(prepared) => prepared,
        Err(error) => {
            return BoundRecovery::Unresolved(format!(
                "Recovery could not return the exact held file: {error}"
            ))
        }
    };
    if let Err(error) = reverse.finalize_source_removal() {
        return BoundRecovery::Unresolved(format!(
            "Recovery copied the held file back but could not dispose the exact old copy: {error}"
        ));
    }
    BoundRecovery::RolledBack
}

/// Roll back every operation left interrupted in the journal. Safe to call on
/// every launch; a clean journal (no in-flight operations) is a no-op.
pub fn recover_interrupted(conn: &Connection) -> Result<RecoveryReport, RecoveryError> {
    let mut report = RecoveryReport::default();

    // The batch flow owns its durable per-entry state and object_archive/2
    // authority. Reconcile it before the legacy/generic loops, which do not
    // understand batch item phases and must never reinterpret a batch row as a
    // one-entry legacy purge.
    recover_permanent_delete_batches(conn, &mut report)?;

    let operations: Vec<(i64, String, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, kind, backup_id FROM operation
             WHERE status IN ('executing', 'backup_running', 'verifying')
               AND kind NOT IN ('permanent_delete', 'permanent_delete_batch', 'restore')
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

    for (op_id, kind, backup_id) in operations {
        let items: Vec<RecoveryItem> = {
            // 'pending' items are included on purpose: quarantine is journal-first
            // (row inserted 'pending' → fs::rename → UPDATE 'done'), so a crash in
            // the window between the rename syscall and the 'done' update leaves an
            // APPLIED move recorded as 'pending'. Skipping those would strand the
            // file in the holding area with no rollback and no Recover entry. The
            // on-disk gates below make reversing a genuinely-unapplied pending item
            // a no-op (`to` does not exist), so including them is always safe.
            let mut stmt = conn.prepare(
                "SELECT id, from_path, to_path,
                        expected_volume_id, expected_file_id, bytes,
                        COALESCE(expected_blake3, checksum_before),
                        result_volume_id, result_file_id,
                        COALESCE(result_blake3, checksum_after)
                 FROM operation_item
                 WHERE operation_id = ?1 AND status IN ('pending', 'done')
                   AND action IN ('move_bound', 'copy_delete_bound', 'bound_move', 'move', 'copy_delete')
                   AND from_path IS NOT NULL AND to_path IS NOT NULL",
            )?;
            let rows = stmt.query_map([op_id], |row| {
                Ok(RecoveryItem {
                    id: row.get(0)?,
                    from_path: row.get(1)?,
                    to_path: row.get(2)?,
                    expected: stamp_from_columns(row.get(3)?, row.get(4)?, row.get(5)?),
                    expected_hash: row.get(6)?,
                    result: stamp_from_columns(row.get(7)?, row.get(8)?, row.get(5)?),
                    result_hash: row.get(9)?,
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };

        let mut operation_resolved = true;
        for item in &items {
            match recover_bound_move(item) {
                BoundRecovery::RolledBack => {
                    conn.execute(
                        "UPDATE operation_item SET status = 'rolled_back' WHERE id = ?1",
                        [item.id],
                    )?;
                    report.rolled_back_items += 1;
                }
                BoundRecovery::BothCopies { bytes } => {
                    if kind == "quarantine" {
                        expose_ambiguous_held_copy(
                            conn,
                            op_id,
                            &item.from_path,
                            &item.to_path,
                            backup_id,
                            bytes,
                        )?;
                    }
                    conn.execute(
                        "UPDATE operation_item SET status = 'done' WHERE id = ?1",
                        [item.id],
                    )?;
                }
                BoundRecovery::Unresolved(reason) => {
                    operation_resolved = false;
                    conn.execute(
                        "UPDATE operation SET error = ?2 WHERE id = ?1",
                        params![op_id, reason],
                    )?;
                }
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
            let proof = items
                .iter()
                .find(|item| item.from_path == original_path && item.to_path == quarantine_path);
            let restored = match proof
                .and_then(|item| Some((item.expected.as_ref()?, item.expected_hash.as_deref()?)))
            {
                Some((expected, expected_hash)) => match (
                    bound_fs::path_matches(Path::new(&original_path), expected, expected_hash),
                    path_provably_absent(Path::new(&quarantine_path)),
                ) {
                    (Ok(true), Ok(true)) => true,
                    (Ok(_), Ok(_)) => false,
                    (Err(error), _) | (_, Err(error)) => {
                        operation_resolved = false;
                        conn.execute(
                            "UPDATE operation SET error = ?2 WHERE id = ?1",
                            params![
                                op_id,
                                format!(
                                    "Recovery could not prove the restored entry identity: {error}"
                                )
                            ],
                        )?;
                        false
                    }
                },
                None => {
                    operation_resolved = false;
                    conn.execute(
                        "UPDATE operation SET error = ?2 WHERE id = ?1",
                        params![
                            op_id,
                            "Recovery cannot mark an entry restored without an identity+hash journal proof"
                        ],
                    )?;
                    false
                }
            };
            if restored {
                conn.execute(
                    "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
                    [entry_id],
                )?;
            }
        }
        // The operation is terminal (we reversed everything we safely could); any entry that
        // could not be put back stays 'quarantined' and remains recoverable from the Recover view.
        if operation_resolved {
            // A quarantine rollback recreates every removed ancestor while returning
            // the held files. Reflect that inverse in the cleanup journal too: leaving
            // a `remove_dir` intent pending beneath a terminal rolled-back operation
            // would make the audit trail claim an unfinished deletion forever.
            let restored_cleanup_dirs = conn.execute(
                "UPDATE operation_item SET status = 'rolled_back'
                 WHERE operation_id = ?1 AND action = 'remove_dir'
                   AND status IN ('pending', 'done')",
                [op_id],
            )?;
            report.rolled_back_items += restored_cleanup_dirs;
            conn.execute(
                "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
                params![op_id, chrono::Utc::now().to_rfc3339()],
            )?;
            report.recovered_operations += 1;
        }
    }

    // Reconcile any permanent delete interrupted mid-unlink: the entry was flipped
    // to 'deleting' BEFORE the unlink, so the on-disk truth of the held copy decides
    // the outcome — gone means the delete completed, still present means it did not.
    let deleting: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT qe.id, qe.quarantine_path
             FROM quarantine_entry qe
             WHERE qe.status = 'deleting'
               AND NOT EXISTS (
                 SELECT 1 FROM permanent_delete_batch_item bi
                 WHERE bi.quarantine_entry_id = qe.id
               )",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (entry_id, quarantine_path) in deleting {
        let proof = load_delete_proof(conn, &quarantine_path)?;
        let now = chrono::Utc::now().to_rfc3339();
        match proof {
            Some((op_id, item_id, stamp, hash)) => {
                match path_provably_absent(Path::new(&quarantine_path)) {
                    Ok(true) => {
                        conn.execute(
                        "UPDATE quarantine_entry SET status = 'permanently_deleted' WHERE id = ?1",
                        [entry_id],
                    )?;
                        conn.execute(
                            "UPDATE operation_item SET status = 'done' WHERE id = ?1",
                            [item_id],
                        )?;
                        conn.execute(
                            "UPDATE operation SET status = 'done', finished_at = ?2 WHERE id = ?1",
                            params![op_id, now],
                        )?;
                        report.recovered_operations += 1;
                    }
                    Ok(false) => {
                        match bound_fs::path_matches(Path::new(&quarantine_path), &stamp, &hash) {
                            Ok(true) => {
                                conn.execute(
                                "UPDATE quarantine_entry SET status = 'quarantined' WHERE id = ?1",
                                [entry_id],
                            )?;
                                conn.execute(
                                "UPDATE operation_item SET status = 'rolled_back' WHERE id = ?1",
                                [item_id],
                            )?;
                                conn.execute(
                                "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
                                params![op_id, now],
                            )?;
                                report.rolled_back_items += 1;
                                report.recovered_operations += 1;
                            }
                            Ok(false) => {
                                conn.execute(
                                "UPDATE operation SET error = ?2 WHERE id = ?1",
                                params![op_id, "Recovery found a different file at the purge path; no action was taken"],
                            )?;
                            }
                            Err(error) => {
                                conn.execute(
                                    "UPDATE operation SET error = ?2 WHERE id = ?1",
                                    params![
                                        op_id,
                                        format!("Recovery could not prove purge identity: {error}")
                                    ],
                                )?;
                            }
                        }
                    }
                    Err(error) => {
                        conn.execute(
                            "UPDATE operation SET error = ?2 WHERE id = ?1",
                            params![
                                op_id,
                                format!(
                                    "Recovery refused an unsafe or unreadable purge path: {error}"
                                )
                            ],
                        )?;
                    }
                }
            }
            None => {
                // Legacy path-only deletion has no safe automatic resolution.
                // Keep it visible and mutation-blocking for explicit review.
            }
        }
    }

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
        let restore_item_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM operation_item WHERE operation_id = ?1",
            [op_id],
            |row| row.get(0),
        )?;
        if restore_item_count == 0 {
            // New restore intents commit operation + pending item atomically. Therefore an
            // old zero-item row proves that this implementation never started a filesystem
            // mutation. Close it terminally, without claiming a rollback, and leave the
            // quarantine entry visible for an explicit manual retry.
            conn.execute(
                "UPDATE operation
                 SET status = 'failed', finished_at = ?2, error = ?3
                 WHERE id = ?1",
                params![op_id, now, RESTORE_ZERO_ITEM_MANUAL],
            )?;
            report.recovered_operations += 1;
            continue;
        }
        let proof = load_restore_proof(conn, op_id)?;
        let Some(proof) = proof else {
            // An item exists, so filesystem work cannot be ruled out. An unknown legacy
            // action is not equivalent to the safe zero-item case above: leave it blocking
            // for manual reconciliation.
            conn.execute(
                "UPDATE operation SET error = ?2 WHERE id = ?1",
                params![
                    op_id,
                    "Recovery refused a restore whose existing journal items have no recognized identity-bound move proof"
                ],
            )?;
            continue;
        };
        let (expected_stamp, expected_hash) = match (proof.expected, proof.expected_hash) {
            (Some(stamp), Some(hash)) => (stamp, hash),
            _ => {
                conn.execute(
                    "UPDATE operation SET error = ?2 WHERE id = ?1",
                    params![
                        op_id,
                        "Recovery refused a path-only restore item without source identity and hash proof"
                    ],
                )?;
                continue;
            }
        };
        let destination_path = match destination.as_deref() {
            Some(path) => Path::new(path),
            None => {
                conn.execute(
                    "UPDATE operation SET error = ?2 WHERE id = ?1",
                    params![
                        op_id,
                        "Recovery found a restore item without a destination path"
                    ],
                )?;
                continue;
            }
        };
        if let Err(error) = bound_fs::validate_local_mutation_path(destination_path) {
            conn.execute(
                "UPDATE operation SET error = ?2 WHERE id = ?1",
                params![
                    op_id,
                    format!("Recovery refused an unsafe restore destination: {error}")
                ],
            )?;
            continue;
        }
        if let Some(held) = held_path.as_deref() {
            if let Err(error) = bound_fs::validate_local_mutation_path(Path::new(held)) {
                conn.execute(
                    "UPDATE operation SET error = ?2 WHERE id = ?1",
                    params![
                        op_id,
                        format!("Recovery refused an unsafe held-copy path: {error}")
                    ],
                )?;
                continue;
            }
        }

        // Reconcile purely by on-disk truth. "completed" requires the destination present AND
        // the held copy gone; a crash mid-copy (cross-volume restore is copy→verify→delete-
        // source) leaves a possibly-truncated file at the destination while the intact held
        // copy still sits in holding, so that case must NOT be finalized 'restored'.
        let destination_occupied = match path_present(destination_path) {
            Ok(present) => present,
            Err(error) => {
                conn.execute(
                    "UPDATE operation SET error = ?2 WHERE id = ?1",
                    params![
                        op_id,
                        format!("Recovery could not safely probe the restore destination: {error}")
                    ],
                )?;
                continue;
            }
        };
        let result_proof = proof.result.zip(proof.result_hash);
        if destination_occupied && result_proof.is_none() {
            // Most importantly, do not call this RolledBack merely because the held
            // source also survives. A cross-volume copy may be complete while the app
            // crashed before persisting its identity; both objects remain authoritative
            // enough to preserve, but neither is safe to classify automatically.
            conn.execute(
                "UPDATE operation SET status = 'verifying', error = ?2 WHERE id = ?1",
                params![op_id, RESTORE_RESULT_IDENTITY_MISSING],
            )?;
            continue;
        }
        let destination_present = match result_proof.as_ref() {
            Some((result_stamp, result_hash)) => {
                match bound_fs::path_matches(destination_path, result_stamp, result_hash) {
                    Ok(matches) => matches,
                    Err(error) => {
                        conn.execute(
                            "UPDATE operation SET error = ?2 WHERE id = ?1",
                            params![
                                op_id,
                                format!("Recovery could not prove the restore destination identity: {error}")
                            ],
                        )?;
                        continue;
                    }
                }
            }
            None => false,
        };
        // "Held gone" must be PROVEN: a transiently-unreadable held path counts as PRESENT
        // so we roll back (keep it visible) instead of finalizing a restore we cannot
        // substantiate and hiding the only good copy.
        let held_present = match held_path.as_deref() {
            Some(held) => {
                match bound_fs::path_matches(Path::new(held), &expected_stamp, &expected_hash) {
                    Ok(matches) => matches,
                    Err(error) => {
                        conn.execute(
                            "UPDATE operation SET error = ?2 WHERE id = ?1",
                            params![
                                op_id,
                                format!("Recovery could not prove the held-copy identity: {error}")
                            ],
                        )?;
                        continue;
                    }
                }
            }
            None => false,
        };
        let held_absent = match held_path.as_deref() {
            Some(held) => match path_provably_absent(Path::new(held)) {
                Ok(absent) => absent,
                Err(error) => {
                    conn.execute(
                        "UPDATE operation SET error = ?2 WHERE id = ?1",
                        params![
                            op_id,
                            format!("Recovery could not prove the held copy absent: {error}")
                        ],
                    )?;
                    continue;
                }
            },
            None => true,
        };
        // Belt-and-suspenders for the one residual provable-absence cannot cover: a cleanly
        // unmounted volume whose held path returns NotFound is indistinguishable from a real
        // absence. When the entry recorded a content hash, require the destination to hash to
        // it before finalizing 'restored'; a crash mid-copy leaves a truncated destination
        // that fails this check and stays fail-closed instead of enshrining corruption. With
        // no recorded hash, provable-absence carries the decision alone.
        let destination_content_ok = destination_present;
        let resolved = match entry_id {
            None => {
                // Malformed/legacy restore op — finalise so it cannot wedge recovery.
                conn.execute(
                    "UPDATE operation SET status = 'rolled_back', finished_at = ?2 WHERE id = ?1",
                    params![op_id, now],
                )?;
                true
            }
            Some(eid) if destination_present && held_absent && destination_content_ok => {
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
            Some(eid) if destination_occupied && !held_present && !destination_content_ok => {
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

fn load_delete_proof(
    conn: &Connection,
    quarantine_path: &str,
) -> Result<Option<(i64, i64, FileStamp, String)>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT o.id, oi.id, oi.expected_volume_id, oi.expected_file_id,
                    oi.bytes, COALESCE(oi.expected_blake3, oi.checksum_before)
             FROM operation_item oi
             JOIN operation o ON o.id = oi.operation_id
             WHERE o.kind = 'permanent_delete'
               AND o.status = 'executing'
               AND oi.action = 'delete_bound'
               AND oi.from_path = ?1
             ORDER BY oi.id DESC LIMIT 1",
            [quarantine_path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    Ok(row.and_then(|(op, item, volume, file, bytes, hash)| {
        Some((op, item, stamp_from_columns(volume, file, bytes)?, hash?))
    }))
}

fn load_restore_proof(
    conn: &Connection,
    operation_id: i64,
) -> Result<Option<RestoreJournalProof>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT expected_volume_id, expected_file_id, bytes,
                    COALESCE(expected_blake3, checksum_before),
                    result_volume_id, result_file_id,
                    COALESCE(result_blake3, checksum_after)
             FROM operation_item
             WHERE operation_id = ?1
               AND action IN ('restore_bound', 'restore_move_bound', 'restore_copy_delete_bound', 'move', 'copy_delete')
             ORDER BY id DESC LIMIT 1",
            [operation_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    Ok(row.map(
        |(
            expected_volume,
            expected_file,
            bytes,
            expected_hash,
            result_volume,
            result_file,
            result_hash,
        )| RestoreJournalProof {
            expected: stamp_from_columns(expected_volume, expected_file, bytes),
            expected_hash,
            // Never synthesize a destination identity from the source. That shortcut is
            // invalid for a cross-volume CREATE_NEW copy and can misclassify its crash
            // window as a completed rollback.
            result: stamp_from_columns(result_volume, result_file, bytes),
            result_hash,
        },
    ))
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

    fn open_reviewed_prefer(primary: &Path, fallback: &Path) -> crate::bound_fs::BoundFile {
        match crate::bound_fs::BoundFile::open_read(primary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::bound_fs::BoundFile::open_read(fallback).unwrap()
            }
            Err(error) => panic!("safe fixture handle proof failed: {error}"),
        }
    }

    fn record_move_proof(
        conn: &Connection,
        operation_id: i64,
        expected_path: &Path,
        result_path: &Path,
        expected_hash_override: Option<&str>,
    ) {
        let mut expected_file = open_reviewed_prefer(expected_path, result_path);
        let expected_stamp = expected_file.stamp().clone();
        let expected_hash = expected_hash_override
            .map(str::to_string)
            .unwrap_or_else(|| expected_file.hash().unwrap());
        let result_file = open_reviewed_prefer(result_path, expected_path);
        let result_stamp = result_file.stamp().clone();
        let updated = conn
            .execute(
                "UPDATE operation_item
                 SET bytes = ?2, checksum_before = ?3, checksum_after = ?3,
                     expected_volume_id = ?4, expected_file_id = ?5, expected_blake3 = ?3,
                     result_volume_id = ?6, result_file_id = ?7, result_blake3 = ?3
                 WHERE operation_id = ?1",
                params![
                    operation_id,
                    expected_stamp.bytes as i64,
                    expected_hash,
                    expected_stamp.volume_id,
                    expected_stamp.file_id,
                    result_stamp.volume_id,
                    result_stamp.file_id,
                ],
            )
            .unwrap();
        if updated == 0 {
            conn.execute(
                "INSERT INTO operation_item(
                    operation_id, action, from_path, to_path, bytes,
                    checksum_before, checksum_after,
                    expected_volume_id, expected_file_id, expected_blake3,
                    result_volume_id, result_file_id, result_blake3, status
                 ) VALUES(?1, 'restore_bound', ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?5, ?8, ?9, ?5, 'pending')",
                params![
                    operation_id,
                    expected_path.to_string_lossy(),
                    result_path.to_string_lossy(),
                    expected_stamp.bytes as i64,
                    expected_hash,
                    expected_stamp.volume_id,
                    expected_stamp.file_id,
                    result_stamp.volume_id,
                    result_stamp.file_id,
                ],
            )
            .unwrap();
        }
    }

    fn record_missing_move_proof(
        conn: &Connection,
        operation_id: i64,
        expected_path: &Path,
        result_path: &Path,
    ) {
        let hash = blake3::hash(b"missing reviewed payload")
            .to_hex()
            .to_string();
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, to_path, bytes,
                checksum_before, checksum_after,
                expected_volume_id, expected_file_id, expected_blake3,
                result_volume_id, result_file_id, result_blake3, status
             ) VALUES(?1, 'restore_bound', ?2, ?3, 24, ?4, ?4,
                      'missing-volume', 'missing-file', ?4,
                      'missing-volume', 'missing-file', ?4, 'pending')",
            params![
                operation_id,
                expected_path.to_string_lossy(),
                result_path.to_string_lossy(),
                hash,
            ],
        )
        .unwrap();
    }

    fn record_pending_without_result(
        conn: &Connection,
        operation_id: i64,
        action: &str,
        expected_path: &Path,
        result_path: &Path,
    ) {
        let mut expected_file = crate::bound_fs::BoundFile::open_read(expected_path).unwrap();
        let expected_stamp = expected_file.stamp().clone();
        let expected_hash = expected_file.hash().unwrap();
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, to_path, bytes, checksum_before,
                expected_volume_id, expected_file_id, expected_blake3, status
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?6, 'pending')",
            params![
                operation_id,
                action,
                expected_path.to_string_lossy(),
                result_path.to_string_lossy(),
                expected_stamp.bytes as i64,
                expected_hash,
                expected_stamp.volume_id,
                expected_stamp.file_id,
            ],
        )
        .unwrap();
    }

    fn record_delete_proof(
        conn: &Connection,
        operation_id: i64,
        path: &Path,
        stamp: &FileStamp,
        hash: &str,
    ) {
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, bytes, checksum_before,
                expected_volume_id, expected_file_id, expected_blake3, status
             ) VALUES(?1, 'delete_bound', ?2, ?3, ?4, ?5, ?6, ?4, 'pending')",
            params![
                operation_id,
                path.to_string_lossy(),
                stamp.bytes as i64,
                hash,
                stamp.volume_id,
                stamp.file_id,
            ],
        )
        .unwrap();
    }

    struct BatchRecoveryFixture {
        operation_id: i64,
        batch_id: i64,
        operation_item_id: i64,
        batch_item_id: i64,
        entry_id: i64,
        held_path: std::path::PathBuf,
    }

    #[derive(Clone, Copy)]
    enum PendingArchiveLocation {
        Partial,
        Final,
    }

    struct PendingArchiveRecoveryFixture {
        operation_id: i64,
        batch_id: i64,
        batch_item_id: i64,
        entry_id: i64,
        held_path: std::path::PathBuf,
        partial_path: std::path::PathBuf,
        final_path: std::path::PathBuf,
    }

    fn stamp_and_hash(path: &Path) -> (FileStamp, String) {
        let mut bound = crate::bound_fs::BoundFile::open_read(path).unwrap();
        let stamp = bound.stamp().clone();
        let hash = bound.hash().unwrap();
        (stamp, hash)
    }

    fn seed_pending_archive_recovery(
        conn: &Connection,
        root: &Path,
        location: PendingArchiveLocation,
    ) -> PendingArchiveRecoveryFixture {
        let held_path = root.join("holding/project/render.bin");
        let archive_root = root.join("archive-root");
        fs::create_dir_all(held_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&archive_root).unwrap();
        fs::write(&held_path, b"held object must never be auto-deleted").unwrap();
        let (held_stamp, held_hash) = stamp_and_hash(&held_path);

        conn.execute(
            "INSERT INTO backup(level, destination, manifest_path, verified, created_at)
             VALUES('full', ?1, ?2, 1, '2026-08-24T00:00:00Z')",
            params![
                archive_root.to_string_lossy(),
                root.join("manifest.json").to_string_lossy()
            ],
        )
        .unwrap();
        let backup_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('quarantine', 'done', '{}', '2026-08-24T00:00:00Z')",
            [],
        )
        .unwrap();
        let quarantine_operation_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO quarantine_entry(
                operation_id, original_path, quarantine_path, size, backup_id,
                status, manifest_json, removal_group_id
             ) VALUES(?1, ?2, ?3, ?4, ?5, 'quarantined', '{}', 'project:test')",
            params![
                quarantine_operation_id,
                root.join("project/render.bin").to_string_lossy(),
                held_path.to_string_lossy(),
                held_stamp.bytes as i64,
                backup_id,
            ],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('permanent_delete_batch', 'waiting_for_uac', '{}',
                    '2026-08-24T00:00:00Z')",
            [],
        )
        .unwrap();
        let operation_id = conn.last_insert_rowid();
        let nonce = "8a".repeat(32);
        let nonce_digest = blake3::hash(nonce.as_bytes()).to_hex().to_string();
        conn.execute(
            "INSERT INTO elevation_capability(
                operation_id, request_digest, transport_nonce, nonce_digest,
                status, issued_at
             ) VALUES(?1, ?2, ?3, ?4, 'issued', '2026-08-24T00:00:00Z')",
            params![operation_id, "71".repeat(32), nonce, nonce_digest],
        )
        .unwrap();
        let elevation_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, bytes, expected_volume_id,
                expected_file_id, expected_blake3,
                expected_modified_unix_seconds, status
             ) VALUES(?1, 'final_remove_bound', ?2, ?3, ?4, ?5, ?6, ?7, 'pending')",
            params![
                operation_id,
                held_path.to_string_lossy(),
                held_stamp.bytes as i64,
                held_stamp.volume_id,
                held_stamp.file_id,
                held_hash,
                held_stamp.modified_unix_seconds,
            ],
        )
        .unwrap();
        let operation_item_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO permanent_delete_batch(
                public_id, operation_id, preview_id, preview_digest,
                selected_groups_json, requested_count, status, created_at
             ) VALUES('batch-pending-archive', ?1, 'preview-pending', ?2,
                      '[\"topology:test\"]', 1, 'waiting_for_uac',
                      '2026-08-24T00:00:00Z')",
            params![operation_id, format!("v2:{}", "ab".repeat(32))],
        )
        .unwrap();
        let batch_id = conn.last_insert_rowid();

        let capability_index = 7u32;
        let partial_path =
            crate::archive_path_for_capability(&archive_root, &nonce, capability_index);
        let final_path = archive_root
            .join(crate::OBJECT_ARCHIVE_DIRECTORY_NAME)
            .join(format!("entry-{entry_id:016x}.chobj"));
        let archive_path = match location {
            PendingArchiveLocation::Partial => &partial_path,
            PendingArchiveLocation::Final => &final_path,
        };
        fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        fs::write(archive_path, b"complete synthetic object_archive/2 payload").unwrap();
        let (archive_stamp, archive_hash) = stamp_and_hash(archive_path);
        let semantic = "34".repeat(32);
        conn.execute(
            "INSERT INTO permanent_delete_batch_item(
                batch_id, operation_item_id, quarantine_entry_id,
                removal_group_id, topology_group_id, held_path,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds, expected_content_blake3,
                logical_bytes, elevation_capability_id, capability_index,
                archive_partial_path, archive_initial_volume_id,
                archive_initial_file_id, archive_initial_bytes,
                archive_initial_modified_unix_seconds, archive_final_path,
                archive_proof_volume_id, archive_proof_file_id,
                archive_proof_bytes, archive_proof_modified_unix_seconds,
                archive_proof_blake3, archive_raw_backup_blake3,
                archive_semantic_blake3, archive_roundtrip_blake3,
                archive_stream_count, archive_security_stream_present,
                archive_cleanup_complete, archive_proof_schema,
                phase, status, created_at, updated_at
             ) VALUES(?1, ?2, ?3, 'project:test', 'topology:test', ?4,
                      ?5, ?6, ?7, ?8, ?9, ?7, ?10, ?11,
                      ?12, ?13, ?14, ?15, ?16, ?17,
                      ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?24,
                      1, 1, 1, ?25, 'archive_proof_persisted', 'planned',
                      '2026-08-24T00:00:00Z', '2026-08-24T00:00:00Z')",
            params![
                batch_id,
                operation_item_id,
                entry_id,
                held_path.to_string_lossy(),
                held_stamp.volume_id,
                held_stamp.file_id,
                held_stamp.bytes as i64,
                held_stamp.modified_unix_seconds,
                held_hash,
                elevation_id,
                capability_index as i64,
                partial_path.to_string_lossy(),
                archive_stamp.volume_id,
                archive_stamp.file_id,
                archive_stamp.bytes as i64,
                archive_stamp.modified_unix_seconds,
                final_path.to_string_lossy(),
                archive_stamp.volume_id,
                archive_stamp.file_id,
                archive_stamp.bytes as i64,
                archive_stamp.modified_unix_seconds,
                archive_hash,
                "56".repeat(32),
                semantic,
                crate::OBJECT_ARCHIVE_PROOF_SCHEMA,
            ],
        )
        .unwrap();
        let batch_item_id = conn.last_insert_rowid();
        PendingArchiveRecoveryFixture {
            operation_id,
            batch_id,
            batch_item_id,
            entry_id,
            held_path,
            partial_path,
            final_path,
        }
    }

    fn seed_batch_delete_recovery(
        conn: &Connection,
        root: &Path,
        item_status: &str,
        entry_status: &str,
        operation_item_status: &str,
    ) -> BatchRecoveryFixture {
        let held_path = root.join("holding/project/render.bin");
        let archive_path = root.join("archive/entry-1.chobj");
        fs::create_dir_all(held_path.parent().unwrap()).unwrap();
        fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        fs::write(&held_path, b"exact held object").unwrap();
        fs::write(&archive_path, b"synthetic object_archive/2 payload").unwrap();
        let (held_stamp, held_hash) = stamp_and_hash(&held_path);
        let (archive_stamp, archive_hash) = stamp_and_hash(&archive_path);

        conn.execute(
            "INSERT INTO backup(level, destination, manifest_path, verified, created_at)
             VALUES('full', ?1, ?2, 1, '2026-08-23T00:00:00Z')",
            params![
                root.join("archive").to_string_lossy(),
                root.join("manifest.json").to_string_lossy()
            ],
        )
        .unwrap();
        let backup_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('quarantine', 'done', '{}', '2026-08-23T00:00:00Z')",
            [],
        )
        .unwrap();
        let quarantine_operation_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO quarantine_entry(
                operation_id, original_path, quarantine_path, size, backup_id,
                status, manifest_json, removal_group_id
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, '{}', 'project:test')",
            params![
                quarantine_operation_id,
                root.join("project/render.bin").to_string_lossy(),
                held_path.to_string_lossy(),
                held_stamp.bytes as i64,
                backup_id,
                entry_status,
            ],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        let semantic = "34".repeat(32);
        conn.execute(
            "INSERT INTO object_archive(
                backup_id, quarantine_entry_id, removal_group_id, original_path,
                held_path, held_volume_id, held_file_id, held_bytes,
                held_modified_unix_seconds, held_content_blake3,
                archive_path, source_volume_id, source_file_id, source_bytes,
                source_modified_unix_seconds, source_content_blake3,
                archive_volume_id, archive_file_id, archive_bytes,
                archive_modified_unix_seconds, archive_blake3, raw_backup_blake3,
                semantic_blake3, roundtrip_blake3, stream_count,
                security_stream_present, cleanup_complete, proof_schema, status, verified_at
             ) VALUES(?1, ?2, 'project:test', ?3,
                      ?4, ?5, ?6, ?7, ?8, ?9,
                      ?10, ?5, ?6, ?7, ?8, ?9,
                      ?11, ?12, ?13, ?14, ?15, ?16,
                      ?17, ?17, 1, 1, 1, ?18, 'ready', '2026-08-23T00:00:00Z')",
            params![
                backup_id,
                entry_id,
                root.join("project/render.bin").to_string_lossy(),
                held_path.to_string_lossy(),
                held_stamp.volume_id,
                held_stamp.file_id,
                held_stamp.bytes as i64,
                held_stamp.modified_unix_seconds,
                held_hash,
                archive_path.to_string_lossy(),
                archive_stamp.volume_id,
                archive_stamp.file_id,
                archive_stamp.bytes as i64,
                archive_stamp.modified_unix_seconds,
                archive_hash,
                "56".repeat(32),
                semantic,
                crate::OBJECT_ARCHIVE_PROOF_SCHEMA,
            ],
        )
        .unwrap();
        let archive_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('permanent_delete_batch', 'executing', '{}', '2026-08-23T00:00:00Z')",
            [],
        )
        .unwrap();
        let operation_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, bytes, expected_volume_id,
                expected_file_id, expected_blake3, status
             ) VALUES(?1, 'final_remove_bound', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                operation_id,
                held_path.to_string_lossy(),
                held_stamp.bytes as i64,
                held_stamp.volume_id,
                held_stamp.file_id,
                held_hash,
                operation_item_status,
            ],
        )
        .unwrap();
        let operation_item_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO permanent_delete_batch(
                public_id, operation_id, preview_id, preview_digest,
                selected_groups_json, requested_count, status, created_at
             ) VALUES('batch-recovery', ?1, 'preview-recovery', ?2,
                      '[\"topology:test\"]', 1, 'executing', '2026-08-23T00:00:00Z')",
            params![operation_id, format!("v2:{}", "ab".repeat(32))],
        )
        .unwrap();
        let batch_id = conn.last_insert_rowid();
        let phase = if item_status == "ready" {
            "archive_ready"
        } else if item_status == "deleted" {
            "finished"
        } else {
            "parent_disposition"
        };
        conn.execute(
            "INSERT INTO permanent_delete_batch_item(
                batch_id, operation_item_id, quarantine_entry_id, archive_id,
                removal_group_id, topology_group_id, held_path,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds, expected_content_blake3,
                logical_bytes, phase, status, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, 'project:test', 'topology:test', ?5,
                      ?6, ?7, ?8, ?9, ?10, ?8, ?11, ?12,
                      '2026-08-23T00:00:00Z', '2026-08-23T00:00:00Z')",
            params![
                batch_id,
                operation_item_id,
                entry_id,
                archive_id,
                held_path.to_string_lossy(),
                held_stamp.volume_id,
                held_stamp.file_id,
                held_stamp.bytes as i64,
                held_stamp.modified_unix_seconds,
                held_hash,
                phase,
                item_status,
            ],
        )
        .unwrap();
        let batch_item_id = conn.last_insert_rowid();
        BatchRecoveryFixture {
            operation_id,
            batch_id,
            operation_item_id,
            batch_item_id,
            entry_id,
            held_path,
        }
    }

    fn seed_guardian_recovery_state(
        conn: &Connection,
        fixture: &BatchRecoveryFixture,
        state: &str,
        disposition_mode: Option<&str>,
        reason_code: &str,
    ) {
        let (volume_id, file_id, bytes, modified): (String, String, i64, i64) = conn
            .query_row(
                "SELECT expected_volume_id, expected_file_id, expected_bytes,
                        expected_modified_unix_seconds
                 FROM permanent_delete_batch_item WHERE id = ?1",
                [fixture.batch_item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO final_disposition_guardian(
                batch_item_id, operation_id, nonce_digest, guardian_pid,
                guardian_started_100ns, guardian_image_sha256,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds, state, disposition_mode,
                created_at, updated_at
             ) VALUES(?1, ?2, ?3, 4294967295, 99, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                      '2026-08-27T00:00:00Z', '2026-08-27T00:00:00Z')",
            params![
                fixture.batch_item_id,
                fixture.operation_id,
                "91".repeat(32),
                "92".repeat(32),
                volume_id,
                file_id,
                bytes,
                modified,
                state,
                disposition_mode,
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE permanent_delete_batch_item SET reason_code = ?2 WHERE id = ?1",
            params![fixture.batch_item_id, reason_code],
        )
        .unwrap();
    }

    #[cfg(windows)]
    fn seed_guardian_close_receipt(
        conn: &Connection,
        fixture: &BatchRecoveryFixture,
        state: &str,
        disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode,
        durably_write: bool,
    ) -> PathBuf {
        seed_guardian_close_receipt_for_identity(
            conn,
            fixture,
            state,
            disposition_mode,
            durably_write,
            u32::MAX,
            99,
            "92".repeat(32),
        )
    }

    #[cfg(windows)]
    #[allow(clippy::too_many_arguments)]
    fn seed_guardian_close_receipt_for_identity(
        conn: &Connection,
        fixture: &BatchRecoveryFixture,
        state: &str,
        disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode,
        durably_write: bool,
        guardian_pid: u32,
        guardian_started_100ns: u64,
        guardian_image_sha256: String,
    ) -> PathBuf {
        let (target_stamp, _) = crate::bound_fs::inspect_local_mutation_file(&fixture.held_path)
            .expect("held fixture must remain bound before receipt creation");
        let expectation = crate::elevated_transport::GuardianCloseReceiptExpectation {
            operation_id: fixture.operation_id,
            batch_item_id: fixture.batch_item_id,
            nonce_digest: "91".repeat(32),
            guardian_pid,
            guardian_started_100ns,
            guardian_image_sha256: guardian_image_sha256.clone(),
            target_stamp,
            disposition_mode,
        };
        seed_guardian_close_receipt_with_expectation(
            conn,
            fixture,
            state,
            &expectation,
            durably_write,
        )
    }

    #[cfg(windows)]
    fn seed_guardian_close_receipt_with_expectation(
        conn: &Connection,
        fixture: &BatchRecoveryFixture,
        state: &str,
        expectation: &crate::elevated_transport::GuardianCloseReceiptExpectation,
        durably_write: bool,
    ) -> PathBuf {
        seed_guardian_recovery_state(
            conn,
            fixture,
            state,
            Some(expectation.disposition_mode.journal_label()),
            crate::final_remove::PROVED_FINAL_PROFILE_REASON_CODE,
        );
        let path = fixture
            .held_path
            .parent()
            .unwrap()
            .join(format!("guardian-receipt-{}.bin", fixture.batch_item_id));
        let authority = crate::elevated_transport::create_guardian_receipt_fixture(
            &path,
            expectation,
            durably_write,
        )
        .unwrap();
        conn.execute(
            "UPDATE final_disposition_guardian
             SET receipt_path = ?2, receipt_volume_id = ?3,
                 receipt_file_id = ?4, receipt_key_dpapi = ?5,
                 receipt_cleanup_complete = 0,
                 guardian_pid = ?6, guardian_started_100ns = ?7,
                 guardian_image_sha256 = ?8,
                 expected_volume_id = ?9, expected_file_id = ?10,
                 expected_bytes = ?11, expected_modified_unix_seconds = ?12
             WHERE batch_item_id = ?1",
            params![
                fixture.batch_item_id,
                authority.path.to_string_lossy(),
                authority.initial_stamp.volume_id,
                authority.initial_stamp.file_id,
                authority.protected_key_hex,
                i64::from(expectation.guardian_pid),
                i64::try_from(expectation.guardian_started_100ns).unwrap(),
                expectation.guardian_image_sha256,
                expectation.target_stamp.volume_id,
                expectation.target_stamp.file_id,
                expectation.target_stamp.bytes as i64,
                expectation.target_stamp.modified_unix_seconds,
            ],
        )
        .unwrap();
        path
    }

    fn batch_recovery_statuses(
        conn: &Connection,
        fixture: &BatchRecoveryFixture,
    ) -> (String, String, String, String, String) {
        conn.query_row(
            "SELECT o.status, b.status, oi.status, bi.status, qe.status
             FROM operation o
             JOIN permanent_delete_batch b ON b.operation_id = o.id
             JOIN permanent_delete_batch_item bi ON bi.batch_id = b.id
             JOIN operation_item oi ON oi.id = bi.operation_item_id
             JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
             WHERE o.id = ?1 AND b.id = ?2 AND bi.id = ?3
               AND oi.id = ?4 AND qe.id = ?5",
            params![
                fixture.operation_id,
                fixture.batch_id,
                fixture.batch_item_id,
                fixture.operation_item_id,
                fixture.entry_id,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn batch_recovery_closes_pre_disposition_without_touching_held_object() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "ready", "quarantined", "pending");

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "failed".into(),
                "failed".into(),
                "skipped".into(),
                "blocked".into(),
                "quarantined".into(),
            )
        );
        assert_eq!(fs::read(&fixture.held_path).unwrap(), b"exact held object");

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(fs::read(&fixture.held_path).unwrap(), b"exact held object");
    }

    #[test]
    fn batch_recovery_rolls_back_durable_intent_when_exact_held_object_remains() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        assert_eq!(first.rolled_back_items, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "failed".into(),
                "failed".into(),
                "rolled_back".into(),
                "blocked".into(),
                "quarantined".into(),
            )
        );
        assert_eq!(fs::read(&fixture.held_path).unwrap(), b"exact held object");

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
    }

    #[test]
    fn batch_recovery_never_settles_absence_without_guardian_receipt_authority() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        fs::remove_file(&fixture.held_path).unwrap();

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first, RecoveryReport::default());
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        let effects: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mutation_space_effect
                 WHERE operation_item_id = ?1 AND lifecycle_stage = 'holding_object_removed'",
                [fixture.operation_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(effects, 0);

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        let effects_after_second_pass: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mutation_space_effect
                 WHERE operation_item_id = ?1 AND lifecycle_stage = 'holding_object_removed'",
                [fixture.operation_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(effects_after_second_pass, 0);
    }

    #[test]
    fn batch_recovery_never_settles_absent_unproved_final_profile_as_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        conn.execute(
            "UPDATE permanent_delete_batch_item SET reason_code = ?2 WHERE id = ?1",
            params![
                fixture.batch_item_id,
                crate::final_remove::UNPROVED_FINAL_PROFILE_REASON_CODE,
            ],
        )
        .unwrap();
        fs::remove_file(&fixture.held_path).unwrap();

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first, RecoveryReport::default());
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        let reason: String = conn
            .query_row(
                "SELECT reason_code FROM permanent_delete_batch_item WHERE id = ?1",
                [fixture.batch_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            reason,
            crate::final_remove::UNPROVED_FINAL_PROFILE_REASON_CODE
        );
        let (archives, effects): (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM object_archive
                     WHERE quarantine_entry_id = ?1 AND status = 'ready'),
                    (SELECT COUNT(*) FROM mutation_space_effect
                     WHERE operation_item_id = ?2
                       AND lifecycle_stage = 'holding_object_removed')",
                params![fixture.entry_id, fixture.operation_item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(archives, 1);
        assert_eq!(effects, 0);

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
    }

    #[test]
    fn batch_recovery_never_settles_absent_before_guardian_close_authority() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        seed_guardian_recovery_state(
            &conn,
            &fixture,
            "final_profile_proved_held",
            Some("extendedOnClose"),
            crate::final_remove::PROVED_FINAL_PROFILE_REASON_CODE,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        let guardian_state: String = conn
            .query_row(
                "SELECT state FROM final_disposition_guardian WHERE batch_item_id = ?1",
                [fixture.batch_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(guardian_state, "final_profile_proved_held");
    }

    #[test]
    fn batch_recovery_refuses_proved_marker_without_its_guardian_journal() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        conn.execute(
            "UPDATE permanent_delete_batch_item SET reason_code = ?2 WHERE id = ?1",
            params![
                fixture.batch_item_id,
                crate::final_remove::PROVED_FINAL_PROFILE_REASON_CODE,
            ],
        )
        .unwrap();
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
    }

    #[cfg(windows)]
    #[test]
    fn batch_recovery_settles_absent_after_guardian_close_authority() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "guardian_handle_closed",
            crate::bound_fs::WindowsDeleteDispositionMode::Legacy,
            true,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "completed".into(),
                "completed".into(),
                "done".into(),
                "deleted".into(),
                "permanently_deleted".into(),
            )
        );
        assert!(
            !receipt_path.exists(),
            "terminal recovery must clean the exact durable receipt"
        );
    }

    #[cfg(windows)]
    #[test]
    fn guardian_receipt_flushed_before_lost_ack_settles_after_exact_guardian_death() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "close_authorized",
            crate::bound_fs::WindowsDeleteDispositionMode::ExtendedOnClose,
            true,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "completed".into(),
                "completed".into(),
                "done".into(),
                "deleted".into(),
                "permanently_deleted".into(),
            )
        );
        assert!(!receipt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn guardian_receipt_not_flushed_before_parent_death_stays_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "close_authorized",
            crate::bound_fs::WindowsDeleteDispositionMode::Legacy,
            false,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        assert!(receipt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn torn_guardian_receipt_after_cancel_does_not_block_exact_object_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "close_authorized",
            crate::bound_fs::WindowsDeleteDispositionMode::Legacy,
            false,
        );

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.rolled_back_items, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "failed".into(),
                "failed".into(),
                "rolled_back".into(),
                "blocked".into(),
                "quarantined".into(),
            )
        );
        assert_eq!(fs::read(&fixture.held_path).unwrap(), b"exact held object");
        assert!(
            !receipt_path.exists(),
            "a safely rolled-back exact object no longer needs its torn receipt"
        );
    }

    #[cfg(windows)]
    #[test]
    fn guardian_live_directory_stamp_rolls_back_exact_parent_after_child_deletion() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        fs::remove_file(&fixture.held_path).unwrap();
        fs::create_dir(&fixture.held_path).unwrap();
        let child = fixture.held_path.join("planned-child.bin");
        fs::write(&child, b"planned child").unwrap();
        let (archive_stamp, archive_hash, directory) =
            crate::bound_fs::inspect_local_mutation_object_for_test(&fixture.held_path).unwrap();
        assert!(directory);
        conn.execute(
            "UPDATE permanent_delete_batch_item
             SET expected_volume_id = ?2, expected_file_id = ?3,
                 expected_bytes = ?4, expected_modified_unix_seconds = ?5,
                 expected_content_blake3 = ?6, logical_bytes = ?4
             WHERE id = ?1",
            params![
                fixture.batch_item_id,
                archive_stamp.volume_id,
                archive_stamp.file_id,
                archive_stamp.bytes as i64,
                archive_stamp.modified_unix_seconds,
                archive_hash,
            ],
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(1_100));
        fs::remove_file(&child).unwrap();
        let (live_stamp, live_hash, directory) =
            crate::bound_fs::inspect_local_mutation_object_for_test(&fixture.held_path).unwrap();
        assert!(directory);
        assert!(live_stamp.same_object(&archive_stamp));
        assert_eq!(live_hash, archive_hash);
        assert!(
            live_stamp.bytes != archive_stamp.bytes
                || live_stamp.modified_unix_seconds != archive_stamp.modified_unix_seconds,
            "real child removal must alter the directory stamp exercised by recovery"
        );

        seed_guardian_recovery_state(
            &conn,
            &fixture,
            "handle_bound",
            None,
            crate::final_remove::UNPROVED_FINAL_PROFILE_REASON_CODE,
        );
        conn.execute(
            "UPDATE final_disposition_guardian
             SET expected_bytes = ?2, expected_modified_unix_seconds = ?3
             WHERE batch_item_id = ?1",
            params![
                fixture.batch_item_id,
                live_stamp.bytes as i64,
                live_stamp.modified_unix_seconds,
            ],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.rolled_back_items, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "failed".into(),
                "failed".into(),
                "rolled_back".into(),
                "blocked".into(),
                "quarantined".into(),
            )
        );
        let (after_stamp, after_hash, directory) =
            crate::bound_fs::inspect_local_mutation_object_for_test(&fixture.held_path).unwrap();
        assert!(directory);
        assert_eq!(after_stamp, live_stamp);
        assert_eq!(after_hash, live_hash);
    }

    #[cfg(windows)]
    #[test]
    fn recovery_authenticates_the_guardians_live_directory_stamp_not_the_prechild_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let (mut live_target_stamp, _) =
            crate::bound_fs::inspect_local_mutation_file(&fixture.held_path).unwrap();
        live_target_stamp.bytes = 0;
        live_target_stamp.modified_unix_seconds = live_target_stamp
            .modified_unix_seconds
            .and_then(|value| value.checked_add(1));
        let expectation = crate::elevated_transport::GuardianCloseReceiptExpectation {
            operation_id: fixture.operation_id,
            batch_item_id: fixture.batch_item_id,
            nonce_digest: "91".repeat(32),
            guardian_pid: u32::MAX,
            guardian_started_100ns: 99,
            guardian_image_sha256: "92".repeat(32),
            target_stamp: live_target_stamp,
            disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode::ExtendedOnClose,
        };
        let receipt_path = seed_guardian_close_receipt_with_expectation(
            &conn,
            &fixture,
            "close_authorized",
            &expectation,
            true,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(batch_recovery_statuses(&conn, &fixture).3, "deleted");
        assert!(!receipt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn legacy_live_duplicate_absence_never_settles_and_cancel_restores_same_object() {
        use std::fs::File;
        use std::os::windows::io::{FromRawHandle as _, RawHandle};
        use windows_sys::Win32::Foundation::{
            DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        seed_guardian_recovery_state(
            &conn,
            &fixture,
            "close_authorized",
            Some("legacy"),
            crate::final_remove::PROVED_FINAL_PROFILE_REASON_CODE,
        );
        let (before_stamp, before_hash) =
            crate::bound_fs::inspect_local_mutation_file(&fixture.held_path).unwrap();
        let proof = BoundObjectProof::open_for_archive_delete(
            &fixture.held_path,
            &before_stamp,
            &before_hash,
        )
        .unwrap()
        .detach_exclusive_for_final_disposition()
        .unwrap();
        let mut duplicate = std::ptr::null_mut();
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                proof.raw_handle_value() as usize as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        assert_ne!(duplicated, 0);
        assert!(!duplicate.is_null() && duplicate != INVALID_HANDLE_VALUE);
        let guardian_duplicate = unsafe { File::from_raw_handle(duplicate as RawHandle) };

        let mode = proof.arm_legacy_final_disposition_for_test().unwrap();
        proof.validate_armed_final_disposition(mode).unwrap();
        assert!(
            BoundObjectProof::open_for_archive(&fixture.held_path, &before_stamp, &before_hash)
                .is_err(),
            "the legacy delete-pending object must be unavailable to recovery rebinding"
        );

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(batch_recovery_statuses(&conn, &fixture).3, "deleting");

        crate::bound_fs::guardian_cancel_delete_on_close(
            &guardian_duplicate,
            Some(crate::bound_fs::WindowsDeleteDispositionMode::Legacy),
        )
        .unwrap();
        assert!(!crate::bound_fs::guardian_delete_pending(&guardian_duplicate).unwrap());
        drop(proof);
        drop(guardian_duplicate);

        let (after_stamp, after_hash) =
            crate::bound_fs::inspect_local_mutation_file(&fixture.held_path).unwrap();
        assert!(after_stamp.same_object(&before_stamp));
        assert_eq!(after_stamp.bytes, before_stamp.bytes);
        assert_eq!(after_hash, before_hash);
    }

    #[cfg(windows)]
    #[test]
    fn authenticated_guardian_receipt_waits_while_exact_guardian_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let live = crate::elevated_transport::current_parent_binding().unwrap();
        let receipt_path = seed_guardian_close_receipt_for_identity(
            &conn,
            &fixture,
            "guardian_handle_closed",
            crate::bound_fs::WindowsDeleteDispositionMode::ExtendedOnClose,
            true,
            live.pid,
            live.process_started_100ns,
            live.image_sha256,
        );
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        assert!(receipt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn tampered_guardian_receipt_never_authorizes_absence() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "guardian_handle_closed",
            crate::bound_fs::WindowsDeleteDispositionMode::Legacy,
            true,
        );
        fs::write(&receipt_path, b"attacker changed the same FileId").unwrap();
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(batch_recovery_statuses(&conn, &fixture).3, "deleting");
        assert!(receipt_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn substituted_guardian_receipt_path_never_authorizes_absence() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        let receipt_path = seed_guardian_close_receipt(
            &conn,
            &fixture,
            "guardian_handle_closed",
            crate::bound_fs::WindowsDeleteDispositionMode::ExtendedOnClose,
            true,
        );
        let displaced = receipt_path.with_extension("displaced");
        fs::rename(&receipt_path, &displaced).unwrap();
        fs::write(&receipt_path, fs::read(&displaced).unwrap()).unwrap();
        fs::remove_file(&fixture.held_path).unwrap();

        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
        assert_eq!(batch_recovery_statuses(&conn, &fixture).3, "deleting");
        assert!(receipt_path.exists());
    }

    #[test]
    fn batch_recovery_rolls_back_exact_object_after_guardian_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "interrupted", "deleting", "failed");
        seed_guardian_recovery_state(
            &conn,
            &fixture,
            "cancelled_safe",
            Some("extendedOnClose"),
            crate::final_remove::UNPROVED_FINAL_PROFILE_REASON_CODE,
        );

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(report.rolled_back_items, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "failed".into(),
                "failed".into(),
                "rolled_back".into(),
                "blocked".into(),
                "quarantined".into(),
            )
        );
        assert_eq!(fs::read(&fixture.held_path).unwrap(), b"exact held object");
    }

    #[test]
    fn batch_recovery_finishes_header_after_success_cas_without_redeleting() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleted", "permanently_deleted", "done");
        fs::remove_file(&fixture.held_path).unwrap();

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "completed".into(),
                "completed".into(),
                "done".into(),
                "deleted".into(),
                "permanently_deleted".into(),
            )
        );
        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
    }

    #[test]
    fn batch_recovery_never_accepts_same_path_replacement_as_deleted_or_original() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_batch_delete_recovery(&conn, dir.path(), "deleting", "deleting", "deleting");
        fs::remove_file(&fixture.held_path).unwrap();
        fs::write(&fixture.held_path, b"replacement at held path").unwrap();

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 0);
        assert_eq!(
            batch_recovery_statuses(&conn, &fixture),
            (
                "interrupted".into(),
                "interrupted".into(),
                "deleting".into(),
                "deleting".into(),
                "deleting".into(),
            )
        );
        assert_eq!(
            fs::read(&fixture.held_path).unwrap(),
            b"replacement at held path"
        );

        let second = recover_interrupted(&conn).unwrap();
        assert_eq!(second.recovered_operations, 0);
        assert_eq!(
            fs::read(&fixture.held_path).unwrap(),
            b"replacement at held path"
        );
    }

    #[test]
    fn batch_recovery_deletes_only_the_exact_journaled_partial_archive() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_pending_archive_recovery(&conn, dir.path(), PendingArchiveLocation::Partial);

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        assert!(!fixture.partial_path.exists());
        assert!(!fixture.final_path.exists());
        assert_eq!(
            fs::read(&fixture.held_path).unwrap(),
            b"held object must never be auto-deleted"
        );
        let statuses: (String, String, String) = conn
            .query_row(
                "SELECT o.status, b.status, bi.status
                 FROM operation o
                 JOIN permanent_delete_batch b ON b.operation_id = o.id
                 JOIN permanent_delete_batch_item bi ON bi.batch_id = b.id
                 WHERE o.id = ?1 AND b.id = ?2 AND bi.id = ?3",
                params![
                    fixture.operation_id,
                    fixture.batch_id,
                    fixture.batch_item_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            statuses,
            ("failed".into(), "failed".into(), "blocked".into())
        );
        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
    }

    #[test]
    fn batch_recovery_reconciles_exact_promoted_archive_without_continuing_delete() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_pending_archive_recovery(&conn, dir.path(), PendingArchiveLocation::Final);

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        assert!(fixture.final_path.exists());
        assert!(!fixture.partial_path.exists());
        assert_eq!(
            fs::read(&fixture.held_path).unwrap(),
            b"held object must never be auto-deleted"
        );
        let archive: (i64, String, String) = conn
            .query_row(
                "SELECT oa.id, oa.archive_path, oa.status
                 FROM object_archive oa
                 JOIN permanent_delete_batch_item bi ON bi.archive_id = oa.id
                 WHERE bi.id = ?1 AND oa.quarantine_entry_id = ?2",
                params![fixture.batch_item_id, fixture.entry_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(archive.0 > 0);
        assert_eq!(Path::new(&archive.1), fixture.final_path);
        assert_eq!(archive.2, "ready");
        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
    }

    #[test]
    fn batch_recovery_finds_promoted_archive_even_after_terminal_error_labels() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_pending_archive_recovery(&conn, dir.path(), PendingArchiveLocation::Final);
        conn.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'blocked', status = 'blocked',
                 reason_code = 'helperFailed'
             WHERE id = ?1",
            [fixture.batch_item_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE operation_item SET status = 'skipped'
             WHERE id = (
               SELECT operation_item_id FROM permanent_delete_batch_item WHERE id = ?1
             )",
            [fixture.batch_item_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE permanent_delete_batch SET status = 'failed' WHERE id = ?1",
            [fixture.batch_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE operation SET status = 'failed' WHERE id = ?1",
            [fixture.operation_id],
        )
        .unwrap();

        let first = recover_interrupted(&conn).unwrap();
        assert_eq!(first.recovered_operations, 1);
        let archive_id: Option<i64> = conn
            .query_row(
                "SELECT archive_id FROM permanent_delete_batch_item WHERE id = ?1",
                [fixture.batch_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(archive_id.is_some_and(|id| id > 0));
        assert!(fixture.final_path.exists());
        assert_eq!(
            fs::read(&fixture.held_path).unwrap(),
            b"held object must never be auto-deleted"
        );
        assert_eq!(
            recover_interrupted(&conn).unwrap(),
            RecoveryReport::default()
        );
    }

    #[test]
    fn batch_recovery_preserves_same_path_partial_replacement_and_stays_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_pending_archive_recovery(&conn, dir.path(), PendingArchiveLocation::Partial);
        fs::remove_file(&fixture.partial_path).unwrap();
        fs::write(&fixture.partial_path, b"attacker replacement").unwrap();

        for _ in 0..2 {
            let report = recover_interrupted(&conn).unwrap();
            assert_eq!(report.recovered_operations, 0);
            assert_eq!(
                fs::read(&fixture.partial_path).unwrap(),
                b"attacker replacement"
            );
            assert_eq!(
                fs::read(&fixture.held_path).unwrap(),
                b"held object must never be auto-deleted"
            );
        }
        let statuses: (String, String, String) = conn
            .query_row(
                "SELECT o.status, b.status, bi.status
                 FROM operation o
                 JOIN permanent_delete_batch b ON b.operation_id = o.id
                 JOIN permanent_delete_batch_item bi ON bi.batch_id = b.id
                 WHERE o.id = ?1 AND b.id = ?2 AND bi.id = ?3",
                params![
                    fixture.operation_id,
                    fixture.batch_id,
                    fixture.batch_item_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            statuses,
            ("interrupted".into(), "interrupted".into(), "planned".into())
        );
    }

    #[test]
    fn batch_recovery_never_blind_deletes_scratch_cleanup_pending() {
        let dir = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let fixture =
            seed_pending_archive_recovery(&conn, dir.path(), PendingArchiveLocation::Partial);
        fs::remove_file(&fixture.partial_path).unwrap();
        conn.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'scratch_cleanup_pending', status = 'interrupted',
                 reason_code = 'scratchCleanupPending',
                 archive_proof_volume_id = NULL,
                 archive_proof_file_id = NULL,
                 archive_proof_bytes = NULL,
                 archive_proof_modified_unix_seconds = NULL,
                 archive_proof_blake3 = NULL,
                 archive_raw_backup_blake3 = NULL,
                 archive_semantic_blake3 = NULL,
                 archive_roundtrip_blake3 = NULL,
                 archive_stream_count = NULL,
                 archive_security_stream_present = NULL,
                 archive_cleanup_complete = NULL,
                 archive_proof_schema = NULL
             WHERE id = ?1",
            [fixture.batch_item_id],
        )
        .unwrap();
        let archive_root = fixture
            .partial_path
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let nonce = "8a".repeat(32);
        let scratch_path = archive_root.join(crate::scratch_leaf_for_capability(&nonce, 7));
        fs::write(&scratch_path, b"unproved scratch replacement or residue").unwrap();
        conn.execute(
            "INSERT INTO archive_cleanup(
                operation_id, scratch_path, status, created_at, error
             ) VALUES(?1, ?2, 'pending_identity_proof',
                      '2026-08-24T00:00:00Z', 'cleanup failed')",
            params![fixture.operation_id, scratch_path.to_string_lossy()],
        )
        .unwrap();

        for _ in 0..2 {
            let report = recover_interrupted(&conn).unwrap();
            assert_eq!(report.recovered_operations, 0);
            assert_eq!(
                fs::read(&scratch_path).unwrap(),
                b"unproved scratch replacement or residue"
            );
            assert_eq!(
                fs::read(&fixture.held_path).unwrap(),
                b"held object must never be auto-deleted"
            );
        }
        let statuses: (String, String, String) = conn
            .query_row(
                "SELECT o.status, b.status, bi.status
                 FROM operation o
                 JOIN permanent_delete_batch b ON b.operation_id = o.id
                 JOIN permanent_delete_batch_item bi ON bi.batch_id = b.id
                 WHERE o.id = ?1 AND b.id = ?2 AND bi.id = ?3",
                params![
                    fixture.operation_id,
                    fixture.batch_id,
                    fixture.batch_item_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            statuses,
            (
                "interrupted".into(),
                "interrupted".into(),
                "interrupted".into(),
            )
        );
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
        record_move_proof(&conn, op_id, &original, &quarantined, None);
        conn.execute(
            "INSERT INTO operation_item(operation_id, action, from_path, status)
             VALUES(?1, 'remove_dir', ?2, 'done')",
            params![op_id, original.parent().unwrap().to_string_lossy()],
        )
        .unwrap();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(report.rolled_back_items, 2);

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
        let cleanup_status: String = conn
            .query_row(
                "SELECT status FROM operation_item
                 WHERE operation_id = ?1 AND action = 'remove_dir'",
                [op_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cleanup_status, "rolled_back");
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
        record_move_proof(&conn, op_id, &original, &quarantined, None);

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
    fn pending_copy_with_destination_but_no_result_identity_stays_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("project/output/render.bin");
        let held = dir.path().join("holding/op-1/output/render.bin");
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::create_dir_all(held.parent().unwrap()).unwrap();
        fs::write(&original, b"same preserved payload").unwrap();
        fs::write(&held, b"same preserved payload").unwrap();

        let conn = journaled_conn();
        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at)
             VALUES('quarantine', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        record_pending_without_result(&conn, op_id, "copy_delete", &original, &held);

        for _ in 0..2 {
            let report = recover_interrupted(&conn).unwrap();
            assert_eq!(report.recovered_operations, 0);
            assert_eq!(report.rolled_back_items, 0);
        }
        let operation: (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let item_status: String = conn
            .query_row(
                "SELECT status FROM operation_item WHERE operation_id = ?1",
                [op_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation.0, "executing");
        assert!(operation
            .1
            .as_deref()
            .is_some_and(|error| error.contains("without a matching same-volume object identity")));
        assert_eq!(item_status, "pending");
        assert_eq!(fs::read(original).unwrap(), b"same preserved payload");
        assert_eq!(fs::read(held).unwrap(), b"same preserved payload");
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
        fs::write(&original, b"same verified payload").unwrap();
        fs::write(&held, b"same verified payload").unwrap();

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
        record_move_proof(&conn, op_id, &original, &held, None);

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert!(original.exists());
        assert!(held.exists());
        assert_eq!(fs::read(&original).unwrap(), b"same verified payload");
        assert_eq!(fs::read(&held).unwrap(), b"same verified payload");

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
        record_move_proof(&conn, op_id, &original, &held, None);
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
    fn zero_item_legacy_restore_closes_failed_without_claiming_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(held.parent().unwrap()).unwrap();
        fs::write(&held, b"still safely quarantined").unwrap();

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
             VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            [plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();

        let report = recover_interrupted(&conn).unwrap();
        assert_eq!(report.recovered_operations, 1);
        assert_eq!(report.rolled_back_items, 0);
        let operation: (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
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
        assert_eq!(operation.1.as_deref(), Some(RESTORE_ZERO_ITEM_MANUAL));
        assert_eq!(entry_status, "quarantined");
        assert_eq!(fs::read(held).unwrap(), b"still safely quarantined");
        assert!(!destination.exists());
    }

    #[test]
    fn restore_pending_copy_without_result_identity_never_becomes_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("project/output/render.bin");
        let held = dir.path().join("quarantine/output/render.bin");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::create_dir_all(held.parent().unwrap()).unwrap();
        fs::write(&destination, b"copied but not journaled").unwrap();
        fs::write(&held, b"copied but not journaled").unwrap();

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
             VALUES('restore', 'executing', ?1, '2026-01-01T00:00:00Z')",
            [plan],
        )
        .unwrap();
        let op_id = conn.last_insert_rowid();
        record_pending_without_result(&conn, op_id, "restore_bound", &held, &destination);

        for _ in 0..2 {
            let report = recover_interrupted(&conn).unwrap();
            assert_eq!(report.recovered_operations, 0);
            assert_eq!(report.rolled_back_items, 0);
        }
        let operation: (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM operation WHERE id = ?1",
                [op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let item_status: String = conn
            .query_row(
                "SELECT status FROM operation_item WHERE operation_id = ?1",
                [op_id],
                |row| row.get(0),
            )
            .unwrap();
        let entry_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [entry_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation.0, "verifying");
        assert_eq!(
            operation.1.as_deref(),
            Some(RESTORE_RESULT_IDENTITY_MISSING)
        );
        assert_eq!(item_status, "pending");
        assert_eq!(entry_status, "quarantined");
        assert_eq!(fs::read(destination).unwrap(), b"copied but not journaled");
        assert_eq!(fs::read(held).unwrap(), b"copied but not journaled");
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
        record_move_proof(&conn, op_id, &quarantined, &destination, None);

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
        let mut present_file = crate::bound_fs::BoundFile::open_read(&held_present).unwrap();
        let present_stamp = present_file.stamp().clone();
        let present_hash = present_file.hash().unwrap();
        drop(present_file);
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
        fs::write(&held_gone, b"deleted before final journal update").unwrap();
        let mut gone_file = crate::bound_fs::BoundFile::open_read(&held_gone).unwrap();
        let gone_stamp = gone_file.stamp().clone();
        let gone_hash = gone_file.hash().unwrap();
        drop(gone_file);
        fs::remove_file(&held_gone).unwrap();
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
        let present_op_id = conn.last_insert_rowid();
        record_delete_proof(
            &conn,
            present_op_id,
            &held_present,
            &present_stamp,
            &present_hash,
        );

        conn.execute(
            "INSERT INTO operation(kind, status, plan_json, created_at) VALUES('permanent_delete', 'executing', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let gone_op_id = conn.last_insert_rowid();
        record_delete_proof(&conn, gone_op_id, &held_gone, &gone_stamp, &gone_hash);

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

        let operation_status = |id: i64| -> String {
            conn.query_row("SELECT status FROM operation WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(operation_status(present_op_id), "rolled_back");
        assert_eq!(operation_status(gone_op_id), "done");
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
        record_move_proof(&conn, op_id, &quarantined, &destination, None);

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
        record_move_proof(&conn, op_id, &quarantined, &destination, None);

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
        record_move_proof(&conn, op_id, &held, &destination, Some(&expected));

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
        record_move_proof(&conn, op_id, &held, &destination, Some(&expected));

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
        record_missing_move_proof(&conn, op_id, &held, &destination);

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
        let op_id = conn.last_insert_rowid();
        record_move_proof(&conn, op_id, &shared_q, &b_dest, None);

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
        let held = dir.path().join("holding/render.bin");
        record_move_proof(&conn, op_id, &held, &destination, None);

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
        record_missing_move_proof(&conn, op_id, &held, &destination);

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
