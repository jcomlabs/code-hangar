use chrono::{DateTime, Utc};
use hangar_core::{
    display_path_for_path, normalize_path, AdapterSummary, Comment, ContextFile, DashboardSummary,
    DocumentSearchResult, DuplicateCandidates, DuplicateConfirmation, ExportResult, FilePreview,
    FocusedWatcherStatus, FolderExplanation, FolderInvestigation, GitRepoSummary, GraphMap,
    InvestigationHandle, LostProjectCandidates, MutationActivityLog, MutationBackupSummary,
    MutationFinalRemoveSummary, MutationLockInspection, MutationMoveSummary,
    MutationProtectedPreview, MutationRestoreSummary, MutationTokenResult, NavChildrenPage,
    NavItem, NodeRelationships, OperationPlan, OrphanCandidates, OrphanStatus, PinnedItem,
    PlanPreviewStatus, PreviewMode, PreviewPolicy, ProcessResourceUsage, ProjectDetail,
    ProjectDiscoveryReport, ProjectReviewCheckpoint, ProjectSummary, QuickOpenResult, RecentItem,
    RecoverableSummary, RecoveryPending, RecoveryResolveResult, ReviewLedgerEntry, RiskReport,
    ScanRoot, ScanStatus, SecurityStatus, SessionChangeSet, SessionPreview, StartupStatus,
    SystemResourceProfile, WatcherNodeStatus, WatcherProjectStatus, WatcherStatus,
};
#[cfg(feature = "agent_automation")]
use hangar_core::{
    AiFollowUpResult, AiGlossaryEntry, AiGlossaryState, AiRewriteProposal, AiSuggestionApplyResult,
    AiWalkthroughPreview, AutomationActivityEntry, AutomationAgentSummary, AutomationCredential,
    AutomationReadGrant, AutomationStatus, CodeAnnotation,
};
#[cfg(feature = "mutation")]
use hangar_core::{
    MutationActivityBackup, MutationActivityItem, MutationActivityOperation, MutationMoveEntry,
    MutationStoredEntry,
};
#[cfg(feature = "mutation")]
use hangar_db::DbError;
use hangar_db::{
    Db, DocumentSearchOptions, LostProjectSearchOptions, NodeWatchFingerprint,
    OrphanAssetSearchOptions, RootScanFinish,
};
use hangar_discovery::{DiscoveryOptions, RegisteredRoot};
use hangar_jobs::JobStore;
#[cfg(feature = "mutation")]
use rusqlite::{params, OptionalExtension};
#[cfg(feature = "agent_automation")]
mod ai_assist;
#[cfg(feature = "mutation")]
mod app_removal;
#[cfg(feature = "mutation")]
mod controlled_checks;
mod dup_jobs;
#[cfg(feature = "mutation")]
mod edit_review;
#[cfg(feature = "mutation")]
mod edit_snapshot;
mod performance;
mod plan_jobs;
mod project_review;
mod project_summary;
mod session_changes;
#[cfg(feature = "mutation")]
mod value_edit;
#[cfg(feature = "agent_automation")]
pub use ai_assist::{ai_key_clear, ai_key_set, ai_key_status, AiExplainPreview};
#[cfg(feature = "mutation")]
pub use app_removal::{
    list_app_removals, record_app_removal, remove_antigravity_registration,
    remove_claude_registration, remove_codex_registration, remove_cursor_registration,
    remove_hermes_registration, remove_project_app_registrations, restore_app_removal,
    restore_app_removal_by_id, AppRemovalOutcome, AppRemovalRecord, PersistedAppRemoval,
};
use dup_jobs::DupJobStore;
#[cfg(feature = "agent_automation")]
pub use hangar_ai::AiUsageStatus;
use performance::{scan_limits, PerformanceMode, PerformanceScope};
use plan_jobs::PlanJobStore;
pub use project_summary::project_context_summary;
#[cfg(feature = "agent_automation")]
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "mutation")]
use std::sync::Arc;
use std::sync::{Arc as SharedArc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// Re-exported so the desktop app (which depends on hangar-api, not directly on
// hangar-appconfig) can name the connected-app host status in its Tauri commands.
#[cfg(feature = "agent_automation")]
pub use hangar_appconfig::HostStatus;

// Re-exported so the desktop app can name the AI provider config in its Tauri commands. The
// `pub use` also brings the name into this crate's scope for the provider wrappers below.
#[cfg(feature = "agent_automation")]
pub use hangar_core::AiProviderConfig;

#[cfg(feature = "agent_automation")]
const AUTOMATION_SCOPES: &[&str] = &[
    "read_structure",
    "read_body",
    "build_plan",
    "execute_plan",
    "history_search",
    // Curated-knowledge scopes used by the connected-AI-app surface. Reads list
    // comments; writes let an app manage only its OWN comments, and only when the
    // global AI-write toggle is also on. Never expose file bodies.
    "comments_read",
    "comments_write",
    // Dependency-graph + cleanup-intelligence reads (project graph map, node
    // relationships, orphan/duplicate candidates). Body-free and project-scoped like
    // read_structure, but granular so a user can grant plain context reads without
    // also exposing the heavier graph/cleanup surface.
    "read_graph",
];

/// Phase 3 (mutation feature): read-only signal that mutation/recovery commands
/// are compiled in. Present only with `--features mutation`; the strict `core`
/// lane has no mutation surface.
#[cfg(feature = "mutation")]
pub fn mutation_mode_status() -> Result<bool, String> {
    Ok(hangar_mutation::mutation_foundations_linked())
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_mode_status() -> Result<bool, String> {
    Ok(false)
}

#[cfg(feature = "mutation")]
pub fn recovery_pending(state: &AppState) -> Result<RecoveryPending, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let mut stmt = conn.prepare(
                "SELECT o.id, o.kind, o.status, o.target_node_id, o.target_fingerprint,
                        o.created_at, o.started_at, o.error,
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id),
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'done'),
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'pending'),
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'failed')
                 FROM operation o
                 WHERE o.status IN ('executing', 'backup_running', 'verifying')
                 ORDER BY o.id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(hangar_core::RecoveryOperation {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    status: row.get(2)?,
                    target_node_id: row.get(3)?,
                    target_fingerprint: row.get(4)?,
                    created_at: row.get(5)?,
                    started_at: row.get(6)?,
                    error: row.get(7)?,
                    total_items: row.get::<_, i64>(8)?.max(0) as u64,
                    done_items: row.get::<_, i64>(9)?.max(0) as u64,
                    pending_items: row.get::<_, i64>(10)?.max(0) as u64,
                    failed_items: row.get::<_, i64>(11)?.max(0) as u64,
                })
            })?;
            let operations = rows.collect::<Result<Vec<_>, _>>()?;
            Ok(RecoveryPending {
                enabled: true,
                pending: !operations.is_empty(),
                message: if operations.is_empty() {
                    "No interrupted operation journal entries need recovery.".to_string()
                } else {
                    "Interrupted operation journal entries need a user decision before further disk actions."
                        .to_string()
                },
                operations,
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn recovery_pending(_state: &AppState) -> Result<RecoveryPending, String> {
    Ok(RecoveryPending {
        enabled: false,
        pending: false,
        operations: Vec::new(),
        message: "Recovery checks are disabled because the mutation feature is not compiled."
            .to_string(),
    })
}

#[cfg(feature = "mutation")]
pub fn recovery_resolve(
    state: &AppState,
    decision: String,
) -> Result<RecoveryResolveResult, String> {
    let normalized = decision.trim().to_ascii_lowercase();
    if normalized != "rollback" {
        return Err(
            "Interrupted operations can only be rolled back safely. Resume-in-place is not available."
                .to_string(),
        );
    }

    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let report = hangar_mutation::recover_interrupted(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            Ok(RecoveryResolveResult {
                action: "rollback".to_string(),
                recovered_operations: report.recovered_operations as u64,
                rolled_back_items: report.rolled_back_items as u64,
                message: "Rollback completed from the journal.".to_string(),
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn recovery_resolve(
    _state: &AppState,
    _decision: String,
) -> Result<RecoveryResolveResult, String> {
    Err("Recovery resolution requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_token_issue(
    state: &AppState,
    action: String,
) -> Result<MutationTokenResult, String> {
    let parsed = parse_confirm_action(&action)?;
    if matches!(parsed, hangar_mutation::ConfirmAction::PermanentDelete) {
        ensure_final_remove_runtime_enabled(state)?;
    }
    Ok(MutationTokenResult {
        action,
        token: state.mutation_tokens.issue(parsed),
    })
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_token_issue(
    _state: &AppState,
    _action: String,
) -> Result<MutationTokenResult, String> {
    Err("Mutation confirmation tokens require a mutation-enabled build.".to_string())
}

/// Refuse any new forward mutation while a prior operation was left interrupted. The
/// journal-first design assumes recovery runs to completion before the next disk action;
/// without this guard a second mutation could stack on an unreconciled one.
/// `failed` is deliberately not included: executors use it only after reconciling their
/// physical outcome (for example, a partial quarantine keeps every moved copy as a visible
/// entry, and a post-move restore warning marks the entry restored). Ambiguous outcomes stay
/// `executing`/`verifying`, so they continue to block here and remain visible in Recovery.
#[cfg(feature = "mutation")]
fn ensure_no_pending_recovery(conn: &rusqlite::Connection) -> Result<(), DbError> {
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM operation WHERE status IN ('executing', 'backup_running', 'verifying')",
            [],
            |row| row.get(0),
        )
        .map_err(|err| DbError::FileRead(err.to_string()))?;
    if pending > 0 {
        return Err(DbError::FileRead(
            "A previous mutation was interrupted and must be recovered first. Open the Recovery area and resolve it before any new backup, move or delete."
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "mutation")]
pub fn mutation_backup_start(
    state: &AppState,
    plan: OperationPlan,
    destination_root: String,
    level: String,
    allow_same_volume: Option<bool>,
    include_protected: bool,
    token: String,
) -> Result<MutationBackupSummary, String> {
    consume_enter_token(state, &token)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            // When emptying the folder, sensitive/protected files are included so they are
            // backed up before the move removes them (the move's content-binding check
            // then requires this coverage). Reparse links are never backed up.
            let concrete = concrete_items_for_plan(conn, &plan, include_protected)?.items;
            let backup_items = concrete
                .iter()
                .map(|item| hangar_mutation::BackupItem {
                    source: item.source.clone(),
                    relative: item.relative.clone(),
                })
                .collect::<Vec<_>>();
            // The backup's in-source guard must protect the WHOLE folder that the move will
            // empty, not just the deepest common ancestor of the concrete items. For a
            // project/directory target the move recursively empties `plan.target.path`
            // (cleanup_root), so a backup written anywhere inside it would be moved/deleted with
            // everything else — losing the user's only backup. Guard against that exact root.
            let source_root = if matches!(plan.target.kind.as_str(), "project" | "directory") {
                std::path::PathBuf::from(&plan.target.path)
            } else {
                common_source_root(&concrete)
            };
            let result = hangar_mutation::create_backup(
                conn,
                hangar_mutation::BackupRequest {
                    level: parse_backup_level(&level),
                    source_root: &source_root,
                    destination_root: Path::new(&destination_root),
                    items: backup_items,
                    plan_json: serde_json::to_string(&plan)
                        .map_err(|err| DbError::FileRead(err.to_string()))?,
                    allow_same_volume: allow_same_volume.unwrap_or(false),
                },
            )
            .map_err(|err| DbError::FileRead(err.to_string()))?;
            Ok(MutationBackupSummary {
                backup_id: result.backup_id,
                manifest_path: result.manifest_path.to_string_lossy().to_string(),
                total_bytes: result.total_bytes,
                verified: result.verified,
                item_count: concrete.len() as u64,
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_backup_start(
    _state: &AppState,
    _plan: OperationPlan,
    _destination_root: String,
    _level: String,
    _allow_same_volume: Option<bool>,
    _include_protected: bool,
    _token: String,
) -> Result<MutationBackupSummary, String> {
    Err("Backup requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_move_start(
    state: &AppState,
    plan: OperationPlan,
    holding_root: String,
    verified_backup_id: i64,
    include_protected: bool,
    token: String,
) -> Result<MutationMoveSummary, String> {
    consume_enter_token(state, &token)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            // Gate 3: refuse to move anything into the holding area unless a verified
            // backup covers every concrete file in the plan. The held copies become
            // permanently deletable only via this backup linkage.
            let backup = hangar_mutation::load_verified_backup(conn, verified_backup_id)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let plan_items = concrete_items_for_plan(conn, &plan, include_protected)?;
            let concrete = plan_items.items;
            let reparse_links: Vec<std::path::PathBuf> = plan_items
                .reparse_links
                .into_iter()
                .map(|link| link.path)
                .collect();
            // Content binding (not just a path match): every plan file must be present in
            // the backup AND its current bytes must equal what the backup recorded, so a
            // stale/unrelated same-path backup cannot authorize moving (and later deleting)
            // different content.
            for item in &concrete {
                let source_text = item.source.to_string_lossy().to_string();
                let expected = backup.hash_for(&source_text).ok_or_else(|| {
                    DbError::FileRead(format!(
                        "The chosen backup does not cover {source_text}. Create a verified backup of every file before moving."
                    ))
                })?;
                let actual = hangar_mutation::file_blake3(&item.source)
                    .map_err(|err| DbError::FileRead(err.to_string()))?;
                if actual != expected {
                    return Err(DbError::FileRead(format!(
                        "The chosen backup does not match the current content of {source_text}. Re-create the backup before moving."
                    )));
                }
            }
            let items = concrete
                .iter()
                .map(|item| hangar_mutation::QuarantineItem {
                    source: item.source.clone(),
                    relative: item.relative.clone(),
                    backup_hash: backup
                        .hash_for(&item.source.to_string_lossy())
                        .map(str::to_string),
                })
                .collect::<Vec<_>>();
            let result = hangar_mutation::quarantine(
                conn,
                hangar_mutation::QuarantineRequest {
                    quarantine_root: Path::new(&holding_root),
                    items,
                    plan_json: serde_json::to_string(&plan)
                        .map_err(|err| DbError::FileRead(err.to_string()))?,
                    target_node_id: Some(plan.target.node_id),
                    target_fingerprint: Some(plan.target_fingerprint.clone()),
                    backup_id: verified_backup_id,
                    // For a project/folder target, remove the now-empty source
                    // directories after the move so the whole folder leaves the disk.
                    cleanup_root: matches!(plan.target.kind.as_str(), "project" | "directory")
                        .then(|| std::path::PathBuf::from(&plan.target.path)),
                    include_protected,
                    reparse_links,
                },
            )
            .map_err(|err| DbError::FileRead(err.to_string()))?;
            Ok(MutationMoveSummary {
                operation_id: result.operation_id,
                entries: result
                    .entries
                    .into_iter()
                    .map(|entry| MutationMoveEntry {
                        original_path: entry.original_path,
                        stored_path: entry.quarantine_path,
                        outcome: format!("{:?}", entry.outcome),
                        bytes: entry.bytes,
                        space_recovered: entry.space_recovered,
                        detail: entry.detail,
                    })
                    .collect(),
                space_recovered: result.space_recovered,
                moved: result.moved as u64,
                skipped: result.skipped as u64,
                failed: result.failed as u64,
                removed_dirs: result.removed_dirs as u64,
                removed_links: result.removed_links as u64,
            })
        })
        .map_err(to_message)
}

/// Read-only preview of an opt-in "empty the folder completely": the sensitive/protected
/// files (which would be copied to the backup then removed) and the reparse links (removed
/// without following). Drives the per-project confirmation; performs no mutation.
#[cfg(feature = "mutation")]
pub fn mutation_preview_protected(
    state: &AppState,
    plan: OperationPlan,
) -> Result<MutationProtectedPreview, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let plan_items = concrete_items_for_plan(conn, &plan, true)?;
            Ok(MutationProtectedPreview {
                protected: plan_items.protected_paths,
                reparse: plan_items
                    .reparse_links
                    .into_iter()
                    .map(|link| link.path.to_string_lossy().to_string())
                    .collect(),
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_preview_protected(
    _state: &AppState,
    _plan: OperationPlan,
) -> Result<MutationProtectedPreview, String> {
    Err("Preview requires a mutation-enabled build.".to_string())
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_move_start(
    _state: &AppState,
    _plan: OperationPlan,
    _holding_root: String,
    _verified_backup_id: i64,
    _include_protected: bool,
    _token: String,
) -> Result<MutationMoveSummary, String> {
    Err("Move requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_restore_start(
    state: &AppState,
    entry_id: i64,
    token: String,
) -> Result<MutationRestoreSummary, String> {
    consume_enter_token(state, &token)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            // Restore is itself a forward disk move (it relocates the held copy and writes
            // operation/operation_item rows), so it must obey the same invariant as
            // backup/move/final-remove: refuse while a prior operation was left interrupted,
            // or a restore could move the held copy out from under a pending recovery.
            ensure_no_pending_recovery(conn)?;
            let outcome = hangar_mutation::restore_entry(conn, entry_id)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let (outcome_label, original_path, restored_path, conflict_path) = match outcome {
                hangar_mutation::RestoreOutcome::Restored {
                    original_path,
                    restored_path,
                } => (
                    "restored".to_string(),
                    original_path,
                    Some(restored_path),
                    None,
                ),
                hangar_mutation::RestoreOutcome::Conflict {
                    original_path,
                    conflict_path,
                } => (
                    "conflict".to_string(),
                    original_path,
                    None,
                    Some(conflict_path),
                ),
            };
            Ok(MutationRestoreSummary {
                entry_id,
                outcome: outcome_label,
                original_path,
                restored_path,
                conflict_path,
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_restore_start(
    _state: &AppState,
    _entry_id: i64,
    _token: String,
) -> Result<MutationRestoreSummary, String> {
    Err("Restore requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_restore_to_folder_start(
    state: &AppState,
    entry_id: i64,
    destination_root: String,
    token: String,
) -> Result<MutationRestoreSummary, String> {
    consume_enter_token(state, &token)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            // Same invariant as restore_entry: a restore-to-folder is a forward disk move and
            // must not run while an interrupted operation is awaiting recovery.
            ensure_no_pending_recovery(conn)?;
            let outcome = hangar_mutation::restore_entry_to_folder(
                conn,
                entry_id,
                Path::new(&destination_root),
            )
            .map_err(|err| DbError::FileRead(err.to_string()))?;
            let (outcome_label, original_path, restored_path, conflict_path) = match outcome {
                hangar_mutation::RestoreOutcome::Restored {
                    original_path,
                    restored_path,
                } => (
                    "restored_elsewhere".to_string(),
                    original_path,
                    Some(restored_path),
                    None,
                ),
                hangar_mutation::RestoreOutcome::Conflict {
                    original_path,
                    conflict_path,
                } => (
                    "conflict".to_string(),
                    original_path,
                    None,
                    Some(conflict_path),
                ),
            };
            Ok(MutationRestoreSummary {
                entry_id,
                outcome: outcome_label,
                original_path,
                restored_path,
                conflict_path,
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_restore_to_folder_start(
    _state: &AppState,
    _entry_id: i64,
    _destination_root: String,
    _token: String,
) -> Result<MutationRestoreSummary, String> {
    Err("Restore requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_final_remove_start(
    state: &AppState,
    entry_id: i64,
    token: String,
) -> Result<MutationFinalRemoveSummary, String> {
    ensure_final_remove_runtime_enabled(state)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            let outcome = hangar_mutation::permanent_delete_entry(
                conn,
                &state.mutation_tokens,
                &token,
                entry_id,
            )
            .map_err(|err| DbError::FileRead(err.to_string()))?;
            Ok(MutationFinalRemoveSummary {
                entry_id,
                freed_bytes: outcome.freed_bytes,
            })
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_final_remove_start(
    _state: &AppState,
    _entry_id: i64,
    _token: String,
) -> Result<MutationFinalRemoveSummary, String> {
    Err("Final removal requires a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
pub fn mutation_lock_inspect_path(path: String) -> Result<MutationLockInspection, String> {
    let state = match hangar_mutation::inspect_lock(Path::new(&path)) {
        hangar_mutation::LockState::Free => "free",
        hangar_mutation::LockState::Locked => "locked",
        hangar_mutation::LockState::Missing => "missing",
    };
    Ok(MutationLockInspection {
        path,
        state: state.to_string(),
    })
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_lock_inspect_path(path: String) -> Result<MutationLockInspection, String> {
    Ok(MutationLockInspection {
        path,
        state: "unavailable".to_string(),
    })
}

#[cfg(feature = "mutation")]
pub fn mutation_activity_log(
    state: &AppState,
    limit: Option<usize>,
) -> Result<MutationActivityLog, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            load_activity_log(conn, limit.unwrap_or(50))
        })
        .map_err(to_message)
}

/// The (original_path, owning target_node_id) of a holding-area entry, so a
/// final-remove request can be concretely identified and scoped like the other
/// kinds. Joins the entry to the operation that created it. None if the entry id
/// does not exist. Gated on `agent_automation` (its only caller) so the
/// mutation-only build does not see it as dead code.
#[cfg(feature = "agent_automation")]
fn quarantine_entry_target(
    state: &AppState,
    entry_id: i64,
) -> Result<Option<(String, Option<i64>)>, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            conn.query_row(
                "SELECT qe.original_path, op.target_node_id
                 FROM quarantine_entry qe
                 LEFT JOIN operation op ON op.id = qe.operation_id
                 WHERE qe.id = ?1",
                params![entry_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .map_err(DbError::from)
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_activity_log(
    _state: &AppState,
    _limit: Option<usize>,
) -> Result<MutationActivityLog, String> {
    Ok(MutationActivityLog {
        enabled: false,
        operations: Vec::new(),
        items: Vec::new(),
        backups: Vec::new(),
        stored_entries: Vec::new(),
        message: "Mutation activity log requires a mutation-enabled build.".to_string(),
    })
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone)]
struct ConcreteMutationItem {
    source: PathBuf,
    relative: String,
}

/// A reparse point (junction/symlink) inside the target. It is never followed and has no
/// bytes of its own; when the user opts into emptying the folder it is removed as a LINK
/// (the executor records its target before removal), not backed up.
#[cfg(feature = "mutation")]
#[derive(Debug, Clone)]
struct ReparseLink {
    path: PathBuf,
}

/// The result of re-validating a plan: concrete files to back up + move, and (only when
/// the user opted into emptying the folder) reparse links to remove.
#[cfg(feature = "mutation")]
#[derive(Debug, Clone, Default)]
struct PlanItems {
    items: Vec<ConcreteMutationItem>,
    reparse_links: Vec<ReparseLink>,
    /// Paths of the sensitive/protected files included because the user opted in. Surfaced
    /// in the per-project confirmation so the user sees exactly which secrets will be
    /// copied to the backup and then removed.
    protected_paths: Vec<String>,
}

#[cfg(feature = "mutation")]
fn parse_confirm_action(action: &str) -> Result<hangar_mutation::ConfirmAction, String> {
    match action {
        "enter_mutation_mode" => Ok(hangar_mutation::ConfirmAction::EnterMutationMode),
        "final_remove" => Ok(hangar_mutation::ConfirmAction::PermanentDelete),
        _ => Err("Unknown mutation confirmation action.".to_string()),
    }
}

#[cfg(feature = "mutation")]
fn final_remove_env_enabled() -> bool {
    std::env::var("CODEHANGAR_ENABLE_FINAL_REMOVE")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Whether the irreversible "Final remove" action is enabled in this run: OFF until the user has
/// explicitly opted in through Recover (an encrypted setting), OR forced on by the supervised-QA
/// env var. A DB read error falls back to OFF (fail-closed). The Gate-3 safety checks (verified
/// backup, fresh confirmation token, held-copy content binding and restorable backup payload)
/// ALWAYS run regardless of this flag — it only controls whether final removal is OFFERED at all.
#[cfg(feature = "mutation")]
fn final_remove_runtime_enabled(state: &AppState) -> bool {
    if final_remove_env_enabled() {
        return true;
    }
    state
        .db()
        .ok()
        .and_then(|db| db.final_remove_enabled_value().ok())
        .unwrap_or(false)
}

#[cfg(feature = "mutation")]
fn ensure_final_remove_runtime_enabled(state: &AppState) -> Result<(), String> {
    if final_remove_runtime_enabled(state) {
        Ok(())
    } else {
        Err("Final removal is turned off. Enable it in Recover, then retry. Code Hangar still requires a verified backup and a fresh confirmation for every final removal.".to_string())
    }
}

/// Whether the irreversible "Final remove" action is enabled. OFF by default; the per-entry
/// Final-remove control is shown only when this is true.
#[cfg(feature = "mutation")]
pub fn mutation_final_remove_enabled(state: &AppState) -> bool {
    final_remove_runtime_enabled(state)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_final_remove_enabled(_state: &AppState) -> bool {
    false
}

/// Persist the user's final-remove preference (an encrypted setting, default OFF). Turning it on
/// offers final removal; every Gate-3 safety still runs per action when it is on.
#[cfg(feature = "mutation")]
pub fn set_final_remove_enabled(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_final_remove_enabled(enabled)
        .map_err(to_message)
}

#[cfg(feature = "mutation")]
fn consume_enter_token(state: &AppState, token: &str) -> Result<(), String> {
    if state
        .mutation_tokens
        .consume(token, hangar_mutation::ConfirmAction::EnterMutationMode)
    {
        Ok(())
    } else {
        Err("A fresh mutation confirmation token is required.".to_string())
    }
}

#[cfg(feature = "mutation")]
fn parse_backup_level(level: &str) -> hangar_mutation::BackupLevel {
    match level {
        "minimal" => hangar_mutation::BackupLevel::Minimal,
        "full" => hangar_mutation::BackupLevel::Full,
        _ => hangar_mutation::BackupLevel::Standard,
    }
}

#[cfg(feature = "mutation")]
fn concrete_items_for_plan(
    conn: &rusqlite::Connection,
    plan: &OperationPlan,
    include_protected: bool,
) -> Result<PlanItems, DbError> {
    let current = hangar_plan::build_operation_plan(conn, plan.target.node_id, &plan.action_label)
        .map_err(plan_error_to_db_error)?;
    if current.target_fingerprint != plan.target_fingerprint {
        return Err(DbError::FileRead(
            "Operation Plan is stale. Rebuild the preview before entering mutation mode."
                .to_string(),
        ));
    }

    let accounting = hangar_accounting::recoverable_for_target(conn, plan.target.node_id)
        .map_err(DbError::from)?;
    let mut issues = Vec::new();
    let mut items = Vec::new();
    let mut reparse_links = Vec::new();
    let mut protected_paths = Vec::new();
    for candidate in accounting.candidates {
        let is_explicit_protected_candidate =
            candidate.is_sensitive || candidate.protected_level.is_some() || candidate.is_reparse;
        let is_mutation_candidate = accounting.recoverable_node_ids.contains(&candidate.node_id)
            || (is_explicit_protected_candidate
                && accounting
                    .mutation_owned_node_ids
                    .contains(&candidate.node_id));
        if !is_mutation_candidate || (candidate.item_kind != "file" && !candidate.is_reparse) {
            continue;
        }
        let node = conn
            .query_row(
                "SELECT COALESCE(path, ''), COALESCE(size_apparent, 0), is_reparse, present,
                        volume_id, inode_key
                 FROM node
                 WHERE id = ?1 AND kind = ?2",
                params![candidate.node_id, candidate.item_kind],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?.max(0) as u64,
                        row.get::<_, i64>(2)? == 1,
                        row.get::<_, i64>(3)? == 1,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((path, _bytes, is_reparse, present, stored_volume_id, stored_inode_key)) = node
        else {
            issues.push(issue(
                candidate.node_id,
                &candidate.path,
                "file is missing from node table",
            ));
            continue;
        };
        if !present {
            issues.push(issue(candidate.node_id, &path, "file is no longer present"));
            continue;
        }
        if path.is_empty() {
            issues.push(issue(
                candidate.node_id,
                &candidate.path,
                "file path is empty",
            ));
            continue;
        }
        // Reparse points (junction/symlink) are never followed. Checked before the
        // existence test, which would follow the link. When the user opted into emptying
        // the folder, the link itself is removed (it has no bytes — the recorded target
        // lets it be recreated); otherwise it blocks the operation.
        if is_reparse {
            if include_protected {
                reparse_links.push(ReparseLink {
                    path: PathBuf::from(&path),
                });
            } else {
                issues.push(issue(
                    candidate.node_id,
                    &path,
                    "file is a reparse/symlink/junction entry",
                ));
            }
            continue;
        }
        // Sensitive/protected files are backed up + moved like any other file ONLY when
        // the user explicitly opted into emptying the folder (their bytes — secrets
        // included — are copied to the backup first); otherwise they block the operation.
        let is_sensitive = hangar_protect::is_sensitive_path(&path)
            || hangar_protect::protected_level_for_path(&path).is_some()
            || hangar_protect::is_strong_protected_path(&path);
        if is_sensitive && !include_protected {
            issues.push(issue(
                candidate.node_id,
                &path,
                "file is sensitive or protected",
            ));
            continue;
        }
        let source = PathBuf::from(&path);
        if !source.exists() {
            issues.push(issue(
                candidate.node_id,
                &path,
                "file does not exist on disk",
            ));
            continue;
        }
        let identity = hangar_fs::inspect_path_identity(&source);
        if identity.reparse_kind.as_deref() == Some("cloud_placeholder") {
            // An online-only cloud placeholder has ~0 local bytes; moving/deleting it would
            // hydrate it (network egress) or back up a stub and then destroy the local handle to
            // the cloud data. Never touch it — it is not local data to reclaim. (Belt-and-
            // suspenders: the accounting candidate query already excludes these from the set.)
            issues.push(issue(
                candidate.node_id,
                &path,
                "file is an online-only cloud placeholder",
            ));
            continue;
        }
        if identity.is_reparse {
            if include_protected {
                reparse_links.push(ReparseLink { path: source });
            } else {
                issues.push(issue(
                    candidate.node_id,
                    &path,
                    "runtime identity is reparse/symlink/junction",
                ));
            }
            continue;
        }
        if let (
            Some(stored_volume),
            Some(stored_inode),
            Some(runtime_volume),
            Some(runtime_inode),
        ) = (
            stored_volume_id.as_deref(),
            stored_inode_key.as_deref(),
            identity.volume_id.as_deref(),
            identity.inode_key.as_deref(),
        ) {
            if stored_volume != runtime_volume || stored_inode != runtime_inode {
                issues.push(issue(
                    candidate.node_id,
                    &path,
                    "file identity changed since the preview was built",
                ));
                continue;
            }
        }
        match hangar_mutation::inspect_lock(&source) {
            hangar_mutation::LockState::Locked => {
                issues.push(issue(
                    candidate.node_id,
                    &path,
                    "file is locked by another process",
                ));
                continue;
            }
            hangar_mutation::LockState::Missing => {
                issues.push(issue(
                    candidate.node_id,
                    &path,
                    "file disappeared before mutation",
                ));
                continue;
            }
            hangar_mutation::LockState::Free => {}
        }
        if is_sensitive {
            protected_paths.push(path.clone());
        }
        items.push(ConcreteMutationItem {
            source,
            relative: safe_relative(&candidate.path, &path),
        });
    }

    if !issues.is_empty() {
        return Err(DbError::FileRead(format_validation_issues(&issues)));
    }
    if items.is_empty() && reparse_links.is_empty() {
        return Err(DbError::FileRead(
            "Operation Plan has no concrete recoverable file items after revalidation.".to_string(),
        ));
    }
    Ok(PlanItems {
        items,
        reparse_links,
        protected_paths,
    })
}

#[cfg(feature = "mutation")]
fn plan_error_to_db_error(err: hangar_plan::PlanError) -> DbError {
    match err {
        hangar_plan::PlanError::Sqlite(err) => DbError::from(err),
        other => DbError::FileRead(other.to_string()),
    }
}

#[cfg(feature = "mutation")]
fn issue(node_id: i64, path: &str, reason: &str) -> hangar_core::MutationValidationIssue {
    hangar_core::MutationValidationIssue {
        node_id: Some(node_id),
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(feature = "mutation")]
fn format_validation_issues(issues: &[hangar_core::MutationValidationIssue]) -> String {
    let mut message = format!(
        "Operation Plan revalidation failed for {} item{}.",
        issues.len(),
        if issues.len() == 1 { "" } else { "s" }
    );
    for item in issues.iter().take(5) {
        message.push_str(&format!(" {}: {}", item.path, item.reason));
    }
    if issues.len() > 5 {
        message.push_str(" Additional issues omitted.");
    }
    message
}

#[cfg(feature = "mutation")]
fn safe_relative(candidate_path: &str, absolute_path: &str) -> String {
    let normalized = candidate_path.replace('\\', "/");
    if !normalized.is_empty() && !normalized.contains("..") {
        return normalized.trim_start_matches('/').to_string();
    }
    Path::new(absolute_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string())
}

#[cfg(feature = "mutation")]
fn common_source_root(items: &[ConcreteMutationItem]) -> PathBuf {
    if items.is_empty() {
        return PathBuf::from(".");
    }
    let first_parent = items[0]
        .source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    items.iter().skip(1).fold(first_parent, |root, item| {
        let mut probe = root;
        while !item.source.starts_with(&probe) {
            if !probe.pop() {
                return PathBuf::from(".");
            }
        }
        probe
    })
}

#[cfg(feature = "mutation")]
fn load_activity_log(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<MutationActivityLog, DbError> {
    let limit = limit.clamp(1, 200) as i64;
    let operations = {
        let mut stmt = conn.prepare(
            "SELECT id, kind, status, target_node_id, target_fingerprint, recovered_bytes,
                    created_at, started_at, finished_at, error
             FROM operation
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(MutationActivityOperation {
                id: row.get(0)?,
                kind: row.get(1)?,
                status: row.get(2)?,
                target_node_id: row.get(3)?,
                target_fingerprint: row.get(4)?,
                recovered_bytes: row
                    .get::<_, Option<i64>>(5)?
                    .map(|value| value.max(0) as u64),
                created_at: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let operation_ids = operations
        .iter()
        .map(|operation| operation.id)
        .collect::<Vec<_>>();
    let items = load_activity_items(conn, &operation_ids)?;
    let backups = load_activity_backups(conn, limit)?;
    let stored_entries = load_stored_entries(conn, limit)?;
    Ok(MutationActivityLog {
        enabled: true,
        message: if operations.is_empty() {
            "No mutation activity recorded.".to_string()
        } else {
            "Journal activity loaded from the local database.".to_string()
        },
        operations,
        items,
        backups,
        stored_entries,
    })
}

#[cfg(feature = "mutation")]
fn load_activity_items(
    conn: &rusqlite::Connection,
    operation_ids: &[i64],
) -> Result<Vec<MutationActivityItem>, DbError> {
    if operation_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", operation_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, operation_id, node_id, action, from_path, to_path, bytes, status
         FROM operation_item
         WHERE operation_id IN ({placeholders})
         ORDER BY operation_id DESC, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(operation_ids.iter()), |row| {
        Ok(MutationActivityItem {
            id: row.get(0)?,
            operation_id: row.get(1)?,
            node_id: row.get(2)?,
            action: row.get(3)?,
            from_path: row.get(4)?,
            to_path: row.get(5)?,
            bytes: row
                .get::<_, Option<i64>>(6)?
                .map(|value| value.max(0) as u64),
            status: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(feature = "mutation")]
fn load_activity_backups(
    conn: &rusqlite::Connection,
    limit: i64,
) -> Result<Vec<MutationActivityBackup>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, level, destination, manifest_path, total_bytes, verified, created_at
         FROM backup
         ORDER BY id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(MutationActivityBackup {
            id: row.get(0)?,
            level: row.get(1)?,
            destination: row.get(2)?,
            manifest_path: row.get(3)?,
            total_bytes: row
                .get::<_, Option<i64>>(4)?
                .map(|value| value.max(0) as u64),
            verified: row.get::<_, i64>(5)? == 1,
            created_at: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(feature = "mutation")]
fn load_stored_entries(
    conn: &rusqlite::Connection,
    limit: i64,
) -> Result<Vec<MutationStoredEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, operation_id, original_path, quarantine_path, size, file_count,
                risk_level, backup_id, space_recovered, scheduled_delete_at, status
         FROM quarantine_entry
         ORDER BY id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(MutationStoredEntry {
            id: row.get(0)?,
            operation_id: row.get(1)?,
            original_path: row.get(2)?,
            stored_path: row.get(3)?,
            size: row
                .get::<_, Option<i64>>(4)?
                .map(|value| value.max(0) as u64),
            file_count: row
                .get::<_, Option<i64>>(5)?
                .map(|value| value.max(0) as u64),
            risk_level: row.get(6)?,
            backup_id: row.get(7)?,
            space_recovered: row.get::<_, i64>(8)?.max(0) as u64,
            scheduled_delete_at: row.get(9)?,
            status: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Clone)]
pub struct AppState {
    db: SharedArc<DbSlot>,
    /// The encrypted inventory file this state opened (empty for in-memory state).
    /// Used to pin `CODEHANGAR_DB_PATH` when registering the connected-app server.
    db_path: PathBuf,
    project_cache_path: PathBuf,
    discovery_cache_path: PathBuf,
    jobs: JobStore,
    plan_jobs: PlanJobStore,
    dup_jobs: DupJobStore,
    #[cfg(feature = "mutation")]
    mutation_tokens: Arc<hangar_mutation::ConfirmTokenStore>,
    #[cfg(feature = "agent_automation")]
    automation_endpoint: SharedArc<Mutex<Option<String>>>,
    #[cfg(feature = "agent_automation")]
    ai_followups: SharedArc<Mutex<AiFollowUpStore>>,
    #[cfg(feature = "agent_automation")]
    ai_rewrite_proposals: SharedArc<Mutex<AiRewriteProposalStore>>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone)]
struct AiFollowUpExchange {
    question: String,
    answer: Option<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone)]
struct AiFollowUpConversation {
    node_id: i64,
    section_id: String,
    exchanges: Vec<AiFollowUpExchange>,
    touched_ms: u128,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Default)]
struct AiFollowUpStore {
    conversations: HashMap<String, AiFollowUpConversation>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone)]
struct PendingAiRewriteProposal {
    proposal: AiRewriteProposal,
    source_hash: String,
    created_ms: u128,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Default)]
struct AiRewriteProposalStore {
    proposals: HashMap<String, PendingAiRewriteProposal>,
}

#[cfg(feature = "agent_automation")]
type AiFollowUpHistory = Vec<(String, String)>;

#[cfg(feature = "agent_automation")]
#[derive(Debug)]
struct ReservedAiFollowUp {
    conversation_id: String,
    history: AiFollowUpHistory,
    turn: usize,
}

struct DbSlot {
    db: Mutex<Option<Db>>,
    startup: Mutex<StartupTracker>,
    started_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartupStateKind {
    Starting,
    Ready,
    Failed,
}

#[derive(Clone)]
struct StartupTracker {
    state: StartupStateKind,
    message: String,
    db_open_ms: Option<u64>,
}

impl StartupStateKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl DbSlot {
    fn starting(message: impl Into<String>) -> Self {
        Self {
            db: Mutex::new(None),
            startup: Mutex::new(StartupTracker {
                state: StartupStateKind::Starting,
                message: message.into(),
                db_open_ms: None,
            }),
            started_at: Instant::now(),
        }
    }

    fn ready(db: Db, db_open_ms: Option<u64>) -> Self {
        Self {
            db: Mutex::new(Some(db)),
            startup: Mutex::new(StartupTracker {
                state: StartupStateKind::Ready,
                message: "Local inventory is ready.".to_string(),
                db_open_ms,
            }),
            started_at: Instant::now(),
        }
    }

    fn set_ready(&self, db: Db, db_open_ms: u64) {
        if let Ok(mut slot) = self.db.lock() {
            *slot = Some(db);
        }
        if let Ok(mut startup) = self.startup.lock() {
            *startup = StartupTracker {
                state: StartupStateKind::Ready,
                message: "Local inventory is ready.".to_string(),
                db_open_ms: Some(db_open_ms),
            };
        }
    }

    fn set_failed(&self, message: String) {
        if let Ok(mut startup) = self.startup.lock() {
            *startup = StartupTracker {
                state: StartupStateKind::Failed,
                message,
                db_open_ms: None,
            };
        }
    }
}

pub struct LostProjectRequest {
    pub min_size_bytes: Option<u64>,
    pub project_id: Option<i64>,
    pub stale_preset: Option<String>,
    pub signals: Vec<String>,
    pub keyword: Option<String>,
    pub include_partial: bool,
    pub limit: usize,
    pub include_fixture_projects: bool,
    pub performance_mode: Option<String>,
}

pub struct DocumentSearchRequest {
    pub query: String,
    pub project_id: Option<i64>,
    pub indexed_kind: Option<String>,
    pub path_filter: Option<String>,
    pub name_filter: Option<String>,
    pub limit: Option<usize>,
    pub include_fixture_projects: bool,
    pub performance_mode: Option<String>,
}

pub struct OrphanAssetRequest {
    pub min_size_bytes: Option<u64>,
    pub project_id: Option<i64>,
    pub asset_kind: Option<String>,
    pub min_confidence: Option<String>,
    pub include_partial: Option<bool>,
    pub limit: Option<usize>,
    pub include_fixture_projects: bool,
    pub performance_mode: Option<String>,
}

pub struct DuplicateSearchRequest {
    pub min_size_bytes: Option<u64>,
    pub project_id: Option<i64>,
    pub file_kind: Option<String>,
    pub current_file_node_id: Option<i64>,
    pub limit: Option<usize>,
    pub include_fixture_projects: bool,
    pub performance_mode: Option<String>,
}

impl AppState {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, String> {
        let db_path_ref = db_path.as_ref();
        let state = Self {
            db: SharedArc::new(DbSlot::starting("Opening encrypted local inventory.")),
            db_path: db_path_ref.to_path_buf(),
            project_cache_path: db_path_ref.with_extension("projects.dpapi"),
            discovery_cache_path: db_path_ref.with_extension("discovery.dpapi"),
            jobs: JobStore::default(),
            plan_jobs: PlanJobStore::default(),
            dup_jobs: DupJobStore::default(),
            #[cfg(feature = "mutation")]
            mutation_tokens: Arc::new(hangar_mutation::ConfirmTokenStore::default()),
            #[cfg(feature = "agent_automation")]
            automation_endpoint: SharedArc::new(Mutex::new(None)),
            #[cfg(feature = "agent_automation")]
            ai_followups: SharedArc::new(Mutex::new(AiFollowUpStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_rewrite_proposals: SharedArc::new(Mutex::new(AiRewriteProposalStore::default())),
        };
        let db_path = db_path_ref.to_path_buf();
        let slot = SharedArc::clone(&state.db);
        thread::spawn(move || {
            // If a "Reset all" was requested, wipe the database file now — before
            // any connection opens — so the disk space is actually reclaimed. Doing
            // it here (rather than in-process during the reset) avoids the OS
            // file-handle locks that block deletion of an open SQLite database.
            hangar_db::wipe_pending_reset(&db_path);
            let opened_at = Instant::now();
            match Db::open(&db_path).map_err(to_message) {
                Ok(db) => {
                    let db_open_ms =
                        opened_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    slot.set_ready(db, db_open_ms);
                }
                Err(message) => slot.set_failed(message),
            }
        });
        Ok(state)
    }

    pub fn memory() -> Result<Self, String> {
        Ok(Self {
            db: SharedArc::new(DbSlot::ready(
                Db::open_memory().map_err(to_message)?,
                Some(0),
            )),
            db_path: PathBuf::new(),
            project_cache_path: PathBuf::new(),
            discovery_cache_path: PathBuf::new(),
            jobs: JobStore::default(),
            plan_jobs: PlanJobStore::default(),
            dup_jobs: DupJobStore::default(),
            #[cfg(feature = "mutation")]
            mutation_tokens: Arc::new(hangar_mutation::ConfirmTokenStore::default()),
            #[cfg(feature = "agent_automation")]
            automation_endpoint: SharedArc::new(Mutex::new(None)),
            #[cfg(feature = "agent_automation")]
            ai_followups: SharedArc::new(Mutex::new(AiFollowUpStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_rewrite_proposals: SharedArc::new(Mutex::new(AiRewriteProposalStore::default())),
        })
    }

    pub fn run_startup_maintenance(&self) -> Result<(), String> {
        self.db()?.run_startup_maintenance().map_err(to_message)
    }

    /// The encrypted inventory file this state opened (empty for in-memory state).
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn read_project_cache(&self) -> Vec<ProjectSummary> {
        if self.project_cache_path.as_os_str().is_empty() {
            return Vec::new();
        }
        let protected = match fs::read(&self.project_cache_path) {
            Ok(bytes) => bytes,
            Err(_) => return Vec::new(),
        };
        let json = match hangar_security::unprotect_local_bytes(&protected) {
            Ok(bytes) => bytes,
            Err(_) => return Vec::new(),
        };
        serde_json::from_slice::<Vec<ProjectSummary>>(&json).unwrap_or_default()
    }

    fn write_project_cache(&self, projects: &[ProjectSummary]) {
        if self.project_cache_path.as_os_str().is_empty() {
            return;
        }
        let snapshot: Vec<ProjectSummary> = projects.iter().take(200).cloned().collect();
        let json = match serde_json::to_vec(&snapshot) {
            Ok(json) => json,
            Err(_) => return,
        };
        let protected = match hangar_security::protect_local_bytes(&json) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        if let Some(parent) = self.project_cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.project_cache_path, protected);
    }

    /// Read the DPAPI-protected discovery snapshot (the JSON the frontend cached for
    /// startup responsiveness). Returns the raw JSON string, or None if absent/unreadable.
    fn read_discovery_cache(&self) -> Option<String> {
        if self.discovery_cache_path.as_os_str().is_empty() {
            return None;
        }
        let protected = fs::read(&self.discovery_cache_path).ok()?;
        let json = hangar_security::unprotect_local_bytes(&protected).ok()?;
        String::from_utf8(json).ok()
    }

    /// Persist the discovery snapshot DPAPI-wrapped (same local-user boundary as the
    /// inventory key) — never in plaintext UI storage (SECURITY_INVARIANTS.md:42). A
    /// pathologically large snapshot drops the cache instead of bloating it.
    fn write_discovery_cache(&self, snapshot: &str) {
        if self.discovery_cache_path.as_os_str().is_empty() {
            return;
        }
        // An empty snapshot (or a pathologically large one) clears the cache rather
        // than persisting it — used by "Reset all" to drop the inventory snapshot.
        if snapshot.is_empty() || snapshot.len() > 3_500_000 {
            let _ = fs::remove_file(&self.discovery_cache_path);
            return;
        }
        let protected = match hangar_security::protect_local_bytes(snapshot.as_bytes()) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        if let Some(parent) = self.discovery_cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.discovery_cache_path, protected);
    }

    fn db(&self) -> Result<Db, String> {
        let slot = self
            .db
            .db
            .lock()
            .map_err(|_| "Local inventory state is unavailable.".to_string())?;
        slot.clone()
            .ok_or_else(|| "Local inventory is still opening. Try again shortly.".to_string())
    }

    pub fn startup_status(&self) -> StartupStatus {
        let elapsed_ms = self
            .db
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        match self.db.startup.lock() {
            Ok(startup) => StartupStatus {
                state: startup.state.as_str().to_string(),
                message: startup.message.clone(),
                elapsed_ms,
                db_open_ms: startup.db_open_ms,
            },
            Err(_) => StartupStatus {
                state: "failed".to_string(),
                message: "Local inventory state is unavailable.".to_string(),
                elapsed_ms,
                db_open_ms: None,
            },
        }
    }
}

pub fn startup_status(state: &AppState) -> StartupStatus {
    state.startup_status()
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationProjectParams {
    project_id: i64,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationNodeParams {
    node_id: i64,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationPlanParams {
    target_node_id: i64,
    action_label: String,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationExecutionParams {
    plan: OperationPlan,
    action: String,
    destination_root: String,
    level: Option<String>,
    allow_same_volume: Option<bool>,
    confirm_token: String,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationHistoryParams {
    query: String,
    project_id: Option<i64>,
    limit: Option<usize>,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationCommentAddParams {
    node_id: i64,
    body: String,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationCommentEditParams {
    comment_id: i64,
    body: String,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationRequestCommentChangeParams {
    comment_id: i64,
    /// "edit" | "delete".
    action: String,
    body: Option<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationNavRefParams {
    nav_id: i64,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationNavChildrenParams {
    project_id: i64,
    parent_nav_id: Option<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationGraphParams {
    project_id: i64,
    limit: Option<usize>,
}

#[cfg(feature = "agent_automation")]
const MAX_AUTOMATION_GRAPH_NODES: usize = 1_000;

#[cfg(feature = "agent_automation")]
fn automation_graph_limit(limit: Option<usize>) -> Option<usize> {
    limit.map(|value| value.clamp(25, MAX_AUTOMATION_GRAPH_NODES))
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationOrphanParams {
    project_id: i64,
    min_size_bytes: Option<u64>,
    asset_kind: Option<String>,
    min_confidence: Option<String>,
    include_partial: Option<bool>,
    limit: Option<usize>,
}

#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationDuplicateParams {
    project_id: i64,
    min_size_bytes: Option<u64>,
    file_kind: Option<String>,
    limit: Option<usize>,
}

/// Params for a backup-protected / move-to-holding REQUEST. The agent supplies the
/// target node + a human-readable action label (+ optional level / include-protected
/// intent). It never supplies a destination — the human picks every folder at
/// approval (so an app can't choose where secret bytes land).
#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationNodeActionParams {
    node_id: i64,
    action_label: String,
    level: Option<String>,
    include_protected: Option<bool>,
}

/// Params for a permanent-delete REQUEST: only a prior holding-area entry id.
#[cfg(feature = "agent_automation")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationEntryParams {
    entry_id: i64,
}

/// The human-supplied gate state passed to `agent_request_resolve` at approval. The
/// agent never sees or sets these — they come from the in-app StrengthenedApproveDialog.
#[cfg(feature = "agent_automation")]
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolveInputs {
    /// Folder the human chose for the comment backup (comment kinds) or the verified
    /// backup (backup_protected / move_to_holding).
    pub backup_dir: Option<String>,
    /// Folder the human chose to move the target into (move_to_holding).
    pub holding_root: Option<String>,
    /// The human explicitly opted in to including protected/sensitive files.
    pub include_protected_opt_in: bool,
    /// The human authorized acting on a project the app is NOT scoped to.
    pub cross_scope_authorized: bool,
}

#[cfg(feature = "agent_automation")]
fn require_total_control(db: &Db) -> Result<(), String> {
    if db.mcp_full_control_enabled_value().map_err(to_message)? {
        Ok(())
    } else {
        Err("Total control is off. This app cannot request privileged actions.".to_string())
    }
}

/// Resolve a request target node's owning project, returning (project_id,
/// cross_scope). A node with no nav-item membership is accepted only if it is itself a
/// registered project root; anything else is refused. `cross_scope` is true when the
/// project is outside the agent's grants — allowed, but the approval gate then adds an
/// extra cross-project authorization step (never a silent action on un-granted data).
#[cfg(feature = "agent_automation")]
fn resolve_request_target_project(
    state: &AppState,
    db: &Db,
    agent: &AutomationAgentSummary,
    node_id: i64,
) -> Result<(i64, bool), String> {
    // Prefer a project the agent is actually granted, so a node shared between a
    // granted and an un-granted project is correctly treated as in-scope (and not
    // forced through cross-project authorization by an arbitrary row-order pick).
    let project_ids = db.node_project_ids(node_id).map_err(to_message)?;
    if let Some(granted) = project_ids
        .iter()
        .find(|pid| agent.project_ids.contains(pid))
    {
        return Ok((*granted, false));
    }
    let project_id = match project_ids.first() {
        Some(project_id) => *project_id,
        None if project_get(state, node_id)?.is_some() => node_id,
        None => return Err("Target is not part of a registered project.".to_string()),
    };
    let cross_scope = !agent.project_ids.contains(&project_id);
    Ok((project_id, cross_scope))
}

/// Redact a project graph to what an agent granted `granted_project_ids` may see.
/// The graph can pull nodes, edges and issues from OTHER projects via cross-project
/// duplicate/workflow edges (load_graph_node resolves any node id with no membership
/// check), so — mirroring NodeRelationships — drop every node/edge/issue outside the
/// grant, redact each surviving node's cross-project membership/details, and scrub
/// the machine-wide counts out of the shared-cache / duplicate-model issue and edge
/// text. A single-project app can then never enumerate the names, sizes, model
/// metadata, ids or cross-project counts of files in projects it was never granted.
#[cfg(feature = "agent_automation")]
fn redact_graph_to_grant(map: &mut hangar_core::GraphMap, granted_project_ids: &[i64]) {
    map.nodes
        .retain(|node| granted_project_ids.contains(&node.project_id));
    let granted_nodes: std::collections::HashSet<i64> =
        map.nodes.iter().map(|node| node.node_id).collect();
    for node in &mut map.nodes {
        node.shared_project_ids
            .retain(|pid| granted_project_ids.contains(pid));
        // "Inventoried by N registered projects"-style details disclose how many OTHER
        // projects exist on the machine; drop them from the agent surface.
        node.details
            .retain(|detail| !detail.contains("registered project"));
    }
    map.edges.retain(|edge| {
        granted_nodes.contains(&edge.source_node_id) && granted_nodes.contains(&edge.target_node_id)
    });
    map.issues.retain(|issue| {
        granted_nodes.contains(&issue.node_id)
            && issue
                .project_id
                .is_none_or(|pid| granted_project_ids.contains(&pid))
    });
    // The shared-cache and duplicate-model warnings embed a machine-wide count (how
    // many projects share a cache / how many duplicate copies exist across ALL
    // projects) in their evidence/target text — the same cross-project count
    // disclosure stripped from node.details above. Scrub those counts; the node, the
    // issue kind and the grant-visible edges remain, only the tally is removed.
    for issue in &mut map.issues {
        if issue.kind == "shared_cache_candidate" || issue.kind == "duplicate_model_candidate" {
            issue.evidence = None;
            if issue.kind == "duplicate_model_candidate" {
                issue.target = "model candidates".to_string();
            }
        }
    }
    for edge in &mut map.edges {
        if edge.kind == "duplicate_model_candidate" {
            edge.evidence = None;
        }
    }
    map.total_nodes = map.nodes.len() as i64;
    map.total_edges = map.edges.len() as i64;
    map.total_issues = map.issues.len() as i64;
}

#[cfg(feature = "agent_automation")]
fn queued_request_value(request_id: i64) -> Result<serde_json::Value, String> {
    serde_json::to_value(serde_json::json!({
        "status": "queued",
        "requestId": request_id,
        "message": "Queued for the user's approval in Code Hangar. Nothing has changed yet."
    }))
    .map_err(|error| error.to_string())
}

#[cfg(feature = "agent_automation")]
pub fn start_local_automation(state: &AppState) -> Result<String, String> {
    if let Ok(endpoint) = state.automation_endpoint.lock() {
        if let Some(endpoint) = endpoint.as_ref() {
            return Ok(endpoint.clone());
        }
    }
    let endpoint_id = hangar_agent::random_token(16)?;
    let handler_state = state.clone();
    let handler: hangar_agent::RequestHandler =
        SharedArc::new(move |request| handle_automation_request(&handler_state, request));
    let server = hangar_agent::LocalAgentServer::start(&endpoint_id, handler)?;
    let endpoint = server.endpoint().to_string();
    let mut slot = state
        .automation_endpoint
        .lock()
        .map_err(|_| "Local automation endpoint state is unavailable.".to_string())?;
    *slot = Some(endpoint.clone());
    Ok(endpoint)
}

#[cfg(feature = "agent_automation")]
pub fn automation_status(state: &AppState) -> Result<AutomationStatus, String> {
    let endpoint = state
        .automation_endpoint
        .lock()
        .map_err(|_| "Local automation endpoint state is unavailable.".to_string())?
        .clone();
    let registered_agents = state.db()?.automation_agents().map_err(to_message)?.len() as u64;
    Ok(AutomationStatus {
        enabled: endpoint.is_some(),
        endpoint,
        protocol: Some(hangar_agent::PROTOCOL_VERSION.to_string()),
        registered_agents,
        message: "Local automation is feature-gated, authenticated and restricted to this Windows machine."
            .to_string(),
    })
}

#[cfg(feature = "agent_automation")]
pub fn automation_register(
    state: &AppState,
    name: String,
    scopes: Vec<String>,
    project_ids: Vec<i64>,
) -> Result<AutomationCredential, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("Agent name must contain 1 to 80 characters.".to_string());
    }
    // "user" is the reserved identity of the local human. Refusing it (any case)
    // keeps an agent from ever authoring a comment that normalizes to a human
    // record or that slips past the AI-write gate by masquerading as "user".
    if name.eq_ignore_ascii_case("user") {
        return Err(
            "\"user\" is reserved for the local human; choose another agent name.".to_string(),
        );
    }
    let scopes = normalize_automation_scopes(scopes)?;
    let db = state.db()?;
    // A comment's ownership is keyed on the authoring agent's name (see
    // `guard_comment_actor`), so two ENABLED agents must never share a name (any
    // case) — otherwise one could edit the other's comments. (Human records stay
    // protected regardless: "user" is reserved and the boundary is independent.)
    if db
        .automation_agents()
        .map_err(to_message)?
        .iter()
        .any(|existing| existing.enabled && existing.name.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "An active agent named \"{name}\" already exists; choose a distinct name."
        ));
    }
    let known_projects = db.projects_list_lite().map_err(to_message)?;
    if project_ids.is_empty()
        || project_ids
            .iter()
            .any(|id| !known_projects.iter().any(|project| project.id == *id))
    {
        return Err("Select at least one existing project scope.".to_string());
    }
    let token = hangar_agent::random_token(32)?;
    let token_hash = automation_token_hash(&token);
    let agent = db
        .automation_register(name, &token_hash, &scopes, &project_ids)
        .map_err(to_message)?;
    db.automation_log(
        Some(agent.id),
        "register",
        "allowed",
        "Registered locally with explicit scopes.",
    )
    .map_err(to_message)?;
    let endpoint = state
        .automation_endpoint
        .lock()
        .map_err(|_| "Local automation endpoint state is unavailable.".to_string())?
        .clone()
        .ok_or_else(|| "Local automation server is not running.".to_string())?;
    Ok(AutomationCredential {
        agent,
        token,
        endpoint,
        protocol: hangar_agent::PROTOCOL_VERSION.to_string(),
    })
}

#[cfg(feature = "agent_automation")]
pub fn automation_agents(state: &AppState) -> Result<Vec<AutomationAgentSummary>, String> {
    state.db()?.automation_agents().map_err(to_message)
}

#[cfg(feature = "agent_automation")]
pub fn automation_revoke(state: &AppState, agent_id: i64) -> Result<bool, String> {
    let db = state.db()?;
    let revoked = db.automation_revoke(agent_id).map_err(to_message)?;
    if revoked {
        db.automation_log(
            Some(agent_id),
            "revoke",
            "allowed",
            "Revoked token and all active read grants.",
        )
        .map_err(to_message)?;
    }
    Ok(revoked)
}

#[cfg(feature = "agent_automation")]
pub fn automation_forget_revoked(state: &AppState, agent_id: i64) -> Result<bool, String> {
    state
        .db()?
        .automation_forget_revoked(agent_id)
        .map_err(to_message)
}

#[cfg(feature = "agent_automation")]
pub fn automation_grant_read(
    state: &AppState,
    agent_id: i64,
    node_id: i64,
    minutes: Option<u64>,
) -> Result<AutomationReadGrant, String> {
    let db = state.db()?;
    let agent = db
        .automation_agents()
        .map_err(to_message)?
        .into_iter()
        .find(|agent| agent.id == agent_id && agent.enabled)
        .ok_or_else(|| "The local agent is missing or revoked.".to_string())?;
    // Authorize by ANY project that inventories the node, not just the lowest project_id.
    ensure_automation_node(&agent, &db, node_id)?;
    let duration_ms = minutes.unwrap_or(10).clamp(1, 60) as i64 * 60_000;
    let expires_at_ms = Utc::now().timestamp_millis().saturating_add(duration_ms);
    let grant = db
        .automation_grant_read(agent_id, node_id, expires_at_ms)
        .map_err(to_message)?;
    db.automation_log(
        Some(agent_id),
        "read_grant",
        "allowed",
        &format!("Temporary read grant for node {node_id}."),
    )
    .map_err(to_message)?;
    Ok(grant)
}

#[cfg(feature = "agent_automation")]
pub fn automation_activity(
    state: &AppState,
    limit: Option<usize>,
) -> Result<Vec<AutomationActivityEntry>, String> {
    state
        .db()?
        .automation_activity(limit.unwrap_or(100))
        .map_err(to_message)
}

/// The connected AI apps' pending action requests awaiting the user's decision.
#[cfg(feature = "agent_automation")]
pub fn agent_requests_pending(
    state: &AppState,
) -> Result<Vec<hangar_core::AgentActionRequest>, String> {
    state.db()?.agent_requests_pending().map_err(to_message)
}

/// Resolve one pending request. On approval the app performs the action AS the
/// user (`actor = "user"`) — only here, after this explicit in-app decision —
/// optionally backing up the affected comment to a safe folder the user chose
/// first. The agent never executes anything; it only ever filed the request.
#[cfg(feature = "agent_automation")]
pub fn agent_request_resolve(
    state: &AppState,
    request_id: i64,
    approve: bool,
    inputs: ResolveInputs,
) -> Result<hangar_core::AgentActionRequest, String> {
    let db = state.db()?;
    let request = db
        .agent_request_get(request_id)
        .map_err(to_message)?
        .ok_or_else(|| "That request no longer exists.".to_string())?;
    if request.status != "pending" {
        return Err("That request was already resolved.".to_string());
    }

    if !approve {
        // Only reject if it is still pending — a concurrent approval may have already
        // claimed it (processing/approved); report that honestly instead of a silent
        // no-op that looks like a successful reject.
        if !db
            .agent_request_set_status(request_id, "rejected")
            .map_err(to_message)?
        {
            return Err("That request was already being processed or resolved.".to_string());
        }
        let _ = db.automation_log(
            request.agent_id,
            "request_rejected",
            "denied",
            "The user rejected the connected app's request.",
        );
        return db
            .agent_request_get(request_id)
            .map_err(to_message)?
            .ok_or_else(|| "That request no longer exists.".to_string());
    }

    // Each kind requires a specific live scope. Unknown kinds are refused.
    let required_scope = match request.kind.as_str() {
        "comment_edit" | "comment_delete" => "comments_write",
        "read_body" => "read_structure",
        "backup_protected" | "move_to_holding" | "final_remove" => "execute_plan",
        other => return Err(format!("Unsupported request kind: {other}.")),
    };

    // Re-authorize at approval (Wave-H must-fix): a request queued earlier must NOT
    // execute if the requesting app was revoked/disabled, or lost the scope (or, for
    // an in-scope request, the target project) since filing. The human is approving,
    // but a revoked agent's queued authority must not survive.
    let live_agent = match request.agent_id {
        Some(id) => db.automation_agent_by_id(id).map_err(to_message)?,
        None => None,
    };
    let stale = |db: &Db, reason: &str| -> Result<(), String> {
        db.agent_request_set_status(request_id, "rejected")
            .map_err(to_message)?;
        let _ = db.automation_log(request.agent_id, "request_stale", "denied", reason);
        Ok(())
    };
    let Some(live_agent) = live_agent else {
        stale(
            &db,
            "The requesting app was revoked or disabled before approval.",
        )?;
        return Err(
            "The app that requested this has since been revoked; nothing was changed.".to_string(),
        );
    };
    if !live_agent.scopes.iter().any(|s| s == required_scope) {
        stale(
            &db,
            "The requesting app lost the permission for this action.",
        )?;
        return Err(
            "The requesting app no longer has permission for this action; nothing was changed."
                .to_string(),
        );
    }
    // A non-cross-scope request must still be inside the agent's grants. A cross-scope
    // request was out of scope on purpose; it is gated by the explicit human
    // authorization below instead.
    if !request.cross_scope {
        if let Some(project_id) = request.project_id {
            if !live_agent.project_ids.contains(&project_id) {
                stale(&db, "The requesting app lost access to this project.")?;
                return Err(
                    "The requesting app is no longer scoped to this project; nothing was changed."
                        .to_string(),
                );
            }
        }
    }

    // Read-only panic switch: refuse to execute a queued write while frozen. Leave it
    // PENDING (not stale) so the user can turn read-only off and approve it later.
    if db.mcp_read_only_mode_value().map_err(to_message)? {
        return Err(
            "Code Hangar is in read-only mode; nothing was changed. Turn off read-only mode to apply this."
                .to_string(),
        );
    }

    // Cross-scope extra authorization: an action on a project the app was not granted
    // needs the user's explicit, separate authorization on top of the gate.
    if request.cross_scope && !inputs.cross_scope_authorized {
        return Err(
            "This app is not scoped to the target's project. Authorize the cross-project action to proceed.".to_string(),
        );
    }

    // Claim the request atomically (pending -> processing) so two concurrent
    // approvals can never both reach an executor; only the claimant proceeds.
    if !db
        .agent_request_transition(request_id, "pending", "processing")
        .map_err(to_message)?
    {
        return Err("That request is already being processed.".to_string());
    }

    let backup_dir = inputs
        .backup_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);

    // Perform the action AS the user, only now, after this in-app approval. Every
    // mutation flows through the unchanged Gate-3 executors, which independently
    // re-prove the verified-backup invariant and plan-fingerprint freshness. The
    // closure lets a failed executor release the claim back to pending, and records
    // each action's outcome so the durable agent_request row links forward to it.
    let mut result_outcome: Option<String> = None;
    let executed: Result<(), String> = (|| {
        match request.kind.as_str() {
            "comment_edit" | "comment_delete" => {
                let comment_id = request
                    .target_comment_id
                    .ok_or_else(|| "This request has no target comment.".to_string())?;
                // comment_edit is the only LOSSY comment op (delete is a soft-delete that keeps
                // the body). An agent-originated edit can replace a human's comment in place with
                // no in-DB history, so a backup is MANDATORY here — the prior text must stay
                // recoverable from the user's chosen folder, not optional.
                if request.kind == "comment_edit" && backup_dir.is_none() {
                    return Err("Editing a comment from an agent request requires choosing a backup folder so the original text stays recoverable.".to_string());
                }
                if let Some(dir) = backup_dir.as_deref() {
                    backup_comment_to_dir(&db, comment_id, dir)?;
                }
                if request.kind == "comment_edit" {
                    let body = request.proposed_body.clone().unwrap_or_default();
                    comment_edit(state, comment_id, body, "user")?;
                } else {
                    comment_delete(state, comment_id, "user")?;
                }
            }
            "read_body" => {
                let node_id = request
                    .target_id
                    .ok_or_else(|| "This request has no target node.".to_string())?;
                let agent_id = request
                    .agent_id
                    .ok_or_else(|| "This request has no requesting app.".to_string())?;
                // A short-lived per-node grant (the same expiry as the manual UI grant).
                let expires = Utc::now().timestamp_millis() + 10 * 60 * 1000;
                db.automation_grant_read(agent_id, node_id, expires)
                    .map_err(to_message)?;
                result_outcome = Some(serde_json::json!({ "grantedNode": node_id }).to_string());
            }
            "backup_protected" => {
                let dir = backup_dir
                    .clone()
                    .ok_or_else(|| "Choose a backup folder before approving.".to_string())?;
                if !inputs.include_protected_opt_in {
                    return Err(
                        "Tick the protected-files option to back up sensitive files.".to_string(),
                    );
                }
                let (plan, level) = resolve_plan_payload(&request)?;
                let level = level.unwrap_or_else(|| "standard".to_string());
                let token = mutation_token_issue(state, "enter_mutation_mode".to_string())?.token;
                let backup = mutation_backup_start(state, plan, dir, level, None, true, token)?;
                result_outcome =
                    Some(serde_json::json!({ "backupId": backup.backup_id }).to_string());
            }
            "move_to_holding" => {
                let dir = backup_dir
                    .clone()
                    .ok_or_else(|| "Choose a backup folder before approving.".to_string())?;
                let holding_root = inputs
                    .holding_root
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .ok_or_else(|| "Choose a holding folder before approving.".to_string())?
                    .to_string();
                let (plan, _level) = resolve_plan_payload(&request)?;
                let include_protected = request
                    .payload_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                    .and_then(|value| value.get("includeProtected").and_then(|v| v.as_bool()))
                    .unwrap_or(false);
                if include_protected && !inputs.include_protected_opt_in {
                    return Err(
                        "Tick the protected-files option to move sensitive files.".to_string()
                    );
                }
                // A verified backup covering every file is the precondition; create it
                // first to the user's folder, then move using its id. The move executor
                // re-checks content-binding regardless.
                let token = mutation_token_issue(state, "enter_mutation_mode".to_string())?.token;
                let backup = mutation_backup_start(
                    state,
                    plan.clone(),
                    dir,
                    "standard".to_string(),
                    None,
                    include_protected,
                    token,
                )?;
                let token = mutation_token_issue(state, "enter_mutation_mode".to_string())?.token;
                let moved = mutation_move_start(
                    state,
                    plan,
                    holding_root,
                    backup.backup_id,
                    include_protected,
                    token,
                )?;
                result_outcome = Some(
                    serde_json::json!({ "backupId": backup.backup_id, "moved": moved.moved })
                        .to_string(),
                );
            }
            "final_remove" => {
                let entry_id = request
                    .target_id
                    .ok_or_else(|| "This request has no target entry.".to_string())?;
                // The final-remove confirm token is itself gated behind the
                // CODEHANGAR_ENABLE_FINAL_REMOVE runtime opt-in, so without it this fails
                // here and nothing is deleted. purge.rs then re-proves the verified backup.
                // NB: the action string must be the canonical "final_remove" that
                // parse_confirm_action recognises (not "permanent_delete"), or the token is
                // unusable and the approval is a dead path.
                let token = mutation_token_issue(state, "final_remove".to_string())?.token;
                mutation_final_remove_start(state, entry_id, token)?;
                result_outcome = Some(serde_json::json!({ "removedEntry": entry_id }).to_string());
            }
            other => return Err(format!("Unsupported request kind: {other}.")),
        }
        Ok(())
    })();

    if let Err(error) = executed {
        // Release the claim so the user can review and retry; the Gate-3 executors
        // re-prove their invariants on any retry, so this never double-mutates.
        let _ = db.agent_request_transition(request_id, "processing", "pending");
        return Err(error);
    }

    // Durably link the agent_request row (agent id + kind + target + approved-at)
    // forward to what the app actually did, so the action is attributable.
    if let Some(outcome) = &result_outcome {
        let _ = db.agent_request_set_result(request_id, outcome);
    }

    db.agent_request_transition(request_id, "processing", "approved")
        .map_err(to_message)?;
    let _ = db.automation_log(
        request.agent_id,
        "request_approved",
        "allowed",
        &format!(
            "The user approved a '{}' request from this app; the app performed it as the user.",
            request.kind
        ),
    );
    db.agent_request_get(request_id)
        .map_err(to_message)?
        .ok_or_else(|| "That request no longer exists.".to_string())
}

/// Pull the app-built OperationPlan (and optional level) out of a queued mutation
/// request's payload. The plan was built by the app at filing time, so its
/// target_fingerprint is re-validated by the executor — the agent cannot forge it.
#[cfg(feature = "agent_automation")]
fn resolve_plan_payload(
    request: &hangar_core::AgentActionRequest,
) -> Result<(OperationPlan, Option<String>), String> {
    let raw = request
        .payload_json
        .as_deref()
        .ok_or_else(|| "This request is missing its plan.".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("Bad request payload: {error}"))?;
    let plan: OperationPlan = serde_json::from_value(
        value
            .get("plan")
            .cloned()
            .ok_or_else(|| "This request is missing its plan.".to_string())?,
    )
    .map_err(|error| format!("Bad request plan: {error}"))?;
    let level = value
        .get("level")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok((plan, level))
}

/// Write a single comment's full record to a JSON file in the user's chosen safe
/// folder, then verify it is readable, before a destructive change.
#[cfg(feature = "agent_automation")]
fn backup_comment_to_dir(db: &Db, comment_id: i64, dir: &str) -> Result<(), String> {
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        return Err("The chosen backup folder does not exist.".to_string());
    }
    let comment = db
        .comment_get(comment_id)
        .map_err(to_message)?
        .ok_or_else(|| "The comment to back up no longer exists.".to_string())?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let file = dir_path.join(format!("codehangar-comment-{comment_id}-{stamp}.json"));
    let json = serde_json::to_string_pretty(&comment).map_err(|error| error.to_string())?;
    fs::write(&file, json.as_bytes())
        .map_err(|error| format!("Could not write the backup: {error}"))?;
    let reread = fs::read_to_string(&file).map_err(|error| error.to_string())?;
    if reread.trim().is_empty() {
        return Err("The backup could not be verified after writing.".to_string());
    }
    Ok(())
}

/// Scopes granted when the user one-click-registers an AI app into a host config.
/// Covers the full read-only discovery surface — structure (`read_structure`:
/// catalog/context/nav/git/adapters) AND the body-free dependency graph + cleanup
/// intelligence (`read_graph`: graph map, relationships, orphans, duplicates) — plus
/// own-comment writes. Writes still need the global `comment_write_enabled` toggle.
/// `history_search` (which surfaces redacted session text) is deliberately NOT here:
/// it stays opt-in via the per-tool scope picker. No body, plan or mutation scope is
/// ever granted to a connector.
#[cfg(feature = "agent_automation")]
const CONNECTED_APP_SCOPES: &[&str] = &[
    "comments_read",
    "comments_write",
    "read_structure",
    "read_graph",
];

#[cfg(feature = "agent_automation")]
fn connected_app_home() -> Result<PathBuf, String> {
    hangar_appconfig::user_home()
        .ok_or_else(|| "Could not resolve your Windows home directory.".to_string())
}

#[cfg(feature = "agent_automation")]
fn resolve_connected_app_host(host_id: &str) -> Result<hangar_appconfig::Host, String> {
    hangar_appconfig::Host::from_id(host_id).ok_or_else(|| format!("Unknown AI app: {host_id}."))
}

/// The connected-app server executable, expected next to Code Hangar itself.
#[cfg(feature = "agent_automation")]
fn connected_app_server_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not locate Code Hangar: {e}."))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "Code Hangar has no parent directory.".to_string())?;
    let server = dir.join(hangar_appconfig::SERVER_EXE_NAME);
    if !server.exists() {
        return Err(format!(
            "The connected-app server ({}) was not found next to Code Hangar. Reinstall or build it before connecting an AI app.",
            hangar_appconfig::SERVER_EXE_NAME
        ));
    }
    Ok(server)
}

/// Status of every supported AI app's config (found / readable / registered).
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_status() -> Result<Vec<hangar_appconfig::HostStatus>, String> {
    let home = connected_app_home()?;
    Ok(hangar_appconfig::Host::ALL
        .into_iter()
        .map(|host| hangar_appconfig::status(host, &home))
        .collect())
}

/// Register Code Hangar's connector into one AI app's config: mint a fresh per-app
/// token (rotating any prior one for this app), create a scoped credential, and
/// write the config atomically with a verified backup. Project scope is always
/// explicit: an empty selection is refused rather than promoted to the full catalog.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_register(
    state: &AppState,
    host_id: String,
    project_ids: Vec<i64>,
) -> Result<hangar_appconfig::HostStatus, String> {
    if project_ids.is_empty() {
        return Err("Choose at least one project before connecting an AI app.".to_string());
    }
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    let db = state.db()?;
    let agent_name = host.label();

    // Validate and normalize the explicit project scope.
    let known = db.projects_list_lite().map_err(to_message)?;
    let mut project_ids = project_ids;
    project_ids.sort_unstable();
    project_ids.dedup();
    if project_ids
        .iter()
        .any(|id| !known.iter().any(|project| project.id == *id))
    {
        return Err("One or more selected projects no longer exist.".to_string());
    }
    let server_path = connected_app_server_path()?;

    // Rotate any existing credential only after the replacement scope is known-good,
    // so a stale UI selection cannot disconnect an otherwise valid app registration.
    for existing in db.automation_agents().map_err(to_message)? {
        if existing.enabled && existing.name.eq_ignore_ascii_case(agent_name) {
            db.automation_revoke(existing.id).map_err(to_message)?;
        }
    }

    let token = hangar_agent::random_token(32)?;
    let token_hash = automation_token_hash(&token);
    let scopes: Vec<String> = CONNECTED_APP_SCOPES.iter().map(|s| s.to_string()).collect();
    let agent = db
        .automation_register(agent_name, &token_hash, &scopes, &project_ids)
        .map_err(to_message)?;
    db.automation_log(
        Some(agent.id),
        "appconfig_register",
        "allowed",
        &format!("Connected the {agent_name} app."),
    )
    .map_err(to_message)?;

    let mut env = vec![("CODEHANGAR_MCP_TOKEN".to_string(), token)];
    let db_path = state.db_path.to_string_lossy().to_string();
    if !db_path.is_empty() {
        env.push(("CODEHANGAR_DB_PATH".to_string(), db_path));
    }
    let spec = hangar_appconfig::ServerSpec {
        command: server_path.to_string_lossy().to_string(),
        args: vec![],
        env,
        startup_timeout_sec: 20,
    };

    // If the host config write fails, roll the credential back so we never leave a
    // live token without a matching registration.
    if let Err(error) = hangar_appconfig::register(host, &home, &spec) {
        let _ = db.automation_revoke(agent.id);
        return Err(error);
    }
    Ok(hangar_appconfig::status(host, &home))
}

/// Remove Code Hangar's connector from one AI app's config and revoke its token.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_remove(
    state: &AppState,
    host_id: String,
) -> Result<hangar_appconfig::HostStatus, String> {
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    let db = state.db()?;
    let agent_name = host.label();
    // Revoke the DB credential(s) FIRST (fail-closed): a live config entry with a now-dead token
    // simply stops working. Doing the config write first would, on a revoke failure, leave the
    // credential enabled while the token is already stripped from disk — fail-open for a child
    // process that still holds the token in memory.
    for existing in db.automation_agents().map_err(to_message)? {
        if existing.enabled && existing.name.eq_ignore_ascii_case(agent_name) {
            db.automation_revoke(existing.id).map_err(to_message)?;
            db.automation_log(
                Some(existing.id),
                "appconfig_remove",
                "allowed",
                &format!("Disconnected the {agent_name} app."),
            )
            .map_err(to_message)?;
        }
    }
    hangar_appconfig::unregister(host, &home)?;
    Ok(hangar_appconfig::status(host, &home))
}

#[cfg(feature = "agent_automation")]
fn handle_automation_request(
    state: &AppState,
    request: hangar_agent::AgentRequest,
) -> hangar_agent::AgentResponse {
    let request_id = request.request_id.clone();
    if request.method == hangar_agent::AgentMethod::Status {
        let result = serde_json::json!({
            "enabled": true,
            "protocol": hangar_agent::PROTOCOL_VERSION,
            "authenticationRequired": true,
            "guestAccess": "capabilities_only"
        });
        return hangar_agent::AgentResponse::success(request_id, result);
    }

    let db = match state.db() {
        Ok(db) => db,
        Err(error) => return hangar_agent::AgentResponse::failure(request_id, error),
    };
    let token = match request.token.as_deref() {
        Some(token) if !token.is_empty() => token,
        _ => {
            let _ = db.automation_log(None, "authenticate", "denied", "Missing token.");
            return hangar_agent::AgentResponse::failure(request_id, "Authentication is required.");
        }
    };
    let agent = match db.automation_authenticate(&automation_token_hash(token)) {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            let _ = db.automation_log(None, "authenticate", "denied", "Invalid or revoked token.");
            return hangar_agent::AgentResponse::failure(
                request_id,
                "Invalid or revoked token. Reconnect this app in Code Hangar → Settings → AI app integration.",
            );
        }
        Err(error) => return hangar_agent::AgentResponse::failure(request_id, to_message(error)),
    };
    let method_label = automation_method_label(&request.method);
    let result = run_automation_method(state, &db, &agent, request.method, request.params);
    let (status, detail) = match &result {
        Ok(value) => (
            "allowed",
            automation_result_detail(method_label, value).to_string(),
        ),
        Err(error) => ("denied", truncate_for_log(error, 240)),
    };
    let _ = db.automation_log(Some(agent.id), method_label, status, &detail);
    match result {
        Ok(value) => hangar_agent::AgentResponse::success(request_id, value),
        Err(error) => hangar_agent::AgentResponse::failure(request_id, error),
    }
}

/// Public entry for an out-of-process host (the connected-AI-app child
/// process): authenticate and dispatch a single agent request, returning the wire
/// response. It funnels through the SAME token auth, scope/project gates and audit
/// logging as the in-process named-pipe server — there is no second code path and
/// no raw DB access for the host to reach around.
#[cfg(feature = "agent_automation")]
pub fn dispatch_agent_request(
    state: &AppState,
    request: hangar_agent::AgentRequest,
) -> hangar_agent::AgentResponse {
    handle_automation_request(state, request)
}

#[cfg(feature = "agent_automation")]
fn run_automation_method(
    state: &AppState,
    db: &Db,
    agent: &AutomationAgentSummary,
    method: hangar_agent::AgentMethod,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Read-only "panic switch": one chokepoint that refuses every write/mutation
    // method regardless of the other toggles. Reads pass through.
    if automation_method_is_write(&method) && db.mcp_read_only_mode_value().map_err(to_message)? {
        return Err(
            "Code Hangar is in read-only mode; the connector cannot write or change anything."
                .to_string(),
        );
    }
    match method {
        hangar_agent::AgentMethod::Status => {
            unreachable!("status is handled before authentication")
        }
        hangar_agent::AgentMethod::AgentProjectContext => {
            ensure_automation_scope(agent, "read_structure")?;
            let params: AutomationProjectParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let project = project_get(state, params.project_id)?
                .ok_or_else(|| "Project was not found.".to_string())?;
            let context = project_context_files(state, params.project_id)?;
            serde_json::to_value(serde_json::json!({
                "project": project,
                "contextFiles": context,
                "bodyContentIncluded": false
            }))
            .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::AgentReadBody => {
            let params: AutomationNodeParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (a shared file may belong to
            // several), not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            let has_scope = agent.scopes.iter().any(|scope| scope == "read_body");
            let has_grant = db
                .automation_has_read_grant(agent.id, params.node_id, Utc::now().timestamp_millis())
                .map_err(to_message)?;
            if !has_scope && !has_grant {
                return Err(
                    "File body access needs read_body scope or a current UI grant.".to_string(),
                );
            }
            let preview = file_preview(
                state,
                params.node_id,
                PreviewMode::Source,
                Some(false),
                Some(PreviewPolicy::default()),
            )?;
            serde_json::to_value(preview).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::AgentPlanBuild => {
            ensure_automation_scope(agent, "build_plan")?;
            let params: AutomationPlanParams = parse_automation_params(params)?;
            // Resolve the target's owning project. The old `unwrap_or(node_id)`
            // fallback treated ANY raw node id as a project id (they share an integer
            // space) — a scope-bypass surface. Now: a child node resolves via its
            // nav-item membership; a node with none is accepted ONLY if it is itself a
            // registered project root (a legitimate whole-project target); anything
            // else (ad-hoc/loose/unregistered) is refused.
            // Authorize by ANY project that inventories the node (not the lowest project_id),
            // keeping the whole-project-root fallback.
            resolve_agent_target_project(state, agent, db, params.target_node_id)?;
            let plan = operation_plan_build(
                state,
                params.target_node_id,
                params.action_label,
                Some("balanced".to_string()),
            )?;
            serde_json::to_value(plan).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::AgentPlanExecute => {
            ensure_automation_scope(agent, "execute_plan")?;
            let params: AutomationExecutionParams = parse_automation_params(params)?;
            // Same scope-resolution as AgentPlanBuild: resolve via nav-item membership,
            // accept a project root as a whole-project target, refuse any other node
            // with no project (no raw node_id-as-project_id fallback).
            // Authorize by ANY project that inventories the node (not the lowest project_id),
            // keeping the whole-project-root fallback.
            resolve_agent_target_project(state, agent, db, params.plan.target.node_id)?;
            let value = match params.action.as_str() {
                "backup" => serde_json::to_value(mutation_backup_start(
                    state,
                    params.plan,
                    params.destination_root,
                    params.level.unwrap_or_else(|| "standard".to_string()),
                    params.allow_same_volume,
                    // Automated emptying of protected/sensitive files is never allowed —
                    // that requires the explicit human confirmation flow.
                    false,
                    params.confirm_token,
                )?),
                "move_to_holding" => {
                    // Gate 3: a move requires a verified backup that covers every file,
                    // and emptying protected files requires explicit human confirmation.
                    // The automation surface does not carry that context, so it cannot
                    // perform a move; the human mutation flow must be used.
                    return Err(
                        "Automated move-to-holding is disabled: a verified backup and explicit confirmation are required (use the interactive mutation flow)."
                            .to_string(),
                    );
                }
                _ => {
                    return Err(
                        "Agent execution supports only verified backup or move_to_holding."
                            .to_string(),
                    )
                }
            };
            value.map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::DeepHistorySearch => {
            ensure_automation_scope(agent, "history_search")?;
            let params: AutomationHistoryParams = parse_automation_params(params)?;
            automation_history_search(state, agent, params)
        }
        hangar_agent::AgentMethod::CommentsList => {
            ensure_automation_scope(agent, "comments_read")?;
            let params: AutomationNodeParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (shared files belong to several),
            // not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            let comments = comments_for_node(state, params.node_id)?;
            serde_json::to_value(serde_json::json!({ "comments": comments }))
                .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::CommentsAdd => {
            ensure_automation_scope(agent, "comments_write")?;
            let params: AutomationCommentAddParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (shared files belong to several),
            // not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            // Belt-and-suspenders: registration already reserves "user", but never
            // let an agent author a human-looking record even if that ever changes.
            if agent.name.trim().eq_ignore_ascii_case("user") {
                return Err("This agent identity may not write comments.".to_string());
            }
            // The author/source are the AUTHENTICATED agent's name, assigned by the
            // server and never client-supplied, so an app cannot forge a human
            // ("user") record. The DB layer additionally refuses the write unless the
            // global AI-write toggle (`comment_write_enabled`) is on.
            let created = comment_add(
                state,
                params.node_id,
                params.body,
                Some(agent.name.clone()),
                Some(agent.name.clone()),
            )?;
            serde_json::to_value(created).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::CommentsEdit => {
            ensure_automation_scope(agent, "comments_write")?;
            let params: AutomationCommentEditParams = parse_automation_params(params)?;
            let project_id = db
                .comment_project_id(params.comment_id)
                .map_err(to_message)?
                .ok_or_else(|| "Comment was not found.".to_string())?;
            ensure_automation_project(agent, project_id)?;
            // `comment_edit` enforces the human/AI boundary in `guard_comment_actor`:
            // an agent (actor = its own name) may only edit a comment it authored
            // itself — never a human's or another agent's.
            let updated = comment_edit(state, params.comment_id, params.body, &agent.name)?;
            serde_json::to_value(updated).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::RequestCommentChange => {
            // Total-control tier: the agent may only REQUEST a change to a record it
            // could not otherwise touch (e.g. a human comment). It never executes;
            // the request is queued for the user to approve in-app.
            ensure_automation_scope(agent, "comments_write")?;
            if !db.mcp_full_control_enabled_value().map_err(to_message)? {
                return Err(
                    "Total control is off. This app can only request changes to its own comments."
                        .to_string(),
                );
            }
            let params: AutomationRequestCommentChangeParams = parse_automation_params(params)?;
            let kind = match params.action.as_str() {
                "edit" => "comment_edit",
                "delete" => "comment_delete",
                _ => return Err("Action must be \"edit\" or \"delete\".".to_string()),
            };
            if kind == "comment_edit" && params.body.as_deref().unwrap_or("").trim().is_empty() {
                return Err("An edit request needs a non-empty body.".to_string());
            }
            let project_id = db
                .comment_project_id(params.comment_id)
                .map_err(to_message)?
                .ok_or_else(|| "Comment was not found.".to_string())?;
            ensure_automation_project(agent, project_id)?;
            let request = db
                .agent_request_create(&hangar_db::NewAgentRequest {
                    agent_id: Some(agent.id),
                    agent_name: agent.name.clone(),
                    kind: kind.to_string(),
                    target_comment_id: Some(params.comment_id),
                    proposed_body: params.body.clone(),
                    target_kind: Some("comment".to_string()),
                    project_id: Some(project_id),
                    ..Default::default()
                })
                .map_err(to_message)?;
            serde_json::to_value(serde_json::json!({
                "status": "queued",
                "requestId": request.id,
                "message": "Queued for the user's approval in Code Hangar. Nothing has changed yet."
            }))
            .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListCatalog => {
            ensure_automation_scope(agent, "read_structure")?;
            // projects_list_lite returns EVERY project; intersect with the agent's
            // grants so an app never learns of a project it was not scoped to.
            let projects = projects_list_lite(state)?
                .into_iter()
                .filter(|project| agent.project_ids.contains(&project.id))
                .collect::<Vec<_>>();
            serde_json::to_value(serde_json::json!({ "projects": projects }))
                .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListContextFiles => {
            ensure_automation_scope(agent, "read_structure")?;
            let params: AutomationProjectParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let files = project_context_files(state, params.project_id)?;
            serde_json::to_value(serde_json::json!({ "contextFiles": files }))
                .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListProjectNav => {
            ensure_automation_scope(agent, "read_structure")?;
            let params: AutomationNavChildrenParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            // The DB query is constrained to project_id, so a foreign parentNavId can
            // only ever yield this (granted) project's rows.
            let page = project_nav_children(
                state,
                params.project_id,
                params.parent_nav_id,
                params.limit,
                params.offset,
            )?;
            serde_json::to_value(page).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ExplainFolder => {
            ensure_automation_scope(agent, "read_structure")?;
            let params: AutomationNavRefParams = parse_automation_params(params)?;
            // folder_explanation looks up by nav id and does NOT check project
            // membership, so resolve the explanation's project and gate on it BEFORE
            // returning anything — otherwise an app could enumerate folder
            // explanations across every project on the machine.
            let explanation = folder_explanation(state, params.nav_id)?
                .ok_or_else(|| "No folder explanation is available for that nav id.".to_string())?;
            ensure_automation_project(agent, explanation.project_id)?;
            serde_json::to_value(explanation).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::GetProjectGraph => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationGraphParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            // Connected apps get a stricter resource ceiling than the local UI.
            // Clamp here as well as advertising the bound in the MCP schema: a
            // hostile client can ignore JSON Schema and call the dispatch directly.
            let mut map = project_graph_map(
                state,
                params.project_id,
                automation_graph_limit(params.limit),
            )?;
            // The graph can pull in nodes, edges and issues from OTHER projects via
            // cross-project duplicate/workflow edges (load_graph_node resolves any node
            // id, with no membership check). Strip everything outside this app's grant
            // — mirroring NodeRelationships — so a single-project app cannot enumerate
            // the names, sizes, model metadata, ids or shared-project counts of files
            // in projects it was never granted.
            redact_graph_to_grant(&mut map, &agent.project_ids);
            serde_json::to_value(map).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::NodeRelationships => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationNodeParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (shared files belong to several),
            // not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            let mut relationships = node_relationships(state, params.node_id)?;
            // A relationship edge can point into another project; drop any related
            // node, and any issue, that belongs to a project this app was not granted.
            relationships
                .outgoing
                .retain(|edge| agent.project_ids.contains(&edge.project_id));
            relationships
                .incoming
                .retain(|edge| agent.project_ids.contains(&edge.project_id));
            relationships
                .issues
                .retain(|issue| agent.project_ids.contains(&issue.project_id));
            serde_json::to_value(relationships).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListOrphanAssets => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationOrphanParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let mut candidates = orphan_asset_candidates(
                state,
                OrphanAssetRequest {
                    min_size_bytes: params.min_size_bytes,
                    project_id: Some(params.project_id),
                    asset_kind: params.asset_kind,
                    min_confidence: params.min_confidence,
                    include_partial: params.include_partial,
                    limit: params.limit,
                    include_fixture_projects: true,
                    performance_mode: None,
                },
            )?;
            // The query already filters to project_id; this is belt-and-suspenders.
            candidates
                .candidates
                .retain(|candidate| agent.project_ids.contains(&candidate.project_id));
            candidates.total = candidates.candidates.len() as i64;
            serde_json::to_value(candidates).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::NodeOrphanStatus => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationNodeParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (shared files belong to several),
            // not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            let status = node_orphan_status(state, params.node_id)?;
            serde_json::to_value(status).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListDuplicateCandidates => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationDuplicateParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let mut result = duplicate_candidates(
                state,
                DuplicateSearchRequest {
                    min_size_bytes: params.min_size_bytes,
                    project_id: Some(params.project_id),
                    file_kind: params.file_kind,
                    current_file_node_id: None,
                    limit: params.limit,
                    include_fixture_projects: true,
                    performance_mode: None,
                },
            )?;
            // A duplicate group's members span MULTIPLE projects (each member carries
            // its own project_id); the project_id arg only seeds the surfacing. Drop
            // every member row from an un-granted project, then drop groups that no
            // longer have at least two visible members.
            result.groups.retain_mut(|group| {
                group
                    .members
                    .retain(|member| agent.project_ids.contains(&member.project_id));
                group.member_count = group.members.len() as u64;
                group.members.len() >= 2
            });
            result.total = result.groups.len() as i64;
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ConfirmDuplicateGroup => {
            ensure_automation_scope(agent, "read_graph")?;
            let params: AutomationNodeParams = parse_automation_params(params)?;
            // Authorize by ANY project that inventories the node (shared files belong to several),
            // not just the lowest project_id.
            ensure_automation_node(agent, db, params.node_id)?;
            let mut confirmation = confirm_duplicate_group(state, params.node_id)?;
            // Same cross-project member leak as the candidate list: filter members,
            // recompute the per-group count and reclaimable bytes, drop singletons.
            confirmation.confirmed_groups.retain_mut(|group| {
                group
                    .members
                    .retain(|member| agent.project_ids.contains(&member.project_id));
                group.member_count = group.members.len();
                if group.members.len() >= 2 {
                    group.reclaimable_bytes = group
                        .size_bytes
                        .saturating_mul(group.members.len() as u64 - 1);
                    true
                } else {
                    false
                }
            });
            confirmation.reclaimable_bytes = confirmation
                .confirmed_groups
                .iter()
                .map(|group| group.reclaimable_bytes)
                .sum();
            serde_json::to_value(confirmation).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ProjectGitStatus => {
            ensure_automation_scope(agent, "read_structure")?;
            let params: AutomationProjectParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let status = project_git_status(state, params.project_id)?;
            serde_json::to_value(status).map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListAdapters => {
            ensure_automation_scope(agent, "read_structure")?;
            // Static capability metadata about the AI-app adapters Code Hangar
            // understands — not user projects or paths — so it carries no project arg.
            let adapters = adapters_list(state)?;
            serde_json::to_value(serde_json::json!({ "adapters": adapters }))
                .map_err(|error| error.to_string())
        }
        hangar_agent::AgentMethod::ListMyRequests => {
            ensure_automation_scope(agent, "read_structure")?;
            // Own-app-scoped: the query is keyed to THIS authenticated agent's id, so
            // an app can only ever observe requests it filed itself — never another
            // app's rows or data. We project ONLY the loop-status fields (id, method,
            // status, timestamps); the payload, target ids, proposed bodies and
            // enriched comment text are deliberately withheld (they can carry other
            // records' content and are not needed to track a request's fate).
            let requests = db.agent_requests_for_agent(agent.id).map_err(to_message)?;
            let items: Vec<serde_json::Value> = requests
                .into_iter()
                .map(|request| {
                    serde_json::json!({
                        "id": request.id,
                        // The request "kind" IS the method the app asked for
                        // (read_body/backup_protected/…); expose it under `method`.
                        "method": request.kind,
                        "status": request.status,
                        "createdAt": request.created_at,
                        "resolvedAt": request.resolved_at,
                    })
                })
                .collect();
            Ok(serde_json::json!({ "requests": items }))
        }
        // ---- Total-control request kinds. Each only QUEUES a pending request; the
        // human approves and the app performs it AS the user via the Gate-3 executors.
        hangar_agent::AgentMethod::RequestReadBody => {
            ensure_automation_scope(agent, "read_structure")?;
            require_total_control(db)?;
            let params: AutomationNodeParams = parse_automation_params(params)?;
            let (project_id, cross_scope) =
                resolve_request_target_project(state, db, agent, params.node_id)?;
            let request = db
                .agent_request_create(&hangar_db::NewAgentRequest {
                    agent_id: Some(agent.id),
                    agent_name: agent.name.clone(),
                    kind: "read_body".to_string(),
                    target_kind: Some("node".to_string()),
                    target_id: Some(params.node_id),
                    project_id: Some(project_id),
                    cross_scope,
                    ..Default::default()
                })
                .map_err(to_message)?;
            queued_request_value(request.id)
        }
        hangar_agent::AgentMethod::RequestBackupProtected => {
            ensure_automation_scope(agent, "execute_plan")?;
            require_total_control(db)?;
            let params: AutomationNodeActionParams = parse_automation_params(params)?;
            let (project_id, cross_scope) =
                resolve_request_target_project(state, db, agent, params.node_id)?;
            // The app builds the plan (with its fingerprint) — never the agent — so a
            // forged target/fingerprint cannot be injected.
            let plan = operation_plan_build(
                state,
                params.node_id,
                params.action_label.clone(),
                Some("balanced".to_string()),
            )?;
            let payload = serde_json::json!({
                "plan": plan,
                "level": params.level.unwrap_or_else(|| "standard".to_string()),
            });
            let request = db
                .agent_request_create(&hangar_db::NewAgentRequest {
                    agent_id: Some(agent.id),
                    agent_name: agent.name.clone(),
                    kind: "backup_protected".to_string(),
                    detail: Some(params.action_label),
                    target_kind: Some("node".to_string()),
                    target_id: Some(params.node_id),
                    project_id: Some(project_id),
                    payload_json: Some(payload.to_string()),
                    cross_scope,
                    ..Default::default()
                })
                .map_err(to_message)?;
            queued_request_value(request.id)
        }
        hangar_agent::AgentMethod::RequestMoveToHolding => {
            ensure_automation_scope(agent, "execute_plan")?;
            require_total_control(db)?;
            let params: AutomationNodeActionParams = parse_automation_params(params)?;
            let (project_id, cross_scope) =
                resolve_request_target_project(state, db, agent, params.node_id)?;
            let plan = operation_plan_build(
                state,
                params.node_id,
                params.action_label.clone(),
                Some("balanced".to_string()),
            )?;
            let payload = serde_json::json!({
                "plan": plan,
                "includeProtected": params.include_protected.unwrap_or(false),
            });
            let request = db
                .agent_request_create(&hangar_db::NewAgentRequest {
                    agent_id: Some(agent.id),
                    agent_name: agent.name.clone(),
                    kind: "move_to_holding".to_string(),
                    detail: Some(params.action_label),
                    target_kind: Some("node".to_string()),
                    target_id: Some(params.node_id),
                    project_id: Some(project_id),
                    payload_json: Some(payload.to_string()),
                    cross_scope,
                    ..Default::default()
                })
                .map_err(to_message)?;
            queued_request_value(request.id)
        }
        hangar_agent::AgentMethod::RequestPermanentDelete => {
            ensure_automation_scope(agent, "execute_plan")?;
            require_total_control(db)?;
            let params: AutomationEntryParams = parse_automation_params(params)?;
            // Resolve the entry's path + owning project so the request names a concrete
            // target and is scoped like the other kinds: in-scope -> normal gate;
            // outside the agent's grants or project-less -> cross-scope (extra
            // authorization). A non-existent entry is refused outright.
            let (original_path, target_node_id) = quarantine_entry_target(state, params.entry_id)?
                .ok_or_else(|| "That holding-area entry was not found.".to_string())?;
            // Use ALL projects that inventory the node (not the lowest project_id): a node shared
            // across projects must not be mislabeled cross-scope just because its lowest project
            // is one the agent doesn't hold. Prefer a granted project; cross-scope only when the
            // agent holds NONE of the node's projects (or it has no project at all).
            let project_ids = match target_node_id {
                Some(node) => db.node_project_ids(node).map_err(to_message)?,
                None => Vec::new(),
            };
            let granted = project_ids
                .iter()
                .find(|pid| agent.project_ids.contains(pid))
                .copied();
            let project_id = granted.or_else(|| project_ids.first().copied());
            let cross_scope = granted.is_none();
            let request = db
                .agent_request_create(&hangar_db::NewAgentRequest {
                    agent_id: Some(agent.id),
                    agent_name: agent.name.clone(),
                    kind: "final_remove".to_string(),
                    detail: Some(original_path),
                    target_kind: Some("quarantine_entry".to_string()),
                    target_id: Some(params.entry_id),
                    project_id,
                    cross_scope,
                    ..Default::default()
                })
                .map_err(to_message)?;
            queued_request_value(request.id)
        }
    }
}

#[cfg(feature = "agent_automation")]
fn automation_history_search(
    state: &AppState,
    agent: &AutomationAgentSummary,
    params: AutomationHistoryParams,
) -> Result<serde_json::Value, String> {
    let query = params.query.trim();
    if query.chars().count() < 3 {
        return Err("History search needs at least 3 characters.".to_string());
    }
    let project_id = params
        .project_id
        .ok_or_else(|| "Agent history search requires an explicit projectId.".to_string())?;
    ensure_automation_project(agent, project_id)?;
    // History search runs a discovery pass, so it must honor the persisted WSL
    // opt-in even when it is the first call of a fresh process.
    sync_wsl_scan_flag(state);
    let roots = registered_roots_for_state(state)?;
    let report = hangar_discovery::discover_known_projects(
        &roots,
        DiscoveryOptions {
            limit: 0,
            include_loose_sessions: false,
            include_agents: true,
            include_technical_candidates: false,
        },
    );
    let needle = query.to_ascii_lowercase();
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let mut hits = Vec::new();
    for mut session in report.sessions.into_iter().take(1000) {
        if !session.linked_registered_project_ids.contains(&project_id) {
            continue;
        }
        let preview = match session_preview(session.path.clone(), false) {
            Ok(preview) => preview,
            Err(_) => continue,
        };
        let lower = preview.text.to_ascii_lowercase();
        let Some(index) = lower.find(&needle) else {
            continue;
        };
        let mut snippet = bounded_match_snippet(&preview.text, index, needle.len(), 120, 240);
        // A multi-project session names OTHER projects' absolute paths and ids. Strip
        // every reference to a project this app was not granted before returning the
        // hit — mirroring the cross-project filtering the read_graph tools apply.
        // The bounded snippet is raw transcript text, so also redact every linked-project path
        // from it before returning — otherwise an un-granted project's absolute path can leak in
        // the window. Best-effort exact-string redaction (a path written in a different
        // case/slash form may still slip through), which is why the metadata is scrubbed too.
        for path in &session.linked_project_paths {
            snippet = redact_path_occurrences(&snippet, path);
        }
        session
            .linked_registered_project_ids
            .retain(|id| agent.project_ids.contains(id));
        session.linked_project_paths.clear();
        hits.push(serde_json::json!({
            "session": session,
            "snippet": snippet,
            "redacted": true
        }));
        if hits.len() >= limit {
            break;
        }
    }
    Ok(serde_json::json!({
        "query": query,
        "hits": hits,
        "truncated": hits.len() >= limit,
        "persistentIndexUsed": false
    }))
}

#[cfg(feature = "agent_automation")]
fn normalize_automation_scopes(scopes: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = scopes
        .into_iter()
        .map(|scope| scope.trim().to_ascii_lowercase())
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    if normalized.is_empty()
        || normalized
            .iter()
            .any(|scope| !AUTOMATION_SCOPES.contains(&scope.as_str()))
    {
        return Err(format!(
            "Select one or more allowed scopes: {}.",
            AUTOMATION_SCOPES.join(", ")
        ));
    }
    Ok(normalized)
}

#[cfg(feature = "agent_automation")]
fn ensure_automation_scope(agent: &AutomationAgentSummary, scope: &str) -> Result<(), String> {
    if agent.scopes.iter().any(|candidate| candidate == scope) {
        Ok(())
    } else {
        Err(format!("Agent does not have the {scope} scope."))
    }
}

#[cfg(feature = "agent_automation")]
fn ensure_automation_project(
    agent: &AutomationAgentSummary,
    project_id: i64,
) -> Result<(), String> {
    if agent.project_ids.contains(&project_id) {
        Ok(())
    } else {
        Err("Agent is not scoped to this project.".to_string())
    }
}

/// Authorize an agent for a node by ANY project that inventories it, returning the granted
/// project id. A node can be inventoried by several projects (a shared file); gating on only the
/// lowest project_id (`node_project_id`) wrongly denied an agent access to a node it legitimately
/// owns via a different granted project.
#[cfg(feature = "agent_automation")]
fn ensure_automation_node(
    agent: &AutomationAgentSummary,
    db: &Db,
    node_id: i64,
) -> Result<i64, String> {
    let project_ids = db.node_project_ids(node_id).map_err(to_message)?;
    project_ids
        .iter()
        .find(|pid| agent.project_ids.contains(pid))
        .copied()
        .ok_or_else(|| "Agent is not scoped to this project.".to_string())
}

/// Authorize + resolve the owning project for an agent MUTATION target (plan build/execute).
/// Like `ensure_automation_node` (authorize by ANY granted project, not the lowest project_id),
/// but keeps the whole-project-root fallback the plan path needs: a node with no nav-item
/// membership is accepted only if it is itself a registered project root the agent is scoped to.
#[cfg(feature = "agent_automation")]
fn resolve_agent_target_project(
    state: &AppState,
    agent: &AutomationAgentSummary,
    db: &Db,
    node_id: i64,
) -> Result<i64, String> {
    let project_ids = db.node_project_ids(node_id).map_err(to_message)?;
    if !project_ids.is_empty() {
        return project_ids
            .iter()
            .find(|pid| agent.project_ids.contains(pid))
            .copied()
            .ok_or_else(|| "Agent is not scoped to this project.".to_string());
    }
    if project_get(state, node_id)?.is_some() {
        ensure_automation_project(agent, node_id)?;
        return Ok(node_id);
    }
    Err("Target is not part of a registered project.".to_string())
}

/// Redact every occurrence of `path` in `text`, case- and slash-insensitively, returning the
/// scrubbed text. Used so an un-granted project's absolute path can't leak in a history snippet
/// even when the transcript wrote it in a different slash or case form. The normalization is
/// byte-length- and char-boundary-preserving (ASCII lowercase + `\`→`/`), so matched spans in the
/// normalized haystack map directly back onto the original `text`.
#[cfg(feature = "agent_automation")]
fn redact_path_occurrences(text: &str, path: &str) -> String {
    if path.len() < 3 {
        return text.to_string();
    }
    let normalize = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    let haystack = normalize(text);
    let needle = normalize(path);
    if needle.is_empty() || !haystack.contains(&needle) {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut from = 0usize;
    while let Some(rel) = haystack[from..].find(&needle) {
        let start = from + rel;
        let end = start + needle.len();
        result.push_str(&text[last..start]);
        result.push_str("[redacted project path]");
        last = end;
        from = end;
    }
    result.push_str(&text[last..]);
    result
}

#[cfg(feature = "agent_automation")]
fn parse_automation_params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(params).map_err(|error| format!("Invalid request parameters: {error}"))
}

#[cfg(feature = "agent_automation")]
fn automation_token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// Whether a method writes or mutates state (so the read-only panic switch refuses
/// it). Reads — project context, file bodies, plan *previews*, history search,
/// comment listing, a caller's own request list and the whole discovery surface —
/// are not writes. Every `Request*` method belongs here: filing a pending-request
/// row is itself a write, so all of them are refused while read-only mode is on.
#[cfg(feature = "agent_automation")]
fn automation_method_is_write(method: &hangar_agent::AgentMethod) -> bool {
    matches!(
        method,
        hangar_agent::AgentMethod::CommentsAdd
            | hangar_agent::AgentMethod::CommentsEdit
            | hangar_agent::AgentMethod::RequestCommentChange
            | hangar_agent::AgentMethod::AgentPlanExecute
            // Filing a pending-request row is a write even when the request is only
            // to READ a body, so the read-only switch must refuse it too. (Omitting
            // this let an agent queue a file-access request while frozen, contra
            // SECURITY_INVARIANTS.md "any write/mutation request refused".)
            | hangar_agent::AgentMethod::RequestReadBody
            | hangar_agent::AgentMethod::RequestBackupProtected
            | hangar_agent::AgentMethod::RequestMoveToHolding
            | hangar_agent::AgentMethod::RequestPermanentDelete
    )
}

#[cfg(feature = "agent_automation")]
fn automation_method_label(method: &hangar_agent::AgentMethod) -> &'static str {
    match method {
        hangar_agent::AgentMethod::Status => "status",
        hangar_agent::AgentMethod::AgentProjectContext => "project_context",
        hangar_agent::AgentMethod::AgentReadBody => "read_body",
        hangar_agent::AgentMethod::AgentPlanBuild => "plan_build",
        hangar_agent::AgentMethod::AgentPlanExecute => "plan_execute",
        hangar_agent::AgentMethod::DeepHistorySearch => "history_search",
        hangar_agent::AgentMethod::CommentsList => "comments_list",
        hangar_agent::AgentMethod::CommentsAdd => "comments_add",
        hangar_agent::AgentMethod::CommentsEdit => "comments_edit",
        hangar_agent::AgentMethod::RequestCommentChange => "request_comment_change",
        hangar_agent::AgentMethod::ListCatalog => "list_catalog",
        hangar_agent::AgentMethod::ListContextFiles => "list_context_files",
        hangar_agent::AgentMethod::ListProjectNav => "list_project_nav",
        hangar_agent::AgentMethod::ExplainFolder => "explain_folder",
        hangar_agent::AgentMethod::GetProjectGraph => "get_project_graph",
        hangar_agent::AgentMethod::NodeRelationships => "node_relationships",
        hangar_agent::AgentMethod::ListOrphanAssets => "list_orphan_assets",
        hangar_agent::AgentMethod::NodeOrphanStatus => "node_orphan_status",
        hangar_agent::AgentMethod::ListDuplicateCandidates => "list_duplicate_candidates",
        hangar_agent::AgentMethod::ConfirmDuplicateGroup => "confirm_duplicate_group",
        hangar_agent::AgentMethod::ProjectGitStatus => "project_git_status",
        hangar_agent::AgentMethod::ListAdapters => "list_adapters",
        hangar_agent::AgentMethod::ListMyRequests => "list_my_requests",
        hangar_agent::AgentMethod::RequestReadBody => "request_read_body",
        hangar_agent::AgentMethod::RequestBackupProtected => "request_backup_protected",
        hangar_agent::AgentMethod::RequestMoveToHolding => "request_move_to_holding",
        hangar_agent::AgentMethod::RequestPermanentDelete => "request_permanent_delete",
    }
}

#[cfg(feature = "agent_automation")]
fn automation_result_detail(method: &str, _value: &serde_json::Value) -> String {
    format!("Local request {method} completed. Response body was not stored in the audit log.")
}

#[cfg(feature = "agent_automation")]
fn truncate_for_log(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(feature = "agent_automation")]
fn bounded_match_snippet(
    text: &str,
    match_start: usize,
    match_len: usize,
    before_chars: usize,
    after_chars: usize,
) -> String {
    let start = text[..match_start]
        .char_indices()
        .rev()
        .nth(before_chars)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let match_end = match_start.saturating_add(match_len).min(text.len());
    let end = text[match_end..]
        .char_indices()
        .nth(after_chars)
        .map(|(index, _)| match_end + index)
        .unwrap_or(text.len());
    text[start..end].to_string()
}

/// Push the persisted `wsl_scan_enabled` preference into the discovery runtime
/// gate. WSL enumeration stays OFF unless the user opted in, so a fresh install
/// never spawns `wsl.exe` at startup (which can surface a WSL error on a machine
/// where WSL is present but not fully set up). Public so entry points outside
/// this crate that reach WSL enumeration (e.g. the app-removal command, whose
/// Hermes step walks WSL state DBs) can sync before acting on a fresh process.
pub fn sync_wsl_scan_flag(state: &AppState) {
    let enabled = state
        .db()
        .ok()
        .and_then(|db| db.wsl_scan_enabled_value().ok())
        .unwrap_or(false);
    hangar_discovery::set_wsl_scan_enabled(enabled);
}

/// The user's persisted "I run AI tools in WSL" preference (default OFF).
pub fn wsl_scan_enabled(state: &AppState) -> bool {
    state
        .db()
        .ok()
        .and_then(|db| db.wsl_scan_enabled_value().ok())
        .unwrap_or(false)
}

/// Persist the WSL-scan preference and apply it to the discovery runtime gate.
pub fn set_wsl_scan_enabled(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_wsl_scan_enabled(enabled)
        .map_err(to_message)?;
    hangar_discovery::set_wsl_scan_enabled(enabled);
    Ok(())
}

/// Which local AI tools appear installed on this host (fast existence probe), plus
/// a WSL-presence OFFER when a distro is detected. The host probe never touches
/// WSL; the WSL entries come from a registry read that starts no VM (see
/// [`wsl_presence_apps`]). Drives the Deep Scan / first-run UI so it lists the tools
/// actually present and can offer to include ones installed inside WSL2.
pub fn detect_installed_apps() -> Vec<hangar_core::InstalledApp> {
    let mut apps = hangar_discovery::detect_installed_apps();
    apps.extend(wsl_presence_apps());
    apps
}

/// Distro names that only back a container runtime — never a place a user opens
/// projects, so they are excluded from the WSL presence offer. Mirrors
/// `hangar_discovery::is_system_wsl_distro` (kept local so the crates stay
/// decoupled; the list is tiny and stable).
fn is_system_wsl_distro_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "docker-desktop" | "docker-desktop-data" | "rancher-desktop" | "rancher-desktop-data"
    )
}

/// Normalize the raw `DistributionName` values read from the registry: trim, drop
/// empties and container-runtime distros, and dedup case-insensitively (keeping the
/// first spelling). Pure so the registry-read seam can be mocked in tests.
fn filter_wsl_distro_names(raw: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    raw.into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .filter(|name| !is_system_wsl_distro_name(name))
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect()
}

/// WSL-side presence entries appended to [`detect_installed_apps`], so the first-run
/// dialog can OFFER to include AI tools installed inside WSL2 without the app ever
/// cold-booting a distro.
///
/// Distro discovery is registry-only (no `wsl.exe`): reading
/// `HKCU\…\Lxss` starts no VM. The richer "which app might live there" existence
/// check stats `\\wsl.localhost\<distro>\home\*`, which CAN start a stopped distro,
/// so it runs ONLY when the user has already opted into WSL scanning. While the gate
/// is off the probe stops at registry-level presence — "WSL detected … enable WSL
/// scanning to include them" — preserving the zero-cold-boot guarantee.
///
/// Entries use reserved ids (`wsl`, `wsl:<app>`) that never collide with the host
/// app ids, so the UI can bucket them into the WSL offer.
#[cfg(windows)]
fn wsl_presence_apps() -> Vec<hangar_core::InstalledApp> {
    let distros = filter_wsl_distro_names(wsl_registry_distro_names_raw());
    if distros.is_empty() {
        return Vec::new();
    }
    let scanning = hangar_discovery::wsl_scan_enabled();
    let names = distros.join(", ");
    let summary = if scanning {
        format!(
            "WSL detected: {} distro(s) ({names}). WSL scanning is on — AI tools installed inside them are included.",
            distros.len()
        )
    } else {
        format!(
            "WSL detected: {} distro(s) ({names}). Enable WSL scanning to include AI tools installed inside them.",
            distros.len()
        )
    };
    let mut out = vec![hangar_core::InstalledApp {
        id: "wsl".to_string(),
        label: summary,
        present: true,
    }];

    // The per-app existence check stats `\\wsl.localhost\<distro>` — which can
    // COLD-BOOT a stopped distro — so it runs ONLY after the user opted into WSL
    // scanning. Off → we stop at the registry-level summary above. Cursor and
    // Antigravity are omitted on purpose (Windows-host GUI apps; they never install
    // inside a distro), matching the in-distro discovery-source set.
    if scanning {
        for (id, label, markers) in [
            ("wsl:claude", "Claude Code", &[".claude"][..]),
            ("wsl:codex", "ChatGPT", &[".codex"][..]),
            ("wsl:openclaw", "OpenClaw", &[".openclaw"][..]),
            (
                "wsl:hermes",
                "Hermes / NemoClaw",
                &[".hermes", ".nemoclaw"][..],
            ),
        ] {
            let hits = wsl_distros_with_marker(&distros, markers);
            if !hits.is_empty() {
                out.push(hangar_core::InstalledApp {
                    id: id.to_string(),
                    label: format!("{label} — in WSL ({})", hits.join(", ")),
                    present: true,
                });
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn wsl_presence_apps() -> Vec<hangar_core::InstalledApp> {
    Vec::new()
}

/// Distros whose home dirs contain any of `markers` (e.g. `.claude`). Enumerates
/// `\\wsl.localhost\<distro>\home\*`; the caller only invokes this AFTER the WSL
/// scan gate is on, because statting the share can cold-boot a stopped distro.
#[cfg(windows)]
fn wsl_distros_with_marker(distros: &[String], markers: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    for distro in distros {
        let homes = std::path::PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
        let Ok(entries) = std::fs::read_dir(&homes) else {
            continue;
        };
        let found = entries.flatten().any(|entry| {
            let home = entry.path();
            markers.iter().any(|marker| home.join(marker).exists())
        });
        if found {
            hits.push(distro.clone());
        }
    }
    hits
}

/// Read installed WSL distro `DistributionName` values from
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` WITHOUT invoking `wsl.exe`.
/// A pure registry read starts no distro VM (unlike `wsl.exe --list`, which touches
/// the distro and can cold-boot it). Returns the raw, unfiltered names; missing key
/// or any failure yields an empty list (WSL simply treated as absent).
#[cfg(windows)]
fn wsl_registry_distro_names_raw() -> Vec<String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let lxss = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
    let mut names = Vec::new();
    unsafe {
        let mut lxss_key: HKEY = ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, lxss.as_ptr(), 0, KEY_READ, &mut lxss_key)
            != ERROR_SUCCESS
        {
            // No Lxss key → WSL was never registered for this user. Not an error.
            return names;
        }
        let mut index = 0u32;
        loop {
            // Subkey names are distro GUIDs; 256 wchars is far more than enough.
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            let status = RegEnumKeyExW(
                lxss_key,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if status != ERROR_SUCCESS {
                break; // ERROR_NO_MORE_ITEMS (or any failure) ends enumeration.
            }
            index += 1;
            let mut sub_z: Vec<u16> = name_buf[..name_len as usize].to_vec();
            sub_z.push(0);
            let mut sub_key: HKEY = ptr::null_mut();
            if RegOpenKeyExW(lxss_key, sub_z.as_ptr(), 0, KEY_READ, &mut sub_key) != ERROR_SUCCESS {
                continue;
            }
            if let Some(name) = reg_read_sz(sub_key, "DistributionName") {
                names.push(name);
            }
            RegCloseKey(sub_key);
        }
        RegCloseKey(lxss_key);
    }
    names
}

/// Read one `REG_SZ` value from an open registry key as a `String`. `key` must be a
/// valid open handle (the caller owns and closes it). Returns `None` for a missing
/// value, a non-string type, or any read error.
#[cfg(windows)]
fn reg_read_sz(key: windows_sys::Win32::System::Registry::HKEY, value: &str) -> Option<String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, REG_SZ};

    let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // First query the byte size and type.
        let mut data_len: u32 = 0;
        let mut value_type: u32 = 0;
        if RegQueryValueExW(
            key,
            value_w.as_ptr(),
            ptr::null(),
            &mut value_type,
            ptr::null_mut(),
            &mut data_len,
        ) != ERROR_SUCCESS
            || value_type != REG_SZ
            || data_len == 0
        {
            return None;
        }
        // data_len is bytes; REG_SZ is UTF-16, so hold ceil(len / 2) code units.
        let mut buf = vec![0u16; (data_len as usize).div_ceil(2)];
        let mut got = data_len;
        if RegQueryValueExW(
            key,
            value_w.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            buf.as_mut_ptr() as *mut u8,
            &mut got,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        // Trim the trailing NUL terminator(s) the API includes.
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

pub fn projects_list(state: &AppState) -> Result<Vec<ProjectSummary>, String> {
    sync_wsl_scan_flag(state);
    let mut projects = state.db()?.projects_list().map_err(to_message)?;
    enrich_antigravity_names(&mut projects);
    enrich_current_state(&mut projects);
    state.write_project_cache(&projects);
    Ok(projects)
}

pub fn projects_list_lite(state: &AppState) -> Result<Vec<ProjectSummary>, String> {
    sync_wsl_scan_flag(state);
    let mut projects = state.db()?.projects_list_lite().map_err(to_message)?;
    enrich_antigravity_names(&mut projects);
    enrich_current_state(&mut projects);
    state.write_project_cache(&projects);
    Ok(projects)
}

/// Attach each project's owning `app` badge and its `is_current` flag from the
/// per-app registries/activity signals. A project is `is_current` when its root
/// appears in a recent app-activity signal — the Antigravity summaries proto (which
/// un-archives ExampleProj, whose `.pb` conversations never link a parsed `.db`) or a Claude
/// `lastSessionModified`. The discovery layer parses each source AT MOST ONCE here,
/// so this stays cheap on the hot `projects_list` path. Best-effort: a project whose
/// path no registry claims simply keeps `app = None` / `is_current = false`.
fn enrich_current_state(projects: &mut [ProjectSummary]) {
    let states = hangar_discovery::project_app_states();
    if states.is_empty() {
        return;
    }
    for project in projects.iter_mut() {
        if let Some(state) = states.get(&hangar_discovery::project_path_key(&project.path)) {
            if state.is_current {
                project.is_current = true;
            }
            // The registry is the live source of truth — `projects_list` always loads
            // `app`/`apps` empty, so adopt the registry's primary owner and UNION every app
            // the folder belongs to. A project used in Claude AND Codex must carry both, so
            // it is found under each app's filter even though only the most specific app owns
            // the badge.
            if project.app.is_none() {
                project.app = state.app.clone();
            }
            for app in &state.apps {
                if !project.apps.contains(app) {
                    project.apps.push(app.clone());
                }
            }
        }
    }
}

/// Attach each project's Antigravity (Gemini) display name when the folder basename
/// hides it — e.g. the "ExampleProj" project rooted at `D:\Example` lists as "Example" and gains a
/// "named: ExampleProj" label. Read from the Antigravity registry; best-effort and skipped
/// entirely when the registry is absent.
fn enrich_antigravity_names(projects: &mut [ProjectSummary]) {
    let names = hangar_discovery::antigravity_project_names();
    if names.is_empty() {
        return;
    }
    for project in projects.iter_mut() {
        if project.antigravity_name.is_some() {
            continue;
        }
        if let Some(name) = names.get(&hangar_discovery::project_path_key(&project.path)) {
            // Only show it when it actually adds information (differs from the name).
            if !name.eq_ignore_ascii_case(&project.name) {
                project.antigravity_name = Some(name.clone());
            }
        }
    }
}

pub fn projects_cached_snapshot(state: &AppState) -> Vec<ProjectSummary> {
    state.read_project_cache()
}

/// Persist the discovery snapshot (DPAPI-wrapped) for startup responsiveness. The
/// frontend passes the already-serialized JSON; this never writes plaintext.
pub fn cache_discovery_snapshot(state: &AppState, snapshot: String) {
    state.write_discovery_cache(&snapshot);
}

/// Read back the DPAPI-protected discovery snapshot, or None if absent.
pub fn read_discovery_snapshot(state: &AppState) -> Option<String> {
    state.read_discovery_cache()
}

pub fn watcher_status(
    state: &AppState,
    focused_project_id: Option<i64>,
    current_node_id: Option<i64>,
) -> Result<WatcherStatus, String> {
    let db = state.db()?;
    let roots = db.roots_list().map_err(to_message)?;
    let projects = db.projects_list_lite().map_err(to_message)?;
    let mut project_statuses = Vec::new();
    let mut stale_projects = 0_u64;
    let mut changed_projects = 0_u64;

    for root in roots {
        let project = projects
            .iter()
            .find(|project| same_local_path(&project.path, &root.path));
        let status = watcher_project_status(&root, project);
        if matches!(status.state.as_str(), "stale" | "missing" | "needs_scan") {
            stale_projects += 1;
        }
        if status.state == "stale" {
            changed_projects += 1;
        }
        project_statuses.push(status);
    }

    let focused = match focused_project_id {
        Some(project_id) => Some(focused_watcher_status(&db, project_id, current_node_id)?),
        None => None,
    };
    let message = if stale_projects == 0 {
        "Known project roots look current from the low-resolution watcher.".to_string()
    } else {
        format!("{stale_projects} known project root(s) need attention or a focused rescan.")
    };

    Ok(WatcherStatus {
        generated_at_ms: now_millis(),
        poll_interval_ms: 30_000,
        debounce_ms: 1_500,
        stale_projects,
        changed_projects,
        projects: project_statuses,
        focused,
        message,
    })
}

fn watcher_project_status(
    root: &ScanRoot,
    project: Option<&ProjectSummary>,
) -> WatcherProjectStatus {
    let path = Path::new(&root.path);
    let identity = hangar_fs::inspect_path_identity(path);
    let root_modified_at = identity
        .modified_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok());
    let scan_secs = root
        .last_scanned_at
        .as_deref()
        .and_then(parse_rfc3339_seconds);

    let (state, reason) = if !root.enabled {
        (
            "disabled".to_string(),
            "This scan root is disabled; the watcher does not mark it stale.".to_string(),
        )
    } else if identity.inaccessible || !path.exists() {
        (
            "missing".to_string(),
            identity
                .error
                .clone()
                .unwrap_or_else(|| "The root path is not currently reachable.".to_string()),
        )
    } else if directory_is_provably_empty(path, identity.is_reparse) {
        (
            "empty".to_string(),
            "This project folder is empty.".to_string(),
        )
    } else if root.last_scanned_at.is_none() {
        (
            "needs_scan".to_string(),
            "This root has not completed an inventory scan yet.".to_string(),
        )
    } else if root_modified_at
        .zip(scan_secs)
        .is_some_and(|(modified, scanned)| modified > scanned.saturating_add(1))
    {
        (
            "stale".to_string(),
            "The root folder changed after the last completed inventory scan.".to_string(),
        )
    } else {
        (
            "clean".to_string(),
            "No root-level change detected since the last completed scan.".to_string(),
        )
    };

    WatcherProjectStatus {
        project_id: project.map(|project| project.id),
        scan_root_id: root.id,
        name: project
            .map(|project| project.name.clone())
            .unwrap_or_else(|| display_path_for_path(&root.path)),
        path: root.path.clone(),
        state,
        reason,
        last_scanned_at: root.last_scanned_at.clone(),
        root_modified_at,
    }
}

fn directory_is_provably_empty(path: &Path, is_reparse: bool) -> bool {
    if is_reparse {
        return false;
    }
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

fn focused_watcher_status(
    db: &Db,
    project_id: i64,
    current_node_id: Option<i64>,
) -> Result<FocusedWatcherStatus, String> {
    let mut changed_context_files = 0_u64;
    for fingerprint in db
        .project_context_watch_fingerprints(project_id, 128)
        .map_err(to_message)?
    {
        let status = watcher_node_status(&fingerprint);
        if status.state == "changed" || status.state == "missing" {
            changed_context_files += 1;
        }
    }

    let current_node = match current_node_id {
        Some(node_id) => db
            .node_watch_fingerprint(node_id)
            .map_err(to_message)?
            .map(|fingerprint| watcher_node_status(&fingerprint)),
        None => None,
    };
    let has_current_change = current_node
        .as_ref()
        .is_some_and(|node| matches!(node.state.as_str(), "changed" | "missing"));
    let state = if has_current_change || changed_context_files > 0 {
        "dirty"
    } else {
        "clean"
    };
    let message = if has_current_change {
        "The open file changed on disk. Refresh the preview to see current content."
    } else if changed_context_files > 0 {
        "Context or Markdown files changed in the open project."
    } else {
        "No focused Markdown/context change detected."
    };

    Ok(FocusedWatcherStatus {
        project_id,
        state: state.to_string(),
        changed_context_files,
        current_node,
        message: message.to_string(),
    })
}

fn watcher_node_status(fingerprint: &NodeWatchFingerprint) -> WatcherNodeStatus {
    let path = Path::new(&fingerprint.path);
    let identity = hangar_fs::inspect_path_identity(path);
    let live_size = identity.size_apparent;
    let live_mtime = identity.modified_at.clone();
    let state = if identity.inaccessible || !path.exists() {
        "missing"
    } else if fingerprint.stored_mtime.is_none() && fingerprint.stored_size.is_none() {
        "untracked"
    } else if fingerprint.stored_mtime != live_mtime || fingerprint.stored_size != live_size {
        "changed"
    } else {
        "clean"
    };

    WatcherNodeStatus {
        node_id: fingerprint.node_id,
        path: display_path_for_path(&fingerprint.path),
        display_name: fingerprint.display_name.clone(),
        state: state.to_string(),
        is_markdown: fingerprint.is_markdown,
        is_context: fingerprint.is_context,
        stored_mtime: fingerprint.stored_mtime.clone(),
        live_mtime,
        stored_size: fingerprint.stored_size,
        live_size,
    }
}

fn parse_rfc3339_seconds(value: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc).timestamp())
        .and_then(|value| (value >= 0).then_some(value as u64))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn same_local_path(left: &str, right: &str) -> bool {
    normalize_path(left).eq_ignore_ascii_case(&normalize_path(right))
}

pub fn project_get(state: &AppState, project_id: i64) -> Result<Option<ProjectDetail>, String> {
    state.db()?.project_get(project_id).map_err(to_message)
}

pub fn project_nav_tree(state: &AppState, project_id: i64) -> Result<Vec<NavItem>, String> {
    state.db()?.project_nav_tree(project_id).map_err(to_message)
}

pub fn project_nav_children(
    state: &AppState,
    project_id: i64,
    parent_nav_id: Option<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<NavChildrenPage, String> {
    state
        .db()?
        .project_nav_children(
            project_id,
            parent_nav_id,
            limit.unwrap_or(200),
            offset.unwrap_or(0),
        )
        .map_err(to_message)
}

pub fn project_nav_path(
    state: &AppState,
    project_id: i64,
    node_id: i64,
) -> Result<Vec<NavItem>, String> {
    state
        .db()?
        .project_nav_path(project_id, node_id)
        .map_err(to_message)
}

pub fn project_git_status(state: &AppState, project_id: i64) -> Result<GitRepoSummary, String> {
    state
        .db()?
        .project_git_status(project_id)
        .map_err(to_message)
}

pub fn folder_explanation(
    state: &AppState,
    nav_id: i64,
) -> Result<Option<FolderExplanation>, String> {
    state.db()?.folder_explanation(nav_id).map_err(to_message)
}

/// Investigate an arbitrary folder by path WITHOUT registering it as a project: it is
/// indexed as an ad-hoc root (hidden from the projects list, discovery and settings) and a
/// scan is started. Poll the returned `job_id`, then call `investigation_report`.
pub fn investigate_folder(
    state: &AppState,
    path: String,
    performance_mode: Option<String>,
) -> Result<InvestigationHandle, String> {
    let db = state.db()?;
    let root = db.roots_add_adhoc(&path).map_err(to_message)?;
    let job_id = scan_start(state, Some(vec![root.id]), performance_mode)?;
    Ok(InvestigationHandle {
        root_id: root.id,
        job_id,
        path: display_path_for_path(&path),
    })
}

/// The investigation report for an ad-hoc root: what it is (explanation), who owns it
/// (reverse lookup) or whether it is orphan, and its footprint. `root_node_id` lets the
/// same Gate-3 backup/move/delete actions run on it like any project.
pub fn investigation_report(state: &AppState, root_id: i64) -> Result<FolderInvestigation, String> {
    let db = state.db()?;
    let mut report = db.investigation_report(root_id).map_err(to_message)?;
    if let Some(node_id) = report.root_node_id {
        report.explanation = db.folder_explanation(node_id).map_err(to_message)?;
    }
    Ok(report)
}

/// Drop an ad-hoc investigation (its root + indexed nodes) so it never lingers. Refuses if
/// the root is a registered project (a safety guard against unregistering a real project).
pub fn discard_investigation(state: &AppState, root_id: i64) -> Result<(), String> {
    let db = state.db()?;
    if !db.root_is_adhoc(root_id).map_err(to_message)? {
        return Err(
            "This folder is a registered project, not an ad-hoc investigation; nothing was removed."
                .to_string(),
        );
    }
    // Mirror roots_unregister / roots_set_enabled: never delete the root out from under a still-
    // running investigation scan, or the worker re-inserts a project node + nav_items for a root
    // that no longer has a scan_root (a resurrected, dangling-node state).
    if state.jobs.has_running_job_for_root(root_id) {
        return Err(
            "Cancel the active investigation scan before discarding this folder.".to_string(),
        );
    }
    db.roots_unregister(root_id).map_err(to_message)
}

pub fn node_full_path(state: &AppState, node_id: i64) -> Result<String, String> {
    state
        .db()?
        .node_path(node_id)
        .map_err(to_message)?
        .map(|path| display_path_for_path(&path))
        .ok_or_else(|| "Path is no longer available in the local inventory.".to_string())
}

// Available in the Local (mutation) edition too: in-app file editing reuses this exact
// inventory-resolution + protection gate (registered project, not sensitive/protected,
// not a reparse point) before it is allowed to write a file. agent_automation enables
// mutation, so the AI-Assist callers still compile.
#[cfg(feature = "mutation")]
fn resolve_ai_explain_inventory_target(
    state: &AppState,
    node_id: i64,
) -> Result<(String, Vec<String>), String> {
    let Some(target) = state.db()?.ai_explain_target(node_id).map_err(to_message)? else {
        return Err(
            "Not sent: this file is not a present item in a registered local project.".to_string(),
        );
    };
    if target.is_sensitive || target.protected_level.is_some() {
        return Err(
            "Not sent: this file is sensitive or belongs to a Protected Zone. Nothing left your machine."
                .to_string(),
        );
    }
    if target.is_reparse || target.reparse_kind.is_some() {
        return Err(
            "Not sent: symlinks, junctions, reparse points and cloud placeholders are not eligible for AI Assist."
                .to_string(),
        );
    }
    let project_paths = state
        .db()?
        .ai_explain_project_paths(node_id)
        .map_err(to_message)?;
    if project_paths.is_empty() {
        return Err(
            "Not sent: the file is no longer attached to a registered local project.".to_string(),
        );
    }
    Ok((target.path, project_paths))
}

#[cfg(feature = "mutation")]
fn validate_ai_explain_disk_target(path: &str, project_paths: &[String]) -> Result<(), String> {
    let target = Path::new(path);
    let identity = hangar_fs::inspect_path_identity(target);
    if identity.is_reparse || identity.reparse_kind.is_some() {
        return Err(
            "Not sent: the file is now a symlink, junction, reparse point or cloud placeholder."
                .to_string(),
        );
    }
    let canonical_target = target
        .canonicalize()
        .map_err(|_| "Not sent: the file is no longer available on disk.".to_string())?;
    if !canonical_target.is_file() {
        return Err("Not sent: the selected item is no longer a regular file.".to_string());
    }
    let inside_registered_project = project_paths.iter().any(|project_path| {
        Path::new(project_path)
            .canonicalize()
            .map(|canonical_root| canonical_target.starts_with(canonical_root))
            .unwrap_or(false)
    });
    if !inside_registered_project {
        return Err(
            "Not sent: the file now resolves outside its registered project boundary.".to_string(),
        );
    }
    Ok(())
}

/// Write new UTF-8 text content to an inventoried file. The Local edition's in-app editor uses
/// this path. Reuses the protected-file gate — the node
/// must be a present file in a registered project, not sensitive/protected, not a reparse point,
/// and resolve on disk inside its project boundary — then writes ATOMICALLY (temp file + rename)
/// so a crash mid-write can never truncate the original. Refuses a non-UTF-8 target (writing text
/// over a binary would corrupt it) and content above the editable size cap. Returns the bytes
/// written; the caller keeps the prior content for immediate Undo. Every save first creates and
/// verifies a durable minimal snapshot in the local journal. This is a single in-place save, not
/// a cleanup/delete operation, so it does not use a Gate-3 confirmation token.
///
/// Returns the EXACT prior bytes of the file (read here, server-side) so the caller's Undo restores
/// the true original — never a UI preview snapshot, which may be size-capped and would otherwise
/// truncate a large file on Undo.
#[cfg(feature = "mutation")]
pub fn write_file_content(state: &AppState, node_id: i64, content: &str) -> Result<String, String> {
    write_file_content_with_origin(state, node_id, content, "manual", None, None)
}

#[cfg(feature = "mutation")]
pub fn write_file_content_with_origin(
    state: &AppState,
    node_id: i64,
    content: &str,
    origin: &str,
    session_id: Option<&str>,
    expected_content: Option<&str>,
) -> Result<String, String> {
    let expected_hash =
        expected_content.map(|source| blake3::hash(source.as_bytes()).to_hex().to_string());
    edit_snapshot::write_file_with_snapshot(
        state,
        node_id,
        content,
        origin,
        session_id,
        expected_hash.as_deref(),
    )
    .map(|outcome| outcome.previous)
}

/// Desktop IPC entry point for a reviewed manual change. The review hash binds
/// the confirmation UI to the exact proposed bytes; restore/undo keeps its
/// separate verified-snapshot path.
#[cfg(feature = "mutation")]
pub fn write_reviewed_file_content(
    state: &AppState,
    node_id: i64,
    content: &str,
    origin: &str,
    expected_content: Option<&str>,
    reviewed_after_hash: Option<&str>,
) -> Result<String, String> {
    match origin {
        "manual" => {
            let reviewed = reviewed_after_hash
                .ok_or_else(|| "Not saved: review this change before applying it.".to_string())?;
            let actual = blake3::hash(content.as_bytes()).to_hex().to_string();
            if reviewed != actual {
                return Err(
                    "Not saved: the draft changed after review. Review the current draft again."
                        .to_string(),
                );
            }
        }
        "restore" if expected_content.is_some() => {}
        "restore" => {
            return Err("Undo refused: the current file version was not provided.".to_string())
        }
        _ => return Err("Not saved: that desktop edit origin is not supported.".to_string()),
    }
    write_file_content_with_origin(state, node_id, content, origin, None, expected_content)
}

#[cfg(feature = "mutation")]
pub fn file_edit_preview(
    state: &AppState,
    node_id: i64,
    content: &str,
    expected_content: Option<&str>,
) -> Result<hangar_core::FileEditPreview, String> {
    edit_review::preview_file_edit(state, node_id, content, expected_content)
}

#[cfg(feature = "mutation")]
pub fn edit_snapshots_for_node(
    state: &AppState,
    node_id: i64,
    limit: usize,
) -> Result<Vec<hangar_core::EditSnapshotSummary>, String> {
    edit_snapshot::list_snapshots(state, node_id, limit)
}

#[cfg(feature = "mutation")]
pub fn edit_snapshot_restore(
    state: &AppState,
    snapshot_id: i64,
) -> Result<hangar_core::EditSnapshotRestoreResult, String> {
    edit_snapshot::restore_snapshot(state, snapshot_id)
}

#[cfg(feature = "mutation")]
pub fn edit_snapshot_compare(
    state: &AppState,
    snapshot_id: i64,
) -> Result<hangar_core::EditSnapshotComparison, String> {
    edit_snapshot::compare_snapshot(state, snapshot_id)
}

#[cfg(feature = "agent_automation")]
pub fn ai_edit_sessions_for_node(
    state: &AppState,
    node_id: i64,
    limit: usize,
) -> Result<Vec<hangar_core::AiEditSessionSummary>, String> {
    edit_snapshot::list_ai_sessions(state, node_id, limit)
}

#[cfg(feature = "agent_automation")]
pub fn undo_ai_edit_session(
    state: &AppState,
    node_id: i64,
    session_id: &str,
) -> Result<hangar_core::EditSnapshotRestoreResult, String> {
    edit_snapshot::restore_ai_session(state, node_id, session_id)
}

#[cfg(feature = "mutation")]
pub fn editable_values(
    state: &AppState,
    node_id: i64,
) -> Result<hangar_core::EditableValueSet, String> {
    value_edit::editable_values(state, node_id)
}

#[cfg(feature = "mutation")]
pub fn apply_value_edit(
    state: &AppState,
    node_id: i64,
    request: &hangar_core::ValueEditRequest,
) -> Result<hangar_core::ValueEditResult, String> {
    value_edit::apply_value_edit(state, node_id, request)
}

#[cfg(feature = "mutation")]
pub fn apply_reviewed_value_edit(
    state: &AppState,
    node_id: i64,
    request: &hangar_core::ValueEditRequest,
    reviewed_after_hash: &str,
) -> Result<hangar_core::ValueEditResult, String> {
    let prepared = value_edit::prepare_value_edit(state, node_id, request)?;
    let actual = blake3::hash(prepared.content.as_bytes())
        .to_hex()
        .to_string();
    if actual != reviewed_after_hash {
        return Err(
            "Value not saved: the proposed file changed after review. Review it again.".to_string(),
        );
    }
    value_edit::apply_prepared_value_edit(state, node_id, request, prepared)
}

#[cfg(feature = "mutation")]
pub fn preview_value_edit(
    state: &AppState,
    node_id: i64,
    request: &hangar_core::ValueEditRequest,
) -> Result<hangar_core::FileEditPreview, String> {
    edit_review::preview_value_edit(state, node_id, request)
}

#[cfg(feature = "mutation")]
pub fn static_correction_check(
    state: &AppState,
    node_id: i64,
) -> Result<hangar_core::CorrectionStaticCheckReport, String> {
    controlled_checks::static_correction_check(state, node_id)
}

#[cfg(feature = "mutation")]
pub fn project_checks_detect(
    state: &AppState,
    project_id: i64,
) -> Result<Vec<hangar_core::ProjectCheckDefinition>, String> {
    controlled_checks::detect_project_checks(state, project_id)
}

#[cfg(feature = "mutation")]
pub fn project_check_approve(
    state: &AppState,
    project_id: i64,
    check_id: &str,
    fingerprint: &str,
) -> Result<hangar_core::ProjectCheckDefinition, String> {
    controlled_checks::approve_project_check(state, project_id, check_id, fingerprint)
}

#[cfg(feature = "mutation")]
pub fn project_check_revoke(
    state: &AppState,
    project_id: i64,
    check_id: &str,
) -> Result<bool, String> {
    controlled_checks::revoke_project_check_approval(state, project_id, check_id)
}

#[cfg(feature = "mutation")]
pub fn project_check_run(
    state: &AppState,
    project_id: i64,
    node_id: i64,
    check_id: &str,
    fingerprint: &str,
) -> Result<hangar_core::ControlledCheckRun, String> {
    controlled_checks::run_project_check(state, project_id, node_id, check_id, fingerprint)
}

#[cfg(feature = "agent_automation")]
fn resolve_ai_explain_target(state: &AppState, node_id: i64) -> Result<String, String> {
    let (path, project_paths) = resolve_ai_explain_inventory_target(state, node_id)?;
    validate_ai_explain_disk_target(&path, &project_paths)?;
    Ok(path)
}

/// Read-only cost and safety preview for AI Assist. The webview supplies only a node id;
/// the backend resolves and authorizes the current inventory record before reading bytes.
#[cfg(feature = "agent_automation")]
pub fn ai_explain_preview(state: &AppState, node_id: i64) -> Result<AiExplainPreview, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    ai_assist::ai_explain_preview_for_path(&path)
}

/// Explain an inventoried file with the configured provider. Resolve the target again
/// immediately before the send so a stale preview cannot authorize a changed or removed
/// inventory row. The provider (local loopback server or external API) comes from the encrypted
/// settings; `model` is an optional per-call override (empty ⇒ use the configured model).
#[cfg(feature = "agent_automation")]
pub fn ai_explain_file(
    state: &AppState,
    node_id: i64,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    ai_assist::ai_explain_file_for_path(&path, level, &config)
}

/// Explain a free-text code selection with the configured provider. The selection came from a
/// previewed file (`node_id`); we resolve that file's path again here and run the SAME gate as a
/// file explain (sensitive/protected path refused) before scanning the exact snippet bytes — so a
/// selection can never bypass a protection a whole-file explain would enforce. The provider comes
/// from the encrypted settings; `model` is an optional per-call override (empty ⇒ configured model).
#[cfg(feature = "agent_automation")]
pub fn ai_explain_text(
    state: &AppState,
    node_id: i64,
    snippet: &str,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    ai_assist::ai_explain_text_with_config(snippet, &path, level, &config)
}

/// Ask a configured model for a read-only review lens over an inventoried file.
/// Target resolution, project-boundary validation and secret gates are identical
/// to Explain; the returned text has no mutation path.
#[cfg(feature = "agent_automation")]
pub fn ai_review_file(
    state: &AppState,
    node_id: i64,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    ai_assist::ai_review_file_for_path(&path, level, &config)
}

/// Read-only review lens for selected code. The origin file and exact selected
/// bytes pass through the same gates as selection Explain.
#[cfg(feature = "agent_automation")]
pub fn ai_review_text(
    state: &AppState,
    node_id: i64,
    snippet: &str,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    ai_assist::ai_review_text_with_config(snippet, &path, level, &config)
}

/// Literal, credential-free disclosure for the exact Explain/What-to-check request. The target,
/// provider and prompt are reconstructed afresh; the webview supplies selectors only.
#[cfg(feature = "agent_automation")]
pub fn ai_send_disclosure(
    state: &AppState,
    node_id: i64,
    snippet: Option<&str>,
    lens: &str,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let lens = match lens {
        "explain" => ai_assist::AiReadLens::Explain,
        "review" => ai_assist::AiReadLens::Review,
        _ => return Err("Unknown code-reading lens.".to_string()),
    };
    ai_assist::ai_send_disclosure_for_path(&path, snippet, lens, level, &config)
}

/// Stream the primary Explain/What-to-check result. The same fresh target resolution, send-gate
/// and prompt builders power disclosure and the real send. The callback receives text deltas only.
#[cfg(feature = "agent_automation")]
pub fn ai_read_stream<F>(
    state: &AppState,
    node_id: i64,
    snippet: Option<&str>,
    lens: &str,
    level: &str,
    model: &str,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    match (lens, snippet) {
        ("explain", None) => {
            ai_assist::ai_explain_file_stream_for_path(&path, level, &config, on_delta)
        }
        ("explain", Some(snippet)) => {
            ai_assist::ai_explain_text_stream_with_config(snippet, &path, level, &config, on_delta)
        }
        ("review", None) => {
            ai_assist::ai_review_file_stream_for_path(&path, level, &config, on_delta)
        }
        ("review", Some(snippet)) => {
            ai_assist::ai_review_text_stream_with_config(snippet, &path, level, &config, on_delta)
        }
        _ => Err("Unknown code-reading lens.".to_string()),
    }
}

/// Deterministic local section map and exact pre-send cost for the optional
/// guided file walkthrough. No provider is contacted.
#[cfg(feature = "agent_automation")]
pub fn ai_walkthrough_preview(
    state: &AppState,
    node_id: i64,
) -> Result<AiWalkthroughPreview, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    ai_assist::ai_walkthrough_preview_for_path(&path)
}

/// Explain selected, backend-derived file sections. Section ids are treated as
/// stale selectors only: their source bytes are always reconstructed afresh.
#[cfg(feature = "agent_automation")]
pub fn ai_walkthrough_file(
    state: &AppState,
    node_id: i64,
    section_ids: Vec<String>,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    ai_assist::ai_walkthrough_file_for_path(&path, &section_ids, level, &config)
}

#[cfg(feature = "agent_automation")]
const AI_FOLLOW_UP_MAX_TURNS: usize = 3;
#[cfg(feature = "agent_automation")]
const AI_FOLLOW_UP_MAX_CONVERSATIONS: usize = 50;

#[cfg(feature = "agent_automation")]
fn follow_up_history(
    state: &AppState,
    node_id: i64,
    section_id: &str,
    conversation_id: Option<&str>,
) -> Result<AiFollowUpHistory, String> {
    let Some(conversation_id) = conversation_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let store = state
        .ai_followups
        .lock()
        .map_err(|_| "The follow-up memory is unavailable.".to_string())?;
    let conversation = store.conversations.get(conversation_id).ok_or_else(|| {
        "That follow-up expired. Start a new question from the section.".to_string()
    })?;
    if conversation.node_id != node_id || conversation.section_id != section_id {
        return Err("That follow-up belongs to a different file section.".to_string());
    }
    if conversation
        .exchanges
        .iter()
        .any(|exchange| exchange.answer.is_none())
    {
        return Err(
            "Wait for the current follow-up answer before asking another question.".to_string(),
        );
    }
    Ok(conversation
        .exchanges
        .iter()
        .filter_map(|exchange| {
            exchange
                .answer
                .as_ref()
                .map(|answer| (exchange.question.clone(), answer.clone()))
        })
        .collect())
}

/// Exact cost preview for one section-scoped follow-up, including the bounded
/// in-memory history that would be sent. No conversation turn is consumed.
#[cfg(feature = "agent_automation")]
pub fn ai_follow_up_preview(
    state: &AppState,
    node_id: i64,
    section_id: &str,
    conversation_id: Option<&str>,
    question: &str,
) -> Result<AiExplainPreview, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let history = follow_up_history(state, node_id, section_id, conversation_id)?;
    ai_assist::ai_follow_up_preview_for_path(&path, section_id, &history, question)
}

#[cfg(feature = "agent_automation")]
fn reserve_follow_up_turn(
    state: &AppState,
    node_id: i64,
    section_id: &str,
    conversation_id: Option<&str>,
    question: &str,
) -> Result<ReservedAiFollowUp, String> {
    let mut store = state
        .ai_followups
        .lock()
        .map_err(|_| "The follow-up memory is unavailable.".to_string())?;
    let now = u128::from(now_millis());
    let id = if let Some(id) = conversation_id.filter(|value| !value.trim().is_empty()) {
        id.to_string()
    } else {
        if store.conversations.len() >= AI_FOLLOW_UP_MAX_CONVERSATIONS {
            if let Some(oldest) = store
                .conversations
                .iter()
                .min_by_key(|(_, conversation)| conversation.touched_ms)
                .map(|(id, _)| id.clone())
            {
                store.conversations.remove(&oldest);
            }
        }
        let digest =
            blake3::hash(format!("{node_id}:{section_id}:{now}:{question}").as_bytes()).to_hex();
        let id = format!("follow-up-{}", &digest[..16]);
        store.conversations.insert(
            id.clone(),
            AiFollowUpConversation {
                node_id,
                section_id: section_id.to_string(),
                exchanges: Vec::new(),
                touched_ms: now,
            },
        );
        id
    };
    let conversation = store.conversations.get_mut(&id).ok_or_else(|| {
        "That follow-up expired. Start a new question from the section.".to_string()
    })?;
    if conversation.node_id != node_id || conversation.section_id != section_id {
        return Err("That follow-up belongs to a different file section.".to_string());
    }
    if conversation
        .exchanges
        .iter()
        .any(|exchange| exchange.answer.is_none())
    {
        return Err(
            "Wait for the current follow-up answer before asking another question.".to_string(),
        );
    }
    if conversation.exchanges.len() >= AI_FOLLOW_UP_MAX_TURNS {
        return Err("This follow-up reached its three-turn limit.".to_string());
    }
    let history = conversation
        .exchanges
        .iter()
        .filter_map(|exchange| {
            exchange
                .answer
                .as_ref()
                .map(|answer| (exchange.question.clone(), answer.clone()))
        })
        .collect();
    conversation.exchanges.push(AiFollowUpExchange {
        question: question.trim().to_string(),
        answer: None,
    });
    conversation.touched_ms = now;
    Ok(ReservedAiFollowUp {
        conversation_id: id,
        history,
        turn: conversation.exchanges.len(),
    })
}

#[cfg(feature = "agent_automation")]
fn finish_follow_up_turn(
    state: &AppState,
    conversation_id: &str,
    turn: usize,
    answer: Option<&str>,
) {
    let Ok(mut store) = state.ai_followups.lock() else {
        return;
    };
    let mut remove_empty = false;
    if let Some(conversation) = store.conversations.get_mut(conversation_id) {
        if let Some(exchange) = conversation.exchanges.get_mut(turn.saturating_sub(1)) {
            if let Some(answer) = answer {
                exchange.answer = Some(answer.to_string());
                conversation.touched_ms = u128::from(now_millis());
            } else if exchange.answer.is_none() && turn == conversation.exchanges.len() {
                conversation.exchanges.pop();
                remove_empty = conversation.exchanges.is_empty();
            }
        }
    }
    if remove_empty {
        store.conversations.remove(conversation_id);
    }
}

/// Read-only, section-scoped follow-up with a backend-enforced three-turn cap.
/// The exchange history is in-memory only and disappears when the app exits.
#[cfg(feature = "agent_automation")]
pub fn ai_follow_up(
    state: &AppState,
    node_id: i64,
    section_id: &str,
    conversation_id: Option<&str>,
    question: &str,
    level: &str,
    model: &str,
) -> Result<AiFollowUpResult, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let reservation =
        reserve_follow_up_turn(state, node_id, section_id, conversation_id, question)?;
    let answer = ai_assist::ai_follow_up_for_path(
        &path,
        section_id,
        &reservation.history,
        question,
        level,
        &config,
    );
    match answer {
        Ok(answer) => {
            finish_follow_up_turn(
                state,
                &reservation.conversation_id,
                reservation.turn,
                Some(&answer),
            );
            Ok(AiFollowUpResult {
                conversation_id: reservation.conversation_id,
                section_id: section_id.to_string(),
                turn: reservation.turn as u8,
                remaining_turns: AI_FOLLOW_UP_MAX_TURNS.saturating_sub(reservation.turn) as u8,
                answer,
            })
        }
        Err(error) => {
            finish_follow_up_turn(state, &reservation.conversation_id, reservation.turn, None);
            Err(error)
        }
    }
}

#[cfg(feature = "agent_automation")]
const AI_GLOSSARY_SEEDS: &[(&str, &str)] = &[
    (
        "API",
        "A defined way for one part of software to request data or work from another.",
    ),
    ("array", "An ordered collection of values."),
    (
        "asynchronous",
        "Work that can finish later without blocking everything else.",
    ),
    ("boolean", "A value with two states, usually true or false."),
    (
        "branch",
        "A decision point where code follows one of several paths.",
    ),
    (
        "cache",
        "A temporary copy kept to make repeated work faster.",
    ),
    (
        "callback",
        "A function passed to other code to be called later.",
    ),
    (
        "component",
        "A self-contained part of a user interface or system.",
    ),
    (
        "database",
        "Structured storage that software can query and update.",
    ),
    (
        "dependency",
        "Another package or component this project relies on.",
    ),
    ("function", "A named block of behaviour that can be called."),
    ("hash", "A short fingerprint calculated from content."),
    (
        "object",
        "A value that groups named fields and related data.",
    ),
    (
        "parser",
        "Code that turns text or bytes into a structured form.",
    ),
    (
        "runtime",
        "The environment and time in which a program is running.",
    ),
    (
        "state",
        "Data that describes the current condition of a program or screen.",
    ),
    ("variable", "A named place that holds a value."),
];

#[cfg(feature = "agent_automation")]
fn glossary_seeds() -> Vec<AiGlossaryEntry> {
    AI_GLOSSARY_SEEDS
        .iter()
        .map(|(term, definition)| AiGlossaryEntry {
            term: (*term).to_string(),
            definition: (*definition).to_string(),
            count: 0,
        })
        .collect()
}

#[cfg(feature = "agent_automation")]
pub fn ai_glossary_state(state: &AppState) -> Result<AiGlossaryState, String> {
    let db = state.db()?;
    Ok(AiGlossaryState {
        enabled: db.ai_glossary_enabled_value().map_err(to_message)?,
        seeds: glossary_seeds(),
        entries: db.ai_glossary_entries().map_err(to_message)?,
    })
}

#[cfg(feature = "agent_automation")]
pub fn set_ai_glossary_enabled(state: &AppState, enabled: bool) -> Result<AiGlossaryState, String> {
    state
        .db()?
        .set_ai_glossary_enabled(enabled)
        .map_err(to_message)?;
    ai_glossary_state(state)
}

/// Record only canonical local seed entries. The webview cannot supply a
/// definition, code excerpt, or path to the durable glossary.
#[cfg(feature = "agent_automation")]
pub fn ai_glossary_record(state: &AppState, terms: Vec<String>) -> Result<AiGlossaryState, String> {
    if terms.is_empty() || terms.len() > 12 {
        return Err("Choose between one and twelve glossary terms.".to_string());
    }
    let db = state.db()?;
    if !db.ai_glossary_enabled_value().map_err(to_message)? {
        return Err("Personal glossary persistence is off.".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for requested in terms {
        let requested = requested.trim();
        if !seen.insert(requested.to_ascii_lowercase()) {
            continue;
        }
        let (term, definition) = AI_GLOSSARY_SEEDS
            .iter()
            .find(|(term, _)| term.eq_ignore_ascii_case(requested))
            .ok_or_else(|| "Only terms from the local seed dictionary can be saved.".to_string())?;
        db.ai_glossary_record(term, definition)
            .map_err(to_message)?;
    }
    ai_glossary_state(state)
}

#[cfg(feature = "agent_automation")]
pub fn ai_annotation_add(
    state: &AppState,
    node_id: i64,
    snippet: &str,
    note: &str,
) -> Result<CodeAnnotation, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let source = ai_assist::annotation_source_for_path(&path)?;
    let (line_start, line_end) = ai_assist::unique_snippet_line_range(&source, snippet)?;
    let snippet_hash = ai_assist::hash_snippet(snippet);
    state
        .db()?
        .code_annotation_add(node_id, &snippet_hash, line_start, line_end, snippet, note)
        .map_err(to_message)
}

#[cfg(feature = "agent_automation")]
pub fn ai_annotations_for_node(
    state: &AppState,
    node_id: i64,
) -> Result<Vec<CodeAnnotation>, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let source = ai_assist::annotation_source_for_path(&path)?;
    let stored = state
        .db()?
        .code_annotations_for_node(node_id)
        .map_err(to_message)?;
    Ok(stored
        .into_iter()
        .map(|stored| {
            let mut annotation = stored.annotation;
            if ai_assist::hash_snippet(&stored.snippet) != annotation.snippet_hash {
                annotation.anchor_state = "stale".to_string();
                return annotation;
            }
            let matches: Vec<usize> = source
                .match_indices(&stored.snippet)
                .map(|(index, _)| index)
                .collect();
            match matches.as_slice() {
                [] => annotation.anchor_state = "stale".to_string(),
                [start] => {
                    let line_start = source[..*start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count() as u64
                        + 1;
                    let line_end = line_start
                        + stored.snippet.bytes().filter(|byte| *byte == b'\n').count() as u64;
                    annotation.anchor_state =
                        if line_start == annotation.line_start && line_end == annotation.line_end {
                            "current".to_string()
                        } else {
                            "moved".to_string()
                        };
                    annotation.line_start = line_start;
                    annotation.line_end = line_end;
                }
                _ => annotation.anchor_state = "ambiguous".to_string(),
            }
            annotation
        })
        .collect())
}

#[cfg(feature = "agent_automation")]
pub fn ai_annotation_delete(
    state: &AppState,
    node_id: i64,
    annotation_id: i64,
) -> Result<bool, String> {
    resolve_ai_explain_inventory_target(state, node_id)?;
    state
        .db()?
        .code_annotation_delete(annotation_id, node_id)
        .map_err(to_message)
}

/// Read-only preview of the exact bounded retrospective change context that an
/// AI narration/review send would use. The backend reconstructs and filters the
/// Recap again; no webview-supplied diff body is trusted.
#[cfg(feature = "agent_automation")]
pub fn ai_change_set_preview(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
    file_path: Option<&str>,
    edit_index: Option<usize>,
) -> Result<AiExplainPreview, String> {
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    ai_assist::ai_change_set_preview(&change_set, file_path, edit_index)
}

/// Tell the evidence-led story of a retrospectively reconstructed Recap. This
/// calls the configured explanation provider only and has no mutation path.
#[cfg(feature = "agent_automation")]
pub fn ai_narrate_session_changes(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    ai_assist::ai_narrate_change_set(&change_set, level, &config)
}

/// Teach the user how to read one selected recorded edit. File and edit are
/// resolved from a fresh backend Recap, never from an untrusted webview body.
#[cfg(feature = "agent_automation")]
pub struct AiRecordedEditSelector<'a> {
    pub file_path: &'a str,
    pub edit_index: usize,
}

#[cfg(feature = "agent_automation")]
pub fn ai_explain_change(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
    edit: AiRecordedEditSelector<'_>,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    ai_assist::ai_explain_recorded_change(
        &change_set,
        edit.file_path,
        edit.edit_index,
        level,
        &config,
    )
}

/// Ask evidence-grounded review questions over a reconstructed change set. The
/// command is advisory only and cannot execute or write project content.
#[cfg(feature = "agent_automation")]
pub fn ai_review_change_set(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
    level: &str,
    model: &str,
) -> Result<String, String> {
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    ai_assist::ai_review_change_set_with_config(&change_set, level, &config)
}

#[cfg(feature = "agent_automation")]
const AI_REWRITE_PROPOSAL_CAP: usize = 20;
#[cfg(feature = "agent_automation")]
const AI_REWRITE_PROPOSAL_TTL_MS: u128 = 30 * 60 * 1000;

/// Ask the configured provider for one replacement, but stage it only in memory. Rust freshly
/// reads and gates the complete file, requires a unique exact selection, validates the proposed
/// full-file result, and returns a local proposal for explicit review. This command never writes.
#[cfg(feature = "agent_automation")]
pub fn ai_rewrite_text(
    state: &AppState,
    node_id: i64,
    snippet: &str,
    instruction: &str,
    level: &str,
    model: &str,
) -> Result<AiRewriteProposal, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let staged =
        ai_assist::ai_rewrite_text_with_config(snippet, &path, instruction, level, &config)?;
    value_edit::validate_content_after_edit(&path, &staged.source)?;
    let (start, end) = unique_selection_span(&staged.source, snippet)?;
    let mut candidate =
        String::with_capacity(staged.source.len() - snippet.len() + staged.replacement.len());
    candidate.push_str(&staged.source[..start]);
    candidate.push_str(&staged.replacement);
    candidate.push_str(&staged.source[end..]);
    value_edit::validate_content_after_edit(&path, &candidate)?;

    let now = u128::from(now_millis());
    let source_hash = blake3::hash(staged.source.as_bytes()).to_hex().to_string();
    let digest = blake3::hash(
        format!(
            "{node_id}:{now}:{source_hash}:{}",
            blake3::hash(staged.replacement.as_bytes()).to_hex()
        )
        .as_bytes(),
    )
    .to_hex();
    let proposal_id = format!("proposal-{}", &digest[..20]);
    let proposal = AiRewriteProposal {
        proposal_id: proposal_id.clone(),
        session_id: format!("ai-edit-{}", &digest[..16]),
        node_id,
        language: staged.language,
        original: snippet.to_string(),
        replacement: staged.replacement.clone(),
        summary: selection_change_summary(snippet, &staged.replacement),
    };
    let pending = PendingAiRewriteProposal {
        proposal: proposal.clone(),
        source_hash,
        created_ms: now,
    };
    let mut store = state
        .ai_rewrite_proposals
        .lock()
        .map_err(|_| "The proposed change could not be staged.".to_string())?;
    store
        .proposals
        .retain(|_, item| now.saturating_sub(item.created_ms) <= AI_REWRITE_PROPOSAL_TTL_MS);
    if store.proposals.len() >= AI_REWRITE_PROPOSAL_CAP {
        if let Some(oldest) = store
            .proposals
            .iter()
            .min_by_key(|(_, item)| item.created_ms)
            .map(|(id, _)| id.clone())
        {
            store.proposals.remove(&oldest);
        }
    }
    store.proposals.insert(proposal_id, pending);
    Ok(proposal)
}

#[cfg(feature = "agent_automation")]
fn unique_selection_span(source: &str, snippet: &str) -> Result<(usize, usize), String> {
    if snippet.is_empty() {
        return Err("No text was selected.".to_string());
    }
    let mut matches = source.match_indices(snippet);
    let (start, _) = matches
        .next()
        .ok_or_else(|| "The selected text is no longer present in the file.".to_string())?;
    if matches.next().is_some() {
        return Err(
            "That text appears more than once. Select a slightly larger unique passage."
                .to_string(),
        );
    }
    Ok((start, start + snippet.len()))
}

#[cfg(feature = "agent_automation")]
fn selection_change_summary(original: &str, replacement: &str) -> String {
    let before_lines = original.lines().count().max(1);
    let after_lines = replacement.lines().count().max(1);
    let before_chars = original.chars().count();
    let after_chars = replacement.chars().count();
    let size_change = match after_chars.cmp(&before_chars) {
        std::cmp::Ordering::Greater => format!("{} characters longer", after_chars - before_chars),
        std::cmp::Ordering::Less => format!("{} characters shorter", before_chars - after_chars),
        std::cmp::Ordering::Equal => "the same length".to_string(),
    };
    format!(
        "Only this selected passage changes: {before_lines} line(s) become {after_lines} line(s), {size_change}. Everything outside the selection stays byte-for-byte unchanged."
    )
}

/// Apply one staged provider proposal locally. Rust re-reads the complete file, verifies
/// whole-file CAS and the unique anchor, splices by byte, validates the result, and creates a
/// verified durable snapshot tagged with the AI edit session.
#[cfg(feature = "agent_automation")]
pub fn apply_ai_suggestion(
    state: &AppState,
    proposal_id: &str,
) -> Result<AiSuggestionApplyResult, String> {
    let pending = state
        .ai_rewrite_proposals
        .lock()
        .map_err(|_| "The proposed change is unavailable.".to_string())?
        .proposals
        .get(proposal_id)
        .cloned()
        .ok_or_else(|| "That proposed change expired. Ask for it again.".to_string())?;
    if u128::from(now_millis()).saturating_sub(pending.created_ms) > AI_REWRITE_PROPOSAL_TTL_MS {
        return Err("That proposed change expired. Ask for it again.".to_string());
    }
    let path = resolve_ai_explain_target(state, pending.proposal.node_id)?;
    let bytes = fs::read(&path)
        .map_err(|error| format!("Not applied: the file could not be read ({error})."))?;
    if bytes.len() > 60 * 1024 {
        return Err(
            "Not applied: the file is now above the 60 KB safe replacement limit.".to_string(),
        );
    }
    let source = String::from_utf8(bytes)
        .map_err(|_| "Not applied: the file is no longer UTF-8 text.".to_string())?;
    let source_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    if source_hash != pending.source_hash {
        return Err(
            "Not applied: the file changed after this proposal was created. Reload and ask again."
                .to_string(),
        );
    }
    let (start, end) = unique_selection_span(&source, &pending.proposal.original)?;
    let mut content = String::with_capacity(
        source.len() - pending.proposal.original.len() + pending.proposal.replacement.len(),
    );
    content.push_str(&source[..start]);
    content.push_str(&pending.proposal.replacement);
    content.push_str(&source[end..]);
    value_edit::validate_content_after_edit(&path, &content)?;
    let outcome = edit_snapshot::write_file_with_snapshot(
        state,
        pending.proposal.node_id,
        &content,
        "ai_suggestion",
        Some(&pending.proposal.session_id),
        Some(&pending.source_hash),
    )?;
    state
        .ai_rewrite_proposals
        .lock()
        .map_err(|_| {
            "The change was saved, but its temporary proposal could not be cleared.".to_string()
        })?
        .proposals
        .remove(proposal_id);
    let mut message =
        "One selected change was applied. The verified pre-session version is available to undo."
            .to_string();
    if let Some(warning) = outcome.ledger_warning {
        message.push(' ');
        message.push_str(&warning);
    }
    Ok(AiSuggestionApplyResult {
        node_id: pending.proposal.node_id,
        snapshot_id: outcome.snapshot_id,
        session_id: pending.proposal.session_id,
        message,
    })
}

/// Optional AI-enriched project summary, built from the SAME local context the no-network summary
/// uses (README excerpt / manifests / run commands / file list) and sent to the configured provider
/// through the secret send-gate. Off unless a provider is configured (`resolve_ai_provider_config`
/// hard-errors on `off`). `model` is an optional per-call override.
#[cfg(feature = "agent_automation")]
pub fn ai_summarize_project(
    state: &AppState,
    project_id: i64,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiProjectSummary, String> {
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    let context = project_ai_summary_context(state, project_id)?;
    let estimated_input_tokens = ai_assist::estimate_tokens(&context);
    let summary = ai_assist::ai_summarize_project_with_config(&context, level, &config)?;
    Ok(hangar_core::AiProjectSummary {
        summary,
        estimated_input_tokens,
        model: config.model.clone(),
    })
}

/// Local-only summary send preview. It assembles and gates the exact project context without
/// resolving a provider or making a request, so the user can see size/blockers first.
#[cfg(feature = "agent_automation")]
pub fn ai_summarize_project_preview(
    state: &AppState,
    project_id: i64,
    level: &str,
) -> Result<AiExplainPreview, String> {
    let context = project_ai_summary_context(state, project_id)?;
    Ok(ai_assist::ai_summarize_project_preview(&context, level))
}

/// Assemble, gate and disclose the literal provider request for a project summary.
/// This is local-only preparation: it does not contact the configured provider.
#[cfg(feature = "agent_automation")]
pub fn ai_summarize_project_disclosure(
    state: &AppState,
    project_id: i64,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    let context = project_ai_summary_context(state, project_id)?;
    ai_assist::ai_summarize_project_disclosure_with_config(&context, level, &config)
}

#[cfg(feature = "agent_automation")]
fn project_ai_summary_context(state: &AppState, project_id: i64) -> Result<String, String> {
    let project = project_get(state, project_id)?
        .ok_or_else(|| "That project is no longer registered in Code Hangar.".to_string())?;
    let local = project_summary::project_context_summary(&project.path);
    // Recommended context files are ordered strongest-first. Reads are best-effort and every
    // candidate is independently gated again inside `project_ai_context_text`.
    let context_files = project_context_files(state, project_id).unwrap_or_default();
    Ok(project_ai_context_text(
        &local,
        &project.path,
        &context_files,
    ))
}

/// How many curated context files may contribute a bounded excerpt to one summary prompt. The
/// listing is already ordered best-first (README/AGENTS/CLAUDE, then docs…), so the top few carry
/// the most signal; capping keeps prompt size and cost bounded regardless of project size.
#[cfg(feature = "agent_automation")]
const AI_SUMMARY_EXCERPT_FILES: usize = 4;

/// Assemble the AI-summary context from the local (no-network) project summary AND the curated
/// "Recommended context" files (`context_files`, keyed off `is_context_path`).
///
/// Two levels of context-file signal are folded in:
/// * NAMES/paths of the recommended files — always safe (a path is metadata, not file bytes) and
///   the single best hint about what a project is.
/// * Bounded EXCERPTS of the top few — but ONLY through the SAME send-gate AI Assist uses. Files the
///   inventory already flagged `is_sensitive` or `protected_level` are skipped without a read (honor
///   the Protected Zone); the rest go through `ai_assist::gated_context_excerpt`, which re-applies
///   the sensitive-path + secret + binary gate on the exact candidate bytes and yields nothing for
///   anything it would block. `project_root` resolves each listing-relative path back to disk.
///
/// The fully-assembled string is re-scanned by `ai_summarize_project_with_config` before it leaves
/// the machine, so no raw secret or Protected-file byte can reach a provider even if a gate above
/// were somehow bypassed — this stacks with, and never replaces, that final barrier.
#[cfg(feature = "agent_automation")]
fn project_ai_context_text(
    summary: &hangar_core::ProjectContextSummary,
    project_root: &str,
    context_files: &[ContextFile],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = &summary.readme_title {
        parts.push(format!("Title: {title}"));
    }
    if !summary.kinds.is_empty() {
        parts.push(format!("Detected stack: {}", summary.kinds.join(", ")));
    }
    if !summary.manifest_files.is_empty() {
        parts.push(format!("Manifests: {}", summary.manifest_files.join(", ")));
    }
    if !summary.run_commands.is_empty() {
        parts.push(format!("Run commands: {}", summary.run_commands.join(", ")));
    }
    if !summary.markdown_files.is_empty() {
        parts.push(format!("Docs: {}", summary.markdown_files.join(", ")));
    }

    // Curated recommended-context file NAMES: the strongest signal about intent. Filter out
    // inventory-flagged sensitive/Protected files AT THE SOURCE so neither their PATH (the list
    // below) nor their bytes (the excerpt loop below) can ever reach the prompt — a name like
    // `docs/credentials.md` is itself a leak, so withholding only the body is not enough. The
    // per-file gate in the excerpt loop is kept as defense-in-depth.
    let recommended: Vec<&ContextFile> = context_files
        .iter()
        .filter(|file| file.recommended && !file.is_sensitive && file.protected_level.is_none())
        .collect();
    if !recommended.is_empty() {
        let names: Vec<&str> = recommended.iter().map(|file| file.path.as_str()).collect();
        parts.push(format!("Recommended context files: {}", names.join(", ")));
    }

    if let Some(excerpt) = &summary.readme_excerpt {
        parts.push(format!("README excerpt:\n{excerpt}"));
    }

    // Bounded, gated excerpts of the top recommended files (README excerpt above already covers the
    // README, so skip it here to avoid duplication). Each excerpt passes the SAME send-gate as a
    // file explain; inventory-flagged sensitive/Protected files are never even read.
    let root = Path::new(project_root);
    for file in recommended
        .iter()
        .filter(|file| !file.display_name.eq_ignore_ascii_case("readme.md"))
        .take(AI_SUMMARY_EXCERPT_FILES)
    {
        // Honor the inventory's own classification before touching disk — the Protected Zone /
        // sensitive flags come from the DB (which the path-only gate below cannot see on its own).
        if file.is_sensitive || file.protected_level.is_some() {
            continue;
        }
        let absolute = root.join(&file.path);
        if let Some(excerpt) = ai_assist::gated_context_excerpt(&absolute.to_string_lossy()) {
            parts.push(format!("Excerpt from {}:\n{excerpt}", file.path));
        }
    }

    parts.join("\n\n")
}

/// Resolve the stored AI provider settings into a ready-to-send `ProviderConfig`, or error if
/// AI Assist is off or unconfigured. `off` is the hard guarantee that nothing is contacted until
/// the user configures a provider. Loopback for `local` is enforced again inside `hangar-ai`.
#[cfg(feature = "agent_automation")]
fn resolve_ai_provider_config(state: &AppState) -> Result<hangar_ai::ProviderConfig, String> {
    let stored = state.db()?.ai_provider_config().map_err(to_message)?;
    let local = match stored.mode.as_str() {
        "off" => {
            return Err(
                "AI Assist is turned off. Choose a local model or an API provider in Settings ▸ AI Assist."
                    .to_string(),
            )
        }
        "local" => true,
        "api" => false,
        other => return Err(format!("Unknown AI provider mode \"{other}\".")),
    };
    if stored.base_url.trim().is_empty() {
        return Err("No AI provider endpoint is set. Add one in Settings ▸ AI Assist.".to_string());
    }
    Ok(hangar_ai::ProviderConfig {
        base_url: stored.base_url,
        model: stored.model,
        format: hangar_ai::ProviderFormat::from_tag(&stored.format),
        local,
    })
}

#[cfg(feature = "agent_automation")]
fn resolve_ai_provider_with_model(
    state: &AppState,
    model: &str,
) -> Result<hangar_ai::ProviderConfig, String> {
    let mut config = resolve_ai_provider_config(state)?;
    let model = model.trim();
    if !model.is_empty() {
        config.model = model.to_string();
    }
    if config.model.trim().is_empty() {
        return Err(
            "No model is set for the AI provider. Add one in Settings ▸ AI Assist.".to_string(),
        );
    }
    Ok(config)
}

/// The current AI provider configuration (mode/base_url/model/format). The API key is never
/// included — it lives only in the OS keychain.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_get(state: &AppState) -> Result<AiProviderConfig, String> {
    let mut stored = state.db()?.ai_provider_config().map_err(to_message)?;
    // Normalize any legacy format tag (openai_compatible/anthropic) to its canonical form before
    // it reaches the UI, so the frontend only ever sees the current tags.
    stored.format = hangar_ai::ProviderFormat::from_tag(&stored.format)
        .as_tag()
        .to_string();
    Ok(stored)
}

/// Persist the AI provider configuration. Validates the mode/format and, for a local provider,
/// rejects a non-loopback endpoint at persist time so a bad URL can never be saved.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_set(
    state: &AppState,
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<(), String> {
    let mode = mode.trim();
    if !matches!(mode, "off" | "local" | "api") {
        return Err(format!("Unknown AI provider mode \"{mode}\"."));
    }
    let format = format.trim();
    if !matches!(
        format,
        "chat_completions" | "messages_api" | "openai_compatible" | "anthropic"
    ) {
        return Err(format!("Unknown AI provider format \"{format}\"."));
    }
    // Store the canonical tag so a legacy value (openai_compatible/anthropic) converges to the
    // current form on the next save.
    let format = hangar_ai::ProviderFormat::from_tag(format).as_tag();
    let base_url = base_url.trim();
    if mode != "off" && base_url.is_empty() {
        return Err("Enter the provider endpoint URL.".to_string());
    }
    if mode == "local" {
        hangar_ai::validate_local_endpoint(base_url)?;
    } else if mode == "api" {
        // A remote endpoint later gets the saved Bearer/x-api-key attached, so a
        // cleartext `http://` base must never be persisted (https, or http to a
        // loopback gateway on this machine, only).
        hangar_ai::validate_remote_endpoint(base_url)?;
    }
    // Switching a remote provider to a DIFFERENT host must not silently ship the old
    // provider's key to the new one. If the (api-mode) endpoint origin changes vs the
    // previously stored config, drop the saved key so the user is prompted to enter the
    // new provider's key (needs-key status surfaces automatically once the key is gone).
    let db = state.db()?;
    if mode == "api" {
        let previous = db.ai_provider_config().map_err(to_message)?;
        if remote_host_changed(&previous.base_url, base_url) {
            // Best-effort: an absent key (nothing to clear) is not an error.
            let _ = ai_assist::ai_key_clear();
        }
    }
    let config = AiProviderConfig {
        mode: mode.to_string(),
        base_url: base_url.to_string(),
        model: model.trim().to_string(),
        format: format.to_string(),
    };
    db.set_ai_provider_config(&config).map_err(to_message)
}

/// Whether switching a remote endpoint from `previous` to `next` targets a DIFFERENT origin,
/// meaning a saved API key would be sent to a new host and must be dropped. Origins are compared
/// with the `url` crate reqwest dials with (never a hand-rolled split, which the backslash-bypass
/// note on `is_loopback_url` shows is unsafe), so the "same host?" decision cannot diverge from
/// what actually gets connected to.
///
/// Only a change BETWEEN two comparable origins clears the key. A prior value with no comparable
/// origin (empty base_url on a fresh install, a former off/local mode whose loopback URL still
/// parses to its own origin — so that IS compared — or an unparseable stored URL) is treated as a
/// FIRST set, not a change: clearing there would wipe a key the user just entered for this very
/// provider, and no cross-provider leak is possible because there was no prior remote host. An
/// unparseable `next` (which endpoint validation already rejected upstream) is likewise no change.
#[cfg(feature = "agent_automation")]
fn remote_host_changed(previous: &str, next: &str) -> bool {
    match (
        hangar_ai::endpoint_origin(previous),
        hangar_ai::endpoint_origin(next),
    ) {
        (Some(prev), Some(next)) => prev != next,
        // No comparable prior remote origin -> first set, not a host change.
        (None, _) => false,
        // `next` did not parse (already rejected by validation) -> nothing to clear.
        (_, None) => false,
    }
}

/// Build a provider config from explicit fields (the on-screen draft), WITHOUT touching the
/// stored config. Used by the read-only Test/Models probes so checking connectivity never
/// overwrites the user's saved provider.
#[cfg(feature = "agent_automation")]
fn build_ai_provider_config(
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<hangar_ai::ProviderConfig, String> {
    let local = match mode.trim() {
        "off" => {
            return Err("Choose a local model or an API provider first.".to_string());
        }
        "local" => true,
        "api" => false,
        other => return Err(format!("Unknown AI provider mode \"{other}\".")),
    };
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return Err("Enter the provider endpoint URL.".to_string());
    }
    if local {
        hangar_ai::validate_local_endpoint(base_url)?;
    } else {
        // The Test/Models probes attach the saved key exactly like a real call, so
        // the draft endpoint is held to the same https-or-loopback rule as persist.
        hangar_ai::validate_remote_endpoint(base_url)?;
    }
    Ok(hangar_ai::ProviderConfig {
        base_url: base_url.to_string(),
        model: model.trim().to_string(),
        format: hangar_ai::ProviderFormat::from_tag(format.trim()),
        local,
    })
}

/// Reachability check for a provider DRAFT (a fixed "ping", no file/user content). Non-destructive
/// — it probes the supplied fields without persisting them, so the saved config is untouched.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_test(
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<String, String> {
    let config = build_ai_provider_config(mode, base_url, model, format)?;
    ai_assist::ai_provider_test_with_config(&config)
}

/// Best-effort model list for a provider DRAFT (empty unless an OpenAI-compatible endpoint with a
/// reachable `/models`). Non-destructive; drives an optional dropdown (UI falls back to free text).
#[cfg(feature = "agent_automation")]
pub fn ai_provider_models(
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<Vec<String>, String> {
    let config = build_ai_provider_config(mode, base_url, model, format)?;
    ai_assist::ai_provider_models_with_config(&config)
}

/// User-triggered loopback-only discovery. The fixed probes use 127.0.0.1, short timeouts and no
/// proxy/key; this command is never called during startup or settings mount.
#[cfg(feature = "agent_automation")]
pub fn ai_local_discover() -> Vec<hangar_core::AiLocalProviderCandidate> {
    hangar_ai::discover_local_providers()
        .into_iter()
        .map(|candidate| hangar_core::AiLocalProviderCandidate {
            label: candidate.label,
            base_url: candidate.base_url,
            format: hangar_ai::ProviderFormat::ChatCompletions
                .as_tag()
                .to_string(),
            models: candidate.models,
        })
        .collect()
}

/// Aggregate estimated model usage for this process session. The optional projection lets the UI
/// warn before a send; the threshold is advisory and never turns into a hidden hard block.
#[cfg(feature = "agent_automation")]
pub fn ai_usage_status(
    projected_input_tokens: Option<u64>,
    projected_output_tokens: Option<u64>,
) -> hangar_ai::AiUsageStatus {
    let projected_output_allowance = projected_input_tokens
        .map(|_| {
            projected_output_tokens
                .unwrap_or(u64::from(ai_assist::MAX_TOKENS))
                .min(16_384)
        })
        .unwrap_or(0);
    hangar_ai::usage_status(
        projected_input_tokens.unwrap_or(0),
        projected_output_allowance,
    )
}

#[cfg(feature = "agent_automation")]
pub fn ai_usage_set_soft_cap(
    soft_cap_tokens: Option<u64>,
) -> Result<hangar_ai::AiUsageStatus, String> {
    hangar_ai::usage_set_soft_cap(soft_cap_tokens)
}

#[cfg(feature = "agent_automation")]
pub fn ai_usage_reset() -> hangar_ai::AiUsageStatus {
    hangar_ai::usage_reset()
}

pub fn open_node_external(state: &AppState, node_id: i64) -> Result<(), String> {
    let path = node_full_path(state, node_id)?;
    open_path_external(&path)
}

pub fn reveal_node_external(state: &AppState, node_id: i64) -> Result<(), String> {
    let path = node_full_path(state, node_id)?;
    reveal_path_external(&path)
}

pub fn reveal_project_external(state: &AppState, project_id: i64) -> Result<(), String> {
    let project = state
        .db()?
        .project_get(project_id)
        .map_err(to_message)?
        .ok_or_else(|| "This registered project is no longer available.".to_string())?;
    reveal_path_external(&project.path)
}

fn open_path_external(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.exists() {
        return Err("Path no longer exists on disk.".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        // Launch Explorer directly. Routing through `cmd /C start` would treat
        // shell metacharacters in a valid filename as commands.
        std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map_err(|err| format!("Could not open path with Windows: {err}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(path)
            .spawn()
            .map_err(|err| format!("Could not open path with the operating system: {err}"))?;
        Ok(())
    }
}

fn reveal_path_external(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.exists() {
        return Err("Path no longer exists on disk.".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer.exe");
        if path.is_dir() {
            command.arg(path);
        } else {
            command.arg(format!("/select,{}", path.to_string_lossy()));
        }
        command
            .spawn()
            .map_err(|err| format!("Could not show path in File Explorer: {err}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new(opener)
            .arg(folder)
            .spawn()
            .map_err(|err| format!("Could not show path with the operating system: {err}"))?;
        Ok(())
    }
}

pub fn dashboard_summary(state: &AppState) -> Result<DashboardSummary, String> {
    state.db()?.dashboard_summary().map_err(to_message)
}

pub fn dashboard_summary_filtered(
    state: &AppState,
    include_fixture_projects: bool,
) -> Result<DashboardSummary, String> {
    state
        .db()?
        .dashboard_summary_filtered(include_fixture_projects)
        .map_err(to_message)
}

pub fn adapters_list(state: &AppState) -> Result<Vec<AdapterSummary>, String> {
    state.db()?.adapters_list().map_err(to_message)
}

pub fn project_context_files(
    state: &AppState,
    project_id: i64,
) -> Result<Vec<ContextFile>, String> {
    state
        .db()?
        .project_context_files(project_id)
        .map_err(to_message)
}

pub fn file_preview(
    state: &AppState,
    node_id: i64,
    mode: PreviewMode,
    record_recent: Option<bool>,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    state
        .db()?
        .file_preview_with_policy(
            node_id,
            mode,
            record_recent.unwrap_or(true),
            policy.unwrap_or_default(),
        )
        .map_err(to_message)
}

pub fn file_reveal(
    state: &AppState,
    node_id: i64,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    state
        .db()?
        .file_reveal_with_policy(node_id, mode, policy.unwrap_or_default())
        .map_err(to_message)
}

pub fn quick_open(
    state: &AppState,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<QuickOpenResult>, String> {
    state
        .db()?
        .quick_open(&query, limit.unwrap_or(20))
        .map_err(to_message)
}

pub fn performance_set_mode(mode: Option<String>) -> Result<(), String> {
    performance::set_global_mode(PerformanceMode::parse(mode.as_deref()));
    Ok(())
}

pub fn system_resource_profile() -> SystemResourceProfile {
    performance::system_resource_profile()
}

pub fn process_resource_usage() -> ProcessResourceUsage {
    performance::process_resource_usage()
}

const SESSION_PREVIEW_MAX_BYTES: u64 = 256 * 1024;

/// Larger cap used only when tail-reading a Codex rollout `.jsonl`. These files can
/// be many MB (the readable turns live throughout, and the newest are at the very
/// end), and each turn is a long JSON line, so a bigger tail window surfaces
/// several recent turns instead of one. Still bounded so a huge rollout can never
/// pull an unbounded slice into memory.
const CODEX_ROLLOUT_TAIL_MAX_BYTES: u64 = 768 * 1024;
const CODEX_ROLLOUT_RENDER_SCAN_MAX_BYTES: u64 = 32 * 1024 * 1024;
const CODEX_ROLLOUT_RENDER_MAX_BYTES: usize = 512 * 1024;
const CODEX_ROLLOUT_RENDER_MAX_LINES: usize = 96;
const CODEX_ROLLOUT_RENDER_LINE_MAX_BYTES: usize = 256 * 1024;
const CODEX_ROLLOUT_CONTEXT_SCAN_MAX_BYTES: u64 = 512 * 1024 * 1024;
const CODEX_ROLLOUT_CONTEXT_SCAN_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
const CODEX_ROLLOUT_GAP_EVENT: &str = r#"{"type":"event_msg","payload":{"type":"session_gap","message":"Earlier activity between this request and the recent updates is omitted from this bounded preview."}}"#;

fn requested_session_preview_limit(
    size_bytes: u64,
    default_bytes: u64,
    max_bytes: Option<u64>,
    load_full: bool,
) -> u64 {
    if load_full {
        return size_bytes;
    }
    max_bytes
        .filter(|value| *value > 0)
        .unwrap_or(default_bytes)
        .min(size_bytes)
}

fn preview_limit_as_usize(limit_bytes: u64) -> usize {
    usize::try_from(limit_bytes).unwrap_or(usize::MAX)
}

/// Whether `path` is a Codex sessions rollout transcript:
/// `…/.codex/sessions/<date dirs>/rollout-*.jsonl` (also `archived_sessions`).
/// Detected by structure + filename so it works regardless of where the `.codex`
/// home lives. These are the files whose newest conversation turns sit at the END
/// of a multi-MB file, so the preview tail-reads them instead of head-reading.
fn is_codex_rollout_jsonl(path: &Path) -> bool {
    let is_jsonl = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));
    if !is_jsonl {
        return false;
    }
    let is_rollout = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("rollout-"));
    if !is_rollout {
        return false;
    }
    let lower: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    lower.windows(2).any(|pair| {
        pair[0] == ".codex" && (pair[1] == "sessions" || pair[1] == "archived_sessions")
    })
}

/// Whether the head of a preview window is binary rather than renderable text.
/// JSON/JSONL transcripts never contain a raw NUL (it must be escaped inside
/// JSON strings), while protobuf/SQLite/LevelDB session stores hit one within
/// their first bytes. A secondary control-byte ratio catches NUL-less binary
/// headers, while tolerating the tabs/newlines real transcripts contain. Only
/// the head is sniffed so trailing NUL padding from a torn append (a crashed
/// writer) does not reclassify an otherwise readable transcript.
fn looks_binary_session_head(buffer: &[u8]) -> bool {
    let head = &buffer[..buffer.len().min(4096)];
    if head.is_empty() {
        return false;
    }
    if head.contains(&0) {
        return true;
    }
    let control = head
        .iter()
        .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r'))
        .count();
    // More than 10% control bytes never happens in a text transcript.
    control * 10 > head.len()
}

/// Whether `path` is a `.jsonl` session transcript. Every JSONL session store
/// (Codex rollouts, Claude Code project transcripts, …) is append-ordered — the
/// newest turns sit at the END of the file — so an oversized one must be
/// tail-read: a head-read of a 400 MB Claude transcript shows only the oldest
/// fraction and the recent exchanges are unreachable.
fn is_jsonl_session_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

/// Read the bounded byte window the preview will render, returning
/// `(bytes, truncated)`.
///
/// For an oversized append-ordered `.jsonl` transcript (`is_jsonl` and bigger than
/// its cap) we seek to `len - cap` and read the LAST `cap` bytes so the newest
/// conversation turns are surfaced — these files append the latest turns at the
/// end, so a head-read would only ever show the oldest ones (for a Codex rollout,
/// often just `encrypted_content` blobs; for a huge Claude transcript, messages
/// from days ago). The first line of the tail window is a fragment cut by the
/// byte-offset seek, so it is dropped and the rendered text starts on a whole
/// line. `truncated` is then `true` because older turns were left out, mirroring
/// the Antigravity `.db` path. `is_rollout` only selects the larger Codex cap.
///
/// Every other case keeps the original head-read from the start of the file (up to
/// the standard cap), with `truncated` reflecting whether the file exceeded it.
#[cfg(test)]
fn read_session_preview_window(
    path: &Path,
    is_rollout: bool,
    is_jsonl: bool,
    size_bytes: u64,
) -> std::io::Result<(Vec<u8>, bool)> {
    let cap = if is_rollout {
        CODEX_ROLLOUT_TAIL_MAX_BYTES
    } else {
        SESSION_PREVIEW_MAX_BYTES
    };
    read_session_preview_window_with_limit(path, is_jsonl, size_bytes, cap)
}

fn read_session_preview_window_with_limit(
    path: &Path,
    is_jsonl: bool,
    size_bytes: u64,
    cap: u64,
) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::{Read, Seek};

    let cap = cap.min(size_bytes);
    let mut buffer = Vec::new();
    if is_jsonl && size_bytes > cap {
        let mut file = fs::File::open(path)?;
        file.seek(std::io::SeekFrom::Start(size_bytes - cap))?;
        file.take(cap).read_to_end(&mut buffer)?;
        // Drop everything up to and including the first newline: that leading
        // fragment is a partial line cut by the byte-offset seek. If the window
        // somehow holds no newline, keep it as-is rather than blanking the preview.
        if let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            buffer.drain(..=pos);
        }
        Ok((buffer, true))
    } else {
        fs::File::open(path)?.take(cap).read_to_end(&mut buffer)?;
        let truncated = size_bytes > buffer.len() as u64;
        Ok((buffer, truncated))
    }
}

fn json_contains_readable_text(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => !text.trim().is_empty(),
        serde_json::Value::Array(values) => values.iter().any(json_contains_readable_text),
        serde_json::Value::Object(object) => ["message", "text", "content"]
            .iter()
            .filter_map(|key| object.get(*key))
            .any(json_contains_readable_text),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadableCodexLineKind {
    EventUser,
    EventOther,
    ItemUser,
    ItemOther,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadableCodexStream {
    Event,
    Item,
}

impl ReadableCodexLineKind {
    fn is_user(self) -> bool {
        matches!(self, Self::EventUser | Self::ItemUser)
    }

    fn stream(self) -> ReadableCodexStream {
        match self {
            Self::EventUser | Self::EventOther => ReadableCodexStream::Event,
            Self::ItemUser | Self::ItemOther => ReadableCodexStream::Item,
        }
    }
}

fn readable_codex_rollout_line_kind(line: &str) -> Option<ReadableCodexLineKind> {
    let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
        return None;
    };
    let object = record.as_object()?;
    let payload = object
        .get("payload")
        .and_then(serde_json::Value::as_object)?;
    let outer_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let payload_type = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    match (outer_type, payload_type) {
        ("event_msg", "user_message" | "agent_message")
            if json_contains_readable_text(&serde_json::Value::Object(payload.clone())) =>
        {
            Some(if payload_type == "user_message" {
                ReadableCodexLineKind::EventUser
            } else {
                ReadableCodexLineKind::EventOther
            })
        }
        ("response_item", "message") => {
            let role = payload
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if matches!(role, "user" | "assistant" | "system")
                && json_contains_readable_text(&serde_json::Value::Object(payload.clone()))
            {
                Some(if role == "user" {
                    ReadableCodexLineKind::ItemUser
                } else {
                    ReadableCodexLineKind::ItemOther
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_readable_codex_rollout_line(line: &str) -> bool {
    readable_codex_rollout_line_kind(line).is_some()
}

/// Extract every readable conversation record from an explicitly expanded raw
/// rollout window. The initial preview keeps the tighter contextual recovery
/// below; once the user asks for more, this preserves the whole requested
/// conversation window instead of retaining the fixed 96-line preview cap.
fn expanded_codex_rendered_window(buffer: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buffer);
    let mut event_lines = Vec::new();
    let mut item_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(kind) = readable_codex_rollout_line_kind(line) else {
            continue;
        };
        match kind.stream() {
            ReadableCodexStream::Event => event_lines.push((kind, line)),
            ReadableCodexStream::Item => item_lines.push((kind, line)),
        }
    }

    let mut selected = if event_lines.iter().any(|(kind, _)| kind.is_user()) {
        event_lines
    } else if item_lines.iter().any(|(kind, _)| kind.is_user()) {
        item_lines
    } else if !event_lines.is_empty() {
        event_lines
    } else {
        item_lines
    };
    if let Some(first_user) = selected.iter().position(|(kind, _)| kind.is_user()) {
        selected.drain(..first_user);
    }
    (!selected.is_empty()).then(|| {
        selected
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn append_jsonl_record(output: &mut String, record: &str) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(record);
}

fn generic_session_role(value: &serde_json::Value) -> Option<&'static str> {
    let object = value.as_object()?;
    let message = object.get("message").and_then(serde_json::Value::as_object);
    let role = object
        .get("role")
        .and_then(serde_json::Value::as_str)
        .or_else(|| message?.get("role").and_then(serde_json::Value::as_str))
        .or_else(|| object.get("type").and_then(serde_json::Value::as_str))
        .or_else(|| object.get("sender").and_then(serde_json::Value::as_str))
        .or_else(|| object.get("author").and_then(serde_json::Value::as_str))
        .or_else(|| object.get("from").and_then(serde_json::Value::as_str))?;
    match role.to_ascii_lowercase().as_str() {
        "user" | "human" => Some("user"),
        "assistant" | "ai" | "model" | "bot" => Some("assistant"),
        "system" => Some("system"),
        _ => None,
    }
}

fn collect_generic_session_content(
    value: &serde_json::Value,
    depth: usize,
    output: &mut Vec<String>,
) {
    if depth > 6 {
        return;
    }
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            output.push(text.trim().to_string());
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_generic_session_content(value, depth + 1, output);
            }
        }
        serde_json::Value::Object(object) => {
            let kind = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(
                kind.as_str(),
                "tool_result" | "tool_output" | "thinking" | "reasoning"
            ) {
                return;
            }
            if matches!(kind.as_str(), "tool_use" | "tool_call" | "function_call") {
                if let Some(name) = object
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    output.push(format!("↳ used {}", name.trim()));
                }
                return;
            }
            if let Some(text) = object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
            {
                output.push(text.trim().to_string());
                return;
            }
            if let Some(content) = object.get("content") {
                collect_generic_session_content(content, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn sanitized_generic_session_record(line: &str) -> Option<String> {
    const SKIP_TYPES: [&str; 8] = [
        "queue-operation",
        "summary",
        "file-history-snapshot",
        "snapshot",
        "attachment",
        "last-prompt",
        "ai-title",
        "mode",
    ];
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let object = value.as_object()?;
    let record_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if SKIP_TYPES.contains(&record_type.as_str()) {
        return None;
    }
    let role = generic_session_role(&value)?;
    let message = object.get("message");
    let message_object = message.and_then(serde_json::Value::as_object);
    let content = message_object
        .and_then(|message| message.get("content").or_else(|| message.get("text")))
        .or_else(|| object.get("content"))
        .or_else(|| object.get("text"))
        .or_else(|| message.filter(|message| message.is_string()))?;
    let mut parts = Vec::new();
    collect_generic_session_content(content, 0, &mut parts);
    if parts.is_empty() {
        return None;
    }
    Some(
        serde_json::json!({
            "role": role,
            "content": parts.join("\n\n"),
        })
        .to_string(),
    )
}

/// Stream a complete JSONL file and retain only readable conversation records.
/// Raw tool results, screenshots and internal reasoning are discarded while the
/// file is read, so an explicit full-session request does not create a second
/// hundreds-of-megabytes IPC payload.
fn read_full_rendered_jsonl(path: &Path, is_rollout: bool) -> std::io::Result<Option<String>> {
    use std::io::BufRead;

    let file = fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let mut event_records = String::new();
    let mut item_records = String::new();
    let mut event_has_user = false;
    let mut item_has_user = false;
    let mut generic_records = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let record = line.trim();
        if record.is_empty() {
            continue;
        }
        if is_rollout {
            let Some(kind) = readable_codex_rollout_line_kind(record) else {
                continue;
            };
            match kind.stream() {
                ReadableCodexStream::Event => {
                    event_has_user |= kind.is_user();
                    append_jsonl_record(&mut event_records, record);
                }
                ReadableCodexStream::Item => {
                    item_has_user |= kind.is_user();
                    append_jsonl_record(&mut item_records, record);
                }
            }
        } else if let Some(sanitized) = sanitized_generic_session_record(record) {
            append_jsonl_record(&mut generic_records, &sanitized);
        }
    }

    if !is_rollout {
        return Ok((!generic_records.is_empty()).then_some(generic_records));
    }
    let selected = if event_has_user {
        event_records
    } else if item_has_user {
        item_records
    } else if !event_records.is_empty() {
        event_records
    } else {
        item_records
    };
    Ok((!selected.is_empty()).then_some(selected))
}

fn rfind_any_subslice_before(haystack: &[u8], needles: &[&[u8]], end: usize) -> Option<usize> {
    let mut cursor = end.min(haystack.len());
    while cursor > 0 {
        let candidate = haystack[..cursor].iter().rposition(|&byte| byte == b'"')?;
        if needles
            .iter()
            .any(|needle| haystack[candidate..].starts_with(needle))
        {
            return Some(candidate);
        }
        cursor = candidate;
    }
    None
}

fn read_bounded_jsonl_line_at(
    file: &mut fs::File,
    size_bytes: u64,
    marker_offset: u64,
) -> std::io::Result<Option<String>> {
    use std::io::{Read, Seek};

    let radius = CODEX_ROLLOUT_RENDER_LINE_MAX_BYTES as u64;
    let window_start = marker_offset.saturating_sub(radius);
    let window_end = marker_offset.saturating_add(radius).min(size_bytes);
    file.seek(std::io::SeekFrom::Start(window_start))?;
    let mut buffer = Vec::with_capacity((window_end - window_start) as usize);
    file.take(window_end - window_start)
        .read_to_end(&mut buffer)?;
    let marker_index = (marker_offset - window_start) as usize;
    if marker_index >= buffer.len() {
        return Ok(None);
    }

    let line_start = buffer[..marker_index]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or_else(|| (window_start == 0).then_some(0), |index| Some(index + 1));
    let Some(line_start) = line_start else {
        return Ok(None);
    };
    let line_end = buffer[marker_index..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or_else(
            || (window_end == size_bytes).then_some(buffer.len()),
            |index| Some(marker_index + index),
        );
    let Some(line_end) = line_end else {
        return Ok(None);
    };
    if line_end <= line_start || line_end - line_start > CODEX_ROLLOUT_RENDER_LINE_MAX_BYTES {
        return Ok(None);
    }
    let Ok(line) = std::str::from_utf8(&buffer[line_start..line_end]) else {
        return Ok(None);
    };
    Ok(Some(line.trim_end_matches('\r').to_string()))
}

/// Find the newest human turn even when screenshots/tool output have pushed it far
/// outside the normal rendered tail. The scan is reverse, chunked and memory-bounded;
/// candidate lines are parsed before use so marker-like text inside tool output is
/// never mistaken for a conversation turn.
fn find_latest_codex_user_line(
    path: &Path,
    size_bytes: u64,
    stream: ReadableCodexStream,
) -> std::io::Result<Option<(u64, String)>> {
    use std::io::{Read, Seek};

    const MARKERS: [&[u8]; 4] = [
        b"\"type\":\"user_message\"",
        b"\"type\": \"user_message\"",
        b"\"role\":\"user\"",
        b"\"role\": \"user\"",
    ];
    let overlap = MARKERS.iter().map(|marker| marker.len()).max().unwrap_or(1) as u64;
    let scan_start = size_bytes.saturating_sub(CODEX_ROLLOUT_CONTEXT_SCAN_MAX_BYTES);
    let mut file = fs::File::open(path)?;
    let mut chunk_end = size_bytes;

    while chunk_end > scan_start {
        let chunk_start = chunk_end
            .saturating_sub(CODEX_ROLLOUT_CONTEXT_SCAN_CHUNK_BYTES)
            .max(scan_start);
        file.seek(std::io::SeekFrom::Start(chunk_start))?;
        let mut chunk = Vec::with_capacity((chunk_end - chunk_start) as usize);
        file.by_ref()
            .take(chunk_end - chunk_start)
            .read_to_end(&mut chunk)?;
        let mut cursor = chunk.len();

        while let Some(offset) = rfind_any_subslice_before(&chunk, &MARKERS, cursor) {
            cursor = offset;
            let absolute_offset = chunk_start + offset as u64;
            if let Some(line) = read_bounded_jsonl_line_at(&mut file, size_bytes, absolute_offset)?
            {
                if let Some(kind) = readable_codex_rollout_line_kind(&line) {
                    if kind.is_user() && kind.stream() == stream {
                        return Ok(Some((absolute_offset, line)));
                    }
                }
            }
        }

        if chunk_start == scan_start {
            break;
        }
        chunk_end = chunk_start.saturating_add(overlap);
    }
    Ok(None)
}

/// Build a second, conversation-only tail for Rendered without changing the raw
/// bounded Source window. Large screenshots and tool outputs can occupy many MB
/// after the newest human turn, so scan farther back but parse only small candidate
/// lines and return a tightly bounded set of recent readable records.
fn read_codex_rendered_window(path: &Path, size_bytes: u64) -> std::io::Result<Option<String>> {
    use std::io::{Read, Seek};

    let scan_bytes = size_bytes.min(CODEX_ROLLOUT_RENDER_SCAN_MAX_BYTES);
    let mut file = fs::File::open(path)?;
    file.seek(std::io::SeekFrom::Start(size_bytes - scan_bytes))?;
    let mut buffer = Vec::with_capacity(scan_bytes as usize);
    file.take(scan_bytes).read_to_end(&mut buffer)?;
    if size_bytes > scan_bytes {
        if let Some(pos) = buffer.iter().position(|&byte| byte == b'\n') {
            buffer.drain(..=pos);
        }
    }

    let text = String::from_utf8_lossy(&buffer);
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() || line.len() > CODEX_ROLLOUT_RENDER_LINE_MAX_BYTES {
            continue;
        }
        if !is_readable_codex_rollout_line(line) {
            continue;
        }
        if selected_bytes + line.len() + 1 > CODEX_ROLLOUT_RENDER_MAX_BYTES {
            break;
        }
        selected_bytes += line.len() + 1;
        selected.push(line.to_string());
        if selected.len() >= CODEX_ROLLOUT_RENDER_MAX_LINES {
            break;
        }
    }
    // The frontend renders event_msg whenever that stream exists and only falls
    // back to response_item otherwise. Check human context in that same stream;
    // a user-looking item in the discarded fallback must not suppress recovery.
    let rendered_stream = if selected.iter().any(|line| {
        readable_codex_rollout_line_kind(line)
            .is_some_and(|kind| kind.stream() == ReadableCodexStream::Event)
    }) {
        Some(ReadableCodexStream::Event)
    } else if selected.iter().any(|line| {
        readable_codex_rollout_line_kind(line)
            .is_some_and(|kind| kind.stream() == ReadableCodexStream::Item)
    }) {
        Some(ReadableCodexStream::Item)
    } else {
        None
    };
    let has_user = selected.iter().any(|line| {
        readable_codex_rollout_line_kind(line)
            .is_some_and(|kind| kind.is_user() && Some(kind.stream()) == rendered_stream)
    });
    let recovered_user = if has_user {
        None
    } else if let Some(stream) = rendered_stream {
        find_latest_codex_user_line(path, size_bytes, stream)?
    } else {
        match find_latest_codex_user_line(path, size_bytes, ReadableCodexStream::Event)? {
            Some(context) => Some(context),
            None => find_latest_codex_user_line(path, size_bytes, ReadableCodexStream::Item)?,
        }
    };

    if let Some((_, user_line)) = &recovered_user {
        let reserved = user_line.len() + CODEX_ROLLOUT_GAP_EVENT.len() + 2;
        while selected_bytes + reserved > CODEX_ROLLOUT_RENDER_MAX_BYTES {
            let Some(removed) = selected.pop() else {
                break;
            };
            selected_bytes = selected_bytes.saturating_sub(removed.len() + 1);
        }
    }
    selected.reverse();
    // A bounded tail can contain many updates from an older request before it
    // reaches a newer human turn. Starting Rendered with those contextless
    // assistant messages reads like the app lost the question. Once a user turn
    // exists in the chosen stream, trim everything before its first occurrence;
    // the normal truncated-preview note still makes the bounded history explicit.
    if recovered_user.is_none() {
        if let Some(stream) = rendered_stream {
            if let Some(first_user_index) = selected.iter().position(|line| {
                readable_codex_rollout_line_kind(line)
                    .is_some_and(|kind| kind.is_user() && kind.stream() == stream)
            }) {
                selected.drain(..first_user_index);
            }
        }
    }
    if let Some((_, user_line)) = recovered_user {
        let mut contextual = Vec::with_capacity(selected.len() + 2);
        contextual.push(user_line);
        if !selected.is_empty() {
            contextual.push(CODEX_ROLLOUT_GAP_EVENT.to_string());
        }
        contextual.extend(selected);
        return Ok(Some(contextual.join("\n")));
    }
    Ok((!selected.is_empty()).then(|| selected.join("\n")))
}

/// Epoch milliseconds for an optional file timestamp (created / modified), or `None`
/// when the platform/filesystem doesn't report it or it predates the Unix epoch.
fn system_time_to_ms(time: Option<std::time::SystemTime>) -> Option<i64> {
    time?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|delta| delta.as_millis() as i64)
}

/// Preview returned when a session store was RECOGNIZED (Antigravity/Hermes/
/// OpenClaw database) but its transcript could not be recovered right now —
/// typically the SQLite file is locked by the owning app, or its schema drifted.
/// Falling through to the generic byte window would render raw database bytes as
/// mojibake, so the user gets a plain-language note in `text` instead (same
/// struct shape as a real preview; nothing new for the frontend to learn).
fn unreadable_session_store_preview(
    path: String,
    canonical: &Path,
    session_kind: &str,
    size_bytes: u64,
    created_ms: Option<i64>,
    modified_ms: Option<i64>,
    reveal: bool,
) -> SessionPreview {
    let display_name = canonical
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let text = format!(
        "Code Hangar couldn't read this session store right now — it may be in use by its app, or \
         stored in a layout this version doesn't understand yet. Close the app and try again. The \
         session itself is untouched on disk. File: {path}"
    );
    SessionPreview {
        path,
        display_name,
        session_kind: session_kind.to_string(),
        size_bytes,
        preview_limit_bytes: size_bytes,
        truncated: false,
        source_truncated: false,
        text,
        rendered_text: None,
        redacted_count: 0,
        revealed: reveal,
        created_ms,
        modified_ms,
    }
}

/// Initial bounded session preview retained for internal callers and tests.
pub fn session_preview(path: String, reveal: bool) -> Result<SessionPreview, String> {
    session_preview_window(path, reveal, None, false)
}

fn resolve_allowed_session_file(path: &str) -> Result<(PathBuf, Option<String>), String> {
    let fragment = path
        .rsplit_once('#')
        .map(|(_, fragment)| fragment.to_string());
    let requested = PathBuf::from(path);
    // A session "path" can carry a #fragment for transcripts split out of one
    // store; fall back to the underlying file when the literal path is missing.
    let file_path = if requested.is_file() {
        requested
    } else if let Some((base, _fragment)) = path.rsplit_once('#') {
        let base_path = PathBuf::from(base);
        if base_path.is_file() {
            base_path
        } else {
            return Err("This session file is no longer available on disk.".to_string());
        }
    } else {
        return Err("This session file is no longer available on disk.".to_string());
    };

    let canonical = file_path
        .canonicalize()
        .map_err(|_| "This session file could not be opened.".to_string())?;
    let allowed = hangar_discovery::session_store_roots()
        .into_iter()
        .any(|root| {
            root.canonicalize()
                .map(|root_canon| canonical.starts_with(&root_canon))
                .unwrap_or(false)
        });
    if !allowed {
        return Err(
            "Code Hangar only previews files inside known local session stores.".to_string(),
        );
    }

    // Reading a dehydrated placeholder would silently hydrate it and violate the
    // local-only read contract shared by preview and change reconstruction.
    if hangar_fs::inspect_path_identity(&canonical)
        .reparse_kind
        .as_deref()
        == Some("cloud_placeholder")
    {
        return Err(
            "This session file is stored online-only (a cloud placeholder). Code Hangar will not download it to preview - open it in its owning app to materialize it locally first."
                .to_string(),
        );
    }

    Ok((canonical, fragment))
}

pub fn reveal_session_external(path: String) -> Result<(), String> {
    let (canonical, _fragment) = resolve_allowed_session_file(&path)?;
    reveal_path_external(&canonical.to_string_lossy())
}

/// Reconstruct only the file edits explicitly recorded in a known local session
/// store. The result is transient, secret-redacted, and never executes session
/// content or reads a project file.
pub fn session_change_set(path: String) -> Result<SessionChangeSet, String> {
    let (canonical, fragment) = resolve_allowed_session_file(&path)?;
    if let Some(composer_id) = fragment
        .as_deref()
        .and_then(|value| value.strip_prefix("cursor-ide-chat="))
    {
        let records = hangar_discovery::cursor_ide_chat_changes(&canonical, composer_id)?;
        return Ok(session_changes::build_cursor_change_set(path, records));
    }
    if fragment.is_some() || canonical.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        return Ok(session_changes::unsupported_change_set(path));
    }
    session_changes::build_session_change_set(&canonical, path)
}

/// Reconstruct a known local session in the context of one registered project,
/// compare the recorded edits with current authorized files, and retain the
/// normalized redacted evidence in the encrypted review ledger.
pub fn project_session_change_set(
    state: &AppState,
    project_id: i64,
    path: String,
) -> Result<SessionChangeSet, String> {
    project_review::project_session_change_set(state, project_id, path)
}

/// Read the current local Git index/working-tree evidence without invoking a
/// shell or any remote Git operation.
pub fn project_git_change_set(
    state: &AppState,
    project_id: i64,
) -> Result<SessionChangeSet, String> {
    project_review::project_git_change_set(state, project_id)
}

pub fn project_review_checkpoint(
    state: &AppState,
    project_id: i64,
) -> Result<Option<ProjectReviewCheckpoint>, String> {
    project_review::project_review_checkpoint(state, project_id)
}

pub fn project_review_checkpoints(
    state: &AppState,
) -> Result<Vec<ProjectReviewCheckpoint>, String> {
    project_review::project_review_checkpoints(state)
}

pub fn mark_project_reviewed(
    state: &AppState,
    project_id: i64,
    session_cutoff_ms: i64,
) -> Result<ProjectReviewCheckpoint, String> {
    project_review::mark_project_reviewed(state, project_id, session_cutoff_ms)
}

pub fn project_review_ledger(
    state: &AppState,
    project_id: i64,
    limit: Option<usize>,
) -> Result<Vec<ReviewLedgerEntry>, String> {
    project_review::project_review_ledger(state, project_id, limit.unwrap_or(100))
}

pub fn project_recap(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
) -> Result<SessionChangeSet, String> {
    project_review::project_recap(state, project_id, session_paths)
}

pub fn project_review_receipt_export(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    scope: String,
    path: String,
) -> Result<ExportResult, String> {
    project_review::project_review_receipt_export(state, project_id, session_paths, scope, path)
}

/// Read-only, secret-redacted cumulative window of a local session/transcript.
/// `max_bytes=None` keeps the small initial preview. `load_full=true` is reserved
/// for the explicit UI action that opens the complete session. The allow-list and
/// cloud-placeholder gates remain identical to the initial preview, so this can
/// never become an arbitrary file reader. Results are transient and are never
/// written to SQLite, FTS, persistent caches or logs.
pub fn session_preview_window(
    path: String,
    reveal: bool,
    max_bytes: Option<u64>,
    load_full: bool,
) -> Result<SessionPreview, String> {
    let (canonical, fragment) = resolve_allowed_session_file(&path)?;

    let metadata = fs::metadata(&canonical).map_err(to_message)?;
    let size_bytes = metadata.len();
    let created_ms = system_time_to_ms(metadata.created().ok());
    let modified_ms = system_time_to_ms(metadata.modified().ok());
    let structured_preview_limit_bytes = requested_session_preview_limit(
        size_bytes,
        hangar_discovery::HERMES_TRANSCRIPT_MAX_BYTES as u64,
        max_bytes,
        load_full,
    );
    let structured_preview_limit = preview_limit_as_usize(structured_preview_limit_bytes);

    // Antigravity moves the live conversation into `conversations/<uuid>.db`, a
    // plain SQLite database whose `steps.step_payload` blobs are protobuf. Reading
    // it as raw bytes would render binary noise, so recover the chat text by a
    // schema-less protobuf scan. On failure (DB locked by the app, schema drift)
    // this returns a plain-language note rather than falling through to the
    // generic byte preview, which would render raw SQLite bytes as mojibake.
    if hangar_discovery::is_antigravity_conversation_db(&canonical) {
        if let Some((transcript, transcript_truncated)) =
            hangar_discovery::antigravity_conversation_transcript_window(
                &canonical,
                structured_preview_limit,
                load_full,
            )
        {
            let (redacted_text, redacted_count) = redact_secrets(&transcript);
            let text = if reveal { transcript } else { redacted_text };
            let display_name = canonical
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            return Ok(SessionPreview {
                path,
                display_name,
                session_kind: "Antigravity/Gemini".to_string(),
                size_bytes,
                preview_limit_bytes: structured_preview_limit_bytes,
                // `truncated` reflects whether older messages were dropped to fit
                // the cap, not the binary/text size gap, so the UI signals
                // "newest messages only" only when that is actually true.
                truncated: transcript_truncated,
                source_truncated: transcript_truncated,
                text,
                rendered_text: None,
                redacted_count,
                revealed: reveal,
                created_ms,
                modified_ms,
            });
        }
        return Ok(unreadable_session_store_preview(
            path,
            &canonical,
            "Antigravity/Gemini",
            size_bytes,
            created_ms,
            modified_ms,
            reveal,
        ));
    }

    if hangar_discovery::is_hermes_state_db(&canonical) {
        if let Some(session_id) = fragment
            .as_deref()
            .and_then(|value| value.strip_prefix("hermes-session="))
        {
            if let Some((transcript, transcript_truncated)) =
                hangar_discovery::hermes_session_transcript_window(
                    &canonical,
                    session_id,
                    structured_preview_limit,
                    load_full,
                )
            {
                let (redacted_text, redacted_count) = redact_secrets(&transcript);
                return Ok(SessionPreview {
                    path,
                    display_name: format!(
                        "Hermes · {}",
                        session_id.chars().take(10).collect::<String>()
                    ),
                    session_kind: "Hermes/NemoClaw".to_string(),
                    size_bytes,
                    preview_limit_bytes: structured_preview_limit_bytes,
                    truncated: transcript_truncated,
                    source_truncated: transcript_truncated,
                    text: if reveal { transcript } else { redacted_text },
                    rendered_text: None,
                    redacted_count,
                    revealed: reveal,
                    created_ms,
                    modified_ms,
                });
            }
            // The fragment named a specific conversation but the state.db could
            // not be read (locked/schema drift) — never render raw SQLite bytes.
            return Ok(unreadable_session_store_preview(
                path,
                &canonical,
                "Hermes/NemoClaw",
                size_bytes,
                created_ms,
                modified_ms,
                reveal,
            ));
        }
    }

    if let Some(fragment) = fragment.as_deref() {
        if fragment.starts_with("openclaw-session=") || fragment.starts_with("openclaw-replay=") {
            if let Some((transcript, transcript_truncated)) =
                hangar_discovery::openclaw_session_transcript_window(
                    &canonical,
                    fragment,
                    structured_preview_limit,
                    load_full,
                )
            {
                let (redacted_text, redacted_count) = redact_secrets(&transcript);
                return Ok(SessionPreview {
                    path,
                    display_name: "OpenClaw conversation".to_string(),
                    session_kind: "OpenClaw".to_string(),
                    size_bytes,
                    preview_limit_bytes: structured_preview_limit_bytes,
                    truncated: transcript_truncated,
                    source_truncated: transcript_truncated,
                    text: if reveal { transcript } else { redacted_text },
                    rendered_text: None,
                    redacted_count,
                    revealed: reveal,
                    created_ms,
                    modified_ms,
                });
            }
            // Same rule as the Hermes branch above: a matched conversation whose
            // store cannot be read right now gets a note, not raw database bytes.
            return Ok(unreadable_session_store_preview(
                path,
                &canonical,
                "OpenClaw",
                size_bytes,
                created_ms,
                modified_ms,
                reveal,
            ));
        }
    }

    // Cursor in-IDE (Composer/agent) chats live in the shared global `state.vscdb`
    // and are listed with a `cursor-ide-chat=<composerId>` fragment. Render just that
    // composer's ordered messages into a clean role-labelled transcript (loading only
    // its own bubble rows, never the ~20k-bubble content table). Same discipline as
    // the Hermes/OpenClaw branches: secret-redact, honor `reveal`, and fall back to a
    // friendly note — never raw SQLite bytes — when the composer/record is missing or
    // the store can't be read.
    if let Some(composer_id) = fragment
        .as_deref()
        .and_then(|value| value.strip_prefix("cursor-ide-chat="))
    {
        let display_name = hangar_discovery::cursor_ide_chat_title(&canonical, composer_id)
            .unwrap_or_else(|| {
                let short = composer_id.split('-').next().unwrap_or(composer_id);
                format!("Cursor chat {short}")
            });
        match hangar_discovery::cursor_ide_chat_transcript_window(
            &canonical,
            composer_id,
            structured_preview_limit,
            load_full,
        ) {
            hangar_discovery::CursorChatTranscript::Rendered {
                text: transcript,
                truncated: transcript_truncated,
            } => {
                let (redacted_text, redacted_count) = redact_secrets(&transcript);
                return Ok(SessionPreview {
                    path,
                    display_name,
                    session_kind: "Cursor".to_string(),
                    size_bytes,
                    preview_limit_bytes: structured_preview_limit_bytes,
                    truncated: transcript_truncated,
                    source_truncated: transcript_truncated,
                    text: if reveal { transcript } else { redacted_text },
                    rendered_text: None,
                    redacted_count,
                    revealed: reveal,
                    created_ms,
                    modified_ms,
                });
            }
            // The composer record read fine but has no messages (an empty draft — about
            // a third of the listed Cursor conversations on a real machine). Show a calm
            // note, NOT the alarming "couldn't read this store" one below.
            hangar_discovery::CursorChatTranscript::Empty => {
                return Ok(SessionPreview {
                    path,
                    display_name,
                    session_kind: "Cursor".to_string(),
                    size_bytes,
                    preview_limit_bytes: structured_preview_limit_bytes,
                    truncated: false,
                    source_truncated: false,
                    text: "This Cursor conversation has no messages yet.".to_string(),
                    rendered_text: None,
                    redacted_count: 0,
                    revealed: reveal,
                    created_ms,
                    modified_ms,
                });
            }
            // The fragment named a composer but its record could not be read (locked /
            // pruned / schema drift) — a friendly note, never raw SQLite bytes.
            hangar_discovery::CursorChatTranscript::Unavailable => {
                return Ok(unreadable_session_store_preview(
                    path,
                    &canonical,
                    "Cursor",
                    size_bytes,
                    created_ms,
                    modified_ms,
                    reveal,
                ));
            }
        }
    }

    // JSONL session transcripts put the newest conversation turns at the END of a
    // potentially multi-MB file (a Codex rollout's readable text lives under
    // `payload` throughout; a long-running Claude transcript just keeps
    // appending). A head-read therefore surfaces only the oldest turns and never
    // reaches the latest conversation. For an oversized `.jsonl` we read the TAIL
    // instead so the newest turns are what the user sees, mirroring the
    // Antigravity `.db` path that already keeps the newest content. The first
    // (likely partial) line of the tail window is dropped so we never render a
    // half-decoded JSON line. Rollouts get their larger dedicated cap.
    let is_rollout = is_codex_rollout_jsonl(&canonical);
    let is_jsonl = is_jsonl_session_file(&canonical);
    let initial_limit = if is_rollout {
        CODEX_ROLLOUT_TAIL_MAX_BYTES
    } else {
        SESSION_PREVIEW_MAX_BYTES
    };
    let preview_limit_bytes =
        requested_session_preview_limit(size_bytes, initial_limit, max_bytes, load_full);
    // A full JSONL request streams the complete readable conversation below. Keep
    // Source on the already-requested bounded raw window so a 400+ MB transcript
    // never crosses IPC as one unfiltered string.
    let source_limit_bytes = if load_full && is_jsonl {
        requested_session_preview_limit(size_bytes, initial_limit, max_bytes, false)
    } else {
        preview_limit_bytes
    };
    let (buffer, source_truncated) = read_session_preview_window_with_limit(
        &canonical,
        is_jsonl,
        size_bytes,
        source_limit_bytes,
    )
    .map_err(to_message)?;
    let truncated = if load_full && is_jsonl {
        false
    } else {
        source_truncated
    };

    // Some session stores are binary (Antigravity `.pb` conversations, stray
    // LevelDB/SQLite blobs). Rendering those through the lossy-UTF-8 path below
    // yields mojibake, so sniff first and return a plain-language note instead.
    if looks_binary_session_head(&buffer) {
        let display_name = canonical
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let text = format!(
            "This session is stored in a binary format that Code Hangar can't render as text yet, \
             so there is no preview. The conversation itself is intact on disk — open it in its \
             owning app to read it. File: {path}"
        );
        return Ok(SessionPreview {
            path,
            display_name,
            session_kind: String::new(),
            size_bytes,
            preview_limit_bytes,
            truncated: false,
            source_truncated,
            text,
            rendered_text: None,
            redacted_count: 0,
            revealed: reveal,
            created_ms,
            modified_ms,
        });
    }

    let rendered_raw = if load_full && is_jsonl {
        read_full_rendered_jsonl(&canonical, is_rollout)
            .ok()
            .flatten()
    } else if is_rollout {
        if max_bytes.is_some() || load_full {
            expanded_codex_rendered_window(&buffer).or_else(|| {
                read_codex_rendered_window(&canonical, size_bytes)
                    .ok()
                    .flatten()
            })
        } else {
            read_codex_rendered_window(&canonical, size_bytes)
                .ok()
                .flatten()
        }
    } else {
        None
    };
    let (rendered_text, rendered_redacted_count) = match rendered_raw {
        Some(raw) => {
            let (redacted, count) = redact_secrets(&raw);
            (Some(if reveal { raw } else { redacted }), count)
        }
        None => (None, 0),
    };

    let raw = String::from_utf8_lossy(&buffer);
    let (redacted_text, raw_redacted_count) = redact_secrets(&raw);
    // Reveal returns the raw text (explicit local user action, transient, never
    // persisted); the default masks secrets. redacted_count reports how many
    // tokens are maskable either way, so the UI can offer "reveal N hidden".
    let text = if reveal {
        raw.into_owned()
    } else {
        redacted_text
    };

    let display_name = canonical
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    Ok(SessionPreview {
        path,
        display_name,
        session_kind: String::new(),
        size_bytes,
        preview_limit_bytes,
        truncated,
        source_truncated,
        text,
        rendered_text,
        redacted_count: raw_redacted_count.max(rendered_redacted_count),
        revealed: reveal,
        created_ms,
        modified_ms,
    })
}

/// Mask high-confidence secrets in free text while preserving paths, hashes and
/// normal prose. Returns the redacted text and how many tokens were masked.
fn redact_secrets(input: &str) -> (String, u32) {
    let mut out = String::with_capacity(input.len());
    let mut count = 0u32;
    let bytes = input.as_bytes();
    let mut start = 0usize;
    let mut prev_token_key = false;
    let mut i = 0usize;
    while i <= input.len() {
        let at_ws = i == input.len() || bytes[i].is_ascii_whitespace();
        if at_ws {
            if start < i {
                let token = &input[start..i];
                let (rendered, masked) = redact_one_token(token, prev_token_key);
                out.push_str(&rendered);
                if masked {
                    count += 1;
                }
                prev_token_key = is_secret_key_token(token);
            }
            if i < input.len() {
                out.push(bytes[i] as char);
            }
            start = i + 1;
        }
        i += 1;
    }
    (out, count)
}

fn redact_one_token(token: &str, prev_token_key: bool) -> (String, bool) {
    // A bare secret token (API key, JWT, PEM header, ...).
    if looks_like_secret(token) {
        return (mask_token(token), true);
    }
    // `key=value` or `key:value` collapsed into one token.
    if let Some((key, sep, value)) = split_key_value(token) {
        if !value.is_empty()
            && (looks_like_secret(value) || (is_secret_key_token(key) && credential_like(value)))
        {
            return (format!("{key}{sep}{}", mask_token(value)), true);
        }
    }
    // A value that follows a separate sensitive key token (e.g. `Bearer xxxx`).
    if prev_token_key && credential_like(token) {
        return (mask_token(token), true);
    }
    (token.to_string(), false)
}

fn split_key_value(token: &str) -> Option<(&str, char, &str)> {
    if let Some(idx) = token.find('=') {
        return Some((&token[..idx], '=', &token[idx + 1..]));
    }
    if let Some(idx) = token.find(':') {
        let after = &token[idx + 1..];
        // Skip URL schemes (https://) and Windows drive letters (C:\, C:/).
        if after.starts_with('/') || after.starts_with('\\') {
            return None;
        }
        return Some((&token[..idx], ':', after));
    }
    None
}

fn redaction_trim(token: &str) -> &str {
    token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
    })
}

fn mask_token(token: &str) -> String {
    let trimmed = redaction_trim(token);
    if trimmed.is_empty() || trimmed == token {
        "«redacted»".to_string()
    } else {
        token.replacen(trimmed, "«redacted»", 1)
    }
}

fn looks_like_secret(token: &str) -> bool {
    let s = redaction_trim(token);
    // PEM markers must be tested BEFORE the length guard: the trimmed token "-----BEGIN" is only
    // 10 chars, so the `len() < 12` gate below would otherwise skip it and leave the highest-
    // signal secret of all — a private key block — rendered in full.
    if s.starts_with("-----BEGIN") {
        return true;
    }
    if s.len() < 12 {
        return false;
    }
    const PREFIXES: &[&str] = &[
        "sk-ant",
        "sk-",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxs-",
        "xoxr-",
        "xapp-",
        "glpat-",
        "gsk_",
        "aiza",
        "ya29.",
    ];
    let lower = s.to_ascii_lowercase();
    if PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
        && s.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 12
    {
        return true;
    }
    if (s.starts_with("AKIA") || s.starts_with("ASIA"))
        && s.len() >= 20
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return true;
    }
    is_jwt(s)
}

fn is_jwt(s: &str) -> bool {
    if !s.starts_with("eyJ") {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            p.len() >= 8
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

fn is_secret_key_token(token: &str) -> bool {
    let core: String = token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    matches!(
        core.to_ascii_lowercase().as_str(),
        "password"
            | "passwd"
            | "secret"
            | "token"
            | "apikey"
            | "api_key"
            | "accesskey"
            | "access_key"
            | "client_secret"
            | "authorization"
            | "bearer"
            | "private_key"
            | "secret_key"
            | "session_token"
            | "refresh_token"
            | "access_token"
            | "api_token"
            | "auth_token"
    )
}

fn credential_like(token: &str) -> bool {
    let value = redaction_trim(token);
    if value.len() < 12 || value.contains('/') || value.contains('\\') || value.contains("..") {
        return false;
    }
    let has_digit = value.chars().any(|c| c.is_ascii_digit());
    let has_alpha = value.chars().any(|c| c.is_ascii_alphabetic());
    has_digit
        && has_alpha
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '=' | '~'))
}

#[cfg(test)]
mod session_redaction_tests {
    use super::redact_secrets;

    #[test]
    fn masks_known_token_prefixes() {
        let (out, n) = redact_secrets("key=sk-ABCDEF1234567890ABCDEFGH done");
        assert!(out.contains("«redacted»"), "{out}");
        assert!(!out.contains("sk-ABCDEF1234567890ABCDEFGH"));
        assert_eq!(n, 1);
    }

    #[test]
    fn masks_authorization_value() {
        let (out, n) = redact_secrets("Authorization: Bearer abcdef123456ghijkl");
        assert!(out.contains("«redacted»"), "{out}");
        assert_eq!(n, 1);
    }

    #[test]
    fn keeps_paths_hashes_and_prose() {
        let input = "Edited C:/AI/Codex/CodeHangar/src/main.rs at commit 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b today";
        let (out, n) = redact_secrets(input);
        assert_eq!(out, input);
        assert_eq!(n, 0);
    }

    #[test]
    fn preserves_newlines_and_structure() {
        let input = "line one\nline two\n";
        let (out, _n) = redact_secrets(input);
        assert_eq!(out, input);
    }

    #[test]
    fn masks_pem_private_key_marker() {
        // The PEM "-----BEGIN" token is only 10 chars, which previously slipped past the
        // length>=12 guard and rendered the private key in full.
        let (out, n) = redact_secrets("-----BEGIN RSA PRIVATE KEY-----");
        assert!(out.contains("«redacted»"), "{out}");
        assert!(!out.contains("-----BEGIN"), "{out}");
        assert!(n >= 1);
    }
}

pub fn search_documents(
    state: &AppState,
    request: DocumentSearchRequest,
) -> Result<DocumentSearchResult, String> {
    let _performance =
        PerformanceScope::enter(PerformanceMode::parse(request.performance_mode.as_deref()));
    state
        .db()?
        .search_documents_filtered(DocumentSearchOptions {
            query: &request.query,
            project_id: request.project_id,
            indexed_kind: request.indexed_kind.as_deref(),
            path_filter: request.path_filter.as_deref(),
            name_filter: request.name_filter.as_deref(),
            include_fixture_projects: request.include_fixture_projects,
            limit: request.limit.unwrap_or(20),
        })
        .map_err(to_message)
}

pub fn resolve_local_link(
    state: &AppState,
    project_id: i64,
    from_node_id: i64,
    target: String,
) -> Result<Option<i64>, String> {
    state
        .db()?
        .resolve_local_link(project_id, from_node_id, &target)
        .map_err(to_message)
}

pub fn node_relationships(state: &AppState, node_id: i64) -> Result<NodeRelationships, String> {
    state.db()?.node_relationships(node_id).map_err(to_message)
}

pub fn project_graph_map(
    state: &AppState,
    project_id: i64,
    limit: Option<usize>,
) -> Result<GraphMap, String> {
    state
        .db()?
        .project_graph_map(project_id, limit.unwrap_or(300))
        .map_err(to_message)
}

pub fn graph_orphans(state: &AppState, limit: Option<usize>) -> Result<OrphanCandidates, String> {
    state
        .db()?
        .graph_orphans(limit.unwrap_or(50))
        .map_err(to_message)
}

pub fn orphan_asset_candidates(
    state: &AppState,
    request: OrphanAssetRequest,
) -> Result<OrphanCandidates, String> {
    let _performance =
        PerformanceScope::enter(PerformanceMode::parse(request.performance_mode.as_deref()));
    state
        .db()?
        .orphan_asset_candidates(OrphanAssetSearchOptions {
            min_size_bytes: request.min_size_bytes,
            project_id: request.project_id,
            asset_kind: request.asset_kind.as_deref(),
            min_confidence: request.min_confidence.as_deref(),
            include_partial: request.include_partial.unwrap_or(false),
            include_fixture_projects: request.include_fixture_projects,
            limit: request.limit.unwrap_or(50),
        })
        .map_err(to_message)
}

pub fn node_orphan_status(state: &AppState, node_id: i64) -> Result<OrphanStatus, String> {
    state.db()?.node_orphan_status(node_id).map_err(to_message)
}

pub fn lost_project_candidates(
    state: &AppState,
    request: LostProjectRequest,
) -> Result<LostProjectCandidates, String> {
    let _performance =
        PerformanceScope::enter(PerformanceMode::parse(request.performance_mode.as_deref()));
    state
        .db()?
        .lost_project_candidates(LostProjectSearchOptions {
            min_size_bytes: request.min_size_bytes,
            project_id: request.project_id,
            stale_preset: request.stale_preset.as_deref(),
            signals: &request.signals,
            keyword: request.keyword.as_deref(),
            include_partial: request.include_partial,
            include_fixture_projects: request.include_fixture_projects,
            limit: request.limit,
        })
        .map_err(to_message)
}

pub fn duplicate_candidates(
    state: &AppState,
    request: DuplicateSearchRequest,
) -> Result<DuplicateCandidates, String> {
    let _performance =
        PerformanceScope::enter(PerformanceMode::parse(request.performance_mode.as_deref()));
    state
        .db()?
        .duplicate_candidates_filtered(
            request.min_size_bytes,
            request.project_id,
            request.file_kind.as_deref(),
            request.current_file_node_id,
            request.include_fixture_projects,
            request.limit.unwrap_or(25),
        )
        .map_err(to_message)
}

pub fn confirm_duplicate_group(
    state: &AppState,
    node_id: i64,
) -> Result<DuplicateConfirmation, String> {
    state
        .db()?
        .confirm_duplicate_group(node_id)
        .map_err(to_message)
}

/// Start an on-demand full-hash duplicate confirmation as a background job, returning its id. The
/// full-hash verification streams every byte of each candidate, so it runs off the UI thread with
/// live progress + cancel (poll [`confirm_duplicate_group_status`], stop with
/// [`confirm_duplicate_group_cancel`]). It is read-only — it only reads bytes to hash them, and
/// only ever runs because the user explicitly asked to confirm a group (never automatically).
pub fn confirm_duplicate_group_start(state: &AppState, node_id: i64) -> Result<String, String> {
    let (job_id, cancel) = state.dup_jobs.create_running(node_id);
    let db = state.db()?;
    let jobs = state.dup_jobs.clone();
    let thread_job_id = job_id.clone();

    thread::spawn(move || {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            jobs.cancel(&thread_job_id);
            return;
        }
        // The progress closure owns its own clones so the terminal updates below can still use
        // `jobs` / `thread_job_id` without a borrow tangle.
        let progress_jobs = jobs.clone();
        let progress_job_id = thread_job_id.clone();
        let mut progress = move |p: hangar_core::DuplicateConfirmProgress| {
            progress_jobs.update_progress(&progress_job_id, p);
        };
        match db.confirm_duplicate_group_interruptible(node_id, &cancel, &mut progress) {
            Ok(Some(confirmation)) => {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    jobs.cancel(&thread_job_id);
                } else {
                    jobs.complete(&thread_job_id, confirmation);
                }
            }
            Ok(None) => jobs.cancel(&thread_job_id),
            Err(error) => {
                let message = to_message(error);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    jobs.cancel(&thread_job_id);
                } else {
                    jobs.fail(&thread_job_id, message);
                }
            }
        }
    });

    Ok(job_id)
}

/// Poll the status (state + progress + result) of a duplicate-confirmation job.
pub fn confirm_duplicate_group_status(
    state: &AppState,
    job_id: String,
) -> Result<hangar_core::DuplicateConfirmStatus, String> {
    state
        .dup_jobs
        .status(&job_id)
        .ok_or_else(|| format!("Unknown duplicate confirmation job: {job_id}"))
}

/// Request cancellation of a duplicate-confirmation job; it stops at the next file boundary.
pub fn confirm_duplicate_group_cancel(state: &AppState, job_id: String) -> Result<(), String> {
    state.dup_jobs.request_cancel(&job_id);
    Ok(())
}

pub fn project_discovery_report(
    state: &AppState,
    limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    // Push the persisted WSL opt-in into the runtime gate first: on a fresh
    // process an opted-in user's first discovery call would otherwise silently
    // skip WSL (the gate defaults off until some projects_list happens to run).
    sync_wsl_scan_flag(state);
    let registered_roots = registered_roots_for_state(state)?;

    Ok(hangar_discovery::discover_known_projects(
        &registered_roots,
        DiscoveryOptions {
            limit: limit.unwrap_or(100).min(500),
            // Loose (project-less) sessions default ON now, so "sessions soltas"
            // show out of the box; the caller/UI can still pass false to hide them.
            include_loose_sessions: include_loose_sessions.unwrap_or(true),
            include_agents: include_agents.unwrap_or(false),
            include_technical_candidates: include_technical_candidates.unwrap_or(false),
        },
    ))
}

pub fn project_discovery_deep_scan(
    state: &AppState,
    root_path: String,
    limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    // Same as `project_discovery_report`: sync the persisted WSL opt-in before
    // scanning, so a Deep Scan on a fresh process honors it.
    sync_wsl_scan_flag(state);
    let root = PathBuf::from(root_path);
    if !root.is_dir() {
        return Err("Choose an existing folder or drive for Deep Scan.".to_string());
    }
    let registered_roots = registered_roots_for_state(state)?;
    Ok(hangar_discovery::discover_projects_in_root(
        &root,
        &registered_roots,
        DiscoveryOptions {
            limit: limit.unwrap_or(250).min(1_000),
            // Default ON to match project_discovery_report (loose sessions visible
            // by default); an explicit false from the UI still hides them.
            include_loose_sessions: include_loose_sessions.unwrap_or(true),
            include_agents: include_agents.unwrap_or(false),
            include_technical_candidates: include_technical_candidates.unwrap_or(false),
        },
    ))
}

fn registered_roots_for_state(state: &AppState) -> Result<Vec<RegisteredRoot>, String> {
    let db = state.db()?;
    let projects = db.projects_list().map_err(to_message)?;
    let roots = db.roots_list().map_err(to_message)?;
    Ok(roots
        .into_iter()
        .map(|root| {
            let project_id = projects
                .iter()
                .find(|project| same_display_path(&project.path, &root.path))
                .map(|project| project.id);
            RegisteredRoot {
                project_id,
                path: PathBuf::from(root.path),
            }
        })
        .collect::<Vec<_>>())
}

pub fn project_recoverable_summary(
    state: &AppState,
    project_id: i64,
) -> Result<RecoverableSummary, String> {
    state
        .db()?
        .project_recoverable_summary(project_id)
        .map_err(to_message)
}

pub fn node_recoverable_summary(
    state: &AppState,
    node_id: i64,
) -> Result<RecoverableSummary, String> {
    state
        .db()?
        .node_recoverable_summary(node_id)
        .map_err(to_message)
}

pub fn operation_plan_build(
    state: &AppState,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<OperationPlan, String> {
    let _performance = PerformanceScope::enter(PerformanceMode::parse(performance_mode.as_deref()));
    state
        .db()?
        .operation_plan_build(target_node_id, &action_label)
        .map_err(to_message)
}

pub fn operation_plan_start(
    state: &AppState,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<String, String> {
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    let (job_id, cancel) = state
        .plan_jobs
        .create_running(target_node_id, action_label.clone());
    let db = state.db()?;
    let jobs = state.plan_jobs.clone();
    let thread_job_id = job_id.clone();

    thread::spawn(move || {
        let _performance = PerformanceScope::enter(mode);
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            jobs.cancel(&thread_job_id);
            return;
        }
        jobs.update_message(
            &thread_job_id,
            if mode.is_boost() {
                format!("Calculating preview plan in {} mode.", mode.label())
            } else {
                "Calculating preview plan.".to_string()
            },
        );
        match db.operation_plan_build_interruptible(
            target_node_id,
            &action_label,
            std::sync::Arc::clone(&cancel),
        ) {
            Ok(plan) if cancel.load(std::sync::atomic::Ordering::Relaxed) => {
                jobs.cancel(&thread_job_id);
                drop(plan);
            }
            Ok(plan) => {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    jobs.cancel(&thread_job_id);
                    return;
                }
                jobs.update_message(&thread_job_id, "Building risk report from preview plan.");
                let report = match db.risk_report_build(&plan).map_err(to_message) {
                    Ok(report) => report,
                    Err(message) => {
                        jobs.fail(&thread_job_id, message);
                        return;
                    }
                };
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    jobs.cancel(&thread_job_id);
                } else {
                    jobs.complete(&thread_job_id, plan, report);
                }
            }
            Err(error) => {
                let message = to_message(error);
                if cancel.load(std::sync::atomic::Ordering::Relaxed)
                    || message.eq_ignore_ascii_case("cancelled")
                {
                    jobs.cancel(&thread_job_id);
                } else {
                    jobs.fail(&thread_job_id, message);
                }
            }
        }
    });

    Ok(job_id)
}

pub fn operation_plan_status(
    state: &AppState,
    job_id: String,
) -> Result<PlanPreviewStatus, String> {
    state
        .plan_jobs
        .status(&job_id)
        .ok_or_else(|| format!("Unknown preview plan job: {job_id}"))
}

pub fn operation_plan_cancel(state: &AppState, job_id: String) -> Result<(), String> {
    state.plan_jobs.request_cancel(&job_id);
    Ok(())
}

pub fn risk_report_build(
    state: &AppState,
    plan: OperationPlan,
    performance_mode: Option<String>,
) -> Result<RiskReport, String> {
    let _performance = PerformanceScope::enter(PerformanceMode::parse(performance_mode.as_deref()));
    state.db()?.risk_report_build(&plan).map_err(to_message)
}

pub fn risk_report_build_for_target(
    state: &AppState,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<RiskReport, String> {
    let _performance = PerformanceScope::enter(PerformanceMode::parse(performance_mode.as_deref()));
    state
        .db()?
        .risk_report_build_for_target(target_node_id, &action_label)
        .map_err(to_message)
}

pub fn risk_report_export(report: RiskReport, path: String) -> Result<ExportResult, String> {
    hangar_plan::export_risk_report(&report, path).map_err(to_message)
}

pub fn diagnostics_export(state: &AppState, path: String) -> Result<ExportResult, String> {
    if path.trim().is_empty() {
        return Err("Choose a destination for the diagnostic bundle.".to_string());
    }
    let startup = startup_status(state);
    let security = security_status()?;
    let dashboard = dashboard_summary_filtered(state, false)?;
    let adapters = adapters_list(state)?;
    let resources = system_resource_profile();
    let checkpoint_count = project_review::project_review_checkpoints(state)?.len();
    let edition = if cfg!(feature = "agent_automation") {
        "Connector"
    } else {
        "Local"
    };
    let payload = diagnostics_payload(
        &startup,
        &security,
        &dashboard,
        &adapters,
        &resources,
        checkpoint_count,
        edition,
    );
    let bytes = serde_json::to_vec_pretty(&payload).map_err(to_message)?;
    fs::write(&path, &bytes).map_err(to_message)?;
    Ok(ExportResult {
        path,
        bytes_written: bytes.len() as u64,
    })
}

fn diagnostics_payload(
    startup: &StartupStatus,
    security: &SecurityStatus,
    dashboard: &DashboardSummary,
    adapters: &[AdapterSummary],
    resources: &SystemResourceProfile,
    checkpoint_count: usize,
    edition: &str,
) -> serde_json::Value {
    let adapters = adapters
        .iter()
        .map(|adapter| {
            serde_json::json!({
                "name": adapter.name,
                "version": adapter.version,
                "type": adapter.adapter_type,
                "enabled": adapter.enabled,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schemaVersion": "code-hangar/diagnostics/v1",
        "generatedAt": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "app": {
            "name": "Code Hangar",
            "version": env!("CARGO_PKG_VERSION"),
            "edition": edition,
        },
        "startup": {
            "state": startup.state,
            "elapsedMs": startup.elapsed_ms,
            "databaseOpenMs": startup.db_open_ms,
        },
        "security": {
            "outboundNetwork": security.outbound_network,
            "mutationExecutor": security.mutation_executor,
            "agentIpc": security.agent_ipc,
            "activeFeatures": security.active_features,
        },
        "inventory": {
            "projects": dashboard.total_projects,
            "items": dashboard.total_items,
            "contextFiles": dashboard.context_files,
            "indexedDocuments": dashboard.indexed_documents,
            "nonIndexedItems": dashboard.non_indexed_items,
            "partialItems": dashboard.partial_items,
            "gitProjects": dashboard.git_projects,
            "sensitiveLookingFiles": dashboard.sensitive_files,
            "protectedEntries": dashboard.protected_files,
            "scanRoots": dashboard.scan_roots,
            "inventoryState": dashboard.stale_or_dirty,
            "adaptersNeedingReview": dashboard.adapters_needing_review,
        },
        "review": {
            "savedProjectCheckpoints": checkpoint_count,
        },
        "resources": {
            "logicalCpuThreads": resources.logical_cpu_count,
            "totalMemoryBytes": resources.total_memory_bytes,
            "availableMemoryBytes": resources.available_memory_bytes,
        },
        "adapters": adapters,
        "privacy": {
            "redacted": true,
            "omitted": [
                "project and file names",
                "all local paths",
                "session and prompt content",
                "diffs and source code",
                "logs and free-form status messages",
                "endpoints, credentials and model configuration"
            ]
        }
    })
}

pub fn recent_items_list(
    state: &AppState,
    limit: Option<usize>,
) -> Result<Vec<RecentItem>, String> {
    state
        .db()?
        .recent_items_list(limit.unwrap_or(20))
        .map_err(to_message)
}

pub fn pinned_items_list(state: &AppState) -> Result<Vec<PinnedItem>, String> {
    state.db()?.pinned_items_list().map_err(to_message)
}

pub fn pin_item(state: &AppState, node_id: i64, item_kind: String) -> Result<(), String> {
    state
        .db()?
        .pin_item(node_id, &item_kind)
        .map_err(to_message)
}

pub fn unpin_item(state: &AppState, node_id: i64, item_kind: String) -> Result<(), String> {
    state
        .db()?
        .unpin_item(node_id, &item_kind)
        .map_err(to_message)
}

pub fn comment_add(
    state: &AppState,
    node_id: i64,
    body: String,
    author: Option<String>,
    source: Option<String>,
) -> Result<Comment, String> {
    let author = author.unwrap_or_else(|| "user".to_string());
    let source = source.unwrap_or_else(|| "user".to_string());
    state
        .db()?
        .comment_add(node_id, &body, &author, &source)
        .map_err(to_message)
}

pub fn comments_for_node(state: &AppState, node_id: i64) -> Result<Vec<Comment>, String> {
    state.db()?.comments_for_node(node_id).map_err(to_message)
}

pub fn comments_count_for_node(state: &AppState, node_id: i64) -> Result<i64, String> {
    state
        .db()?
        .comments_count_for_node(node_id)
        .map_err(to_message)
}

pub fn comment_edit(
    state: &AppState,
    comment_id: i64,
    body: String,
    actor: &str,
) -> Result<Comment, String> {
    state
        .db()?
        .comment_edit(comment_id, &body, actor)
        .map_err(to_message)
}

pub fn comment_delete(state: &AppState, comment_id: i64, actor: &str) -> Result<(), String> {
    state
        .db()?
        .comment_delete(comment_id, actor)
        .map_err(to_message)
}

/// Whether connected AI apps are allowed to write comments at all (default OFF).
/// This is the global gate that sits on top of each agent's `comments_write` scope.
pub fn comment_write_enabled(state: &AppState) -> Result<bool, String> {
    state
        .db()?
        .comment_write_enabled_value()
        .map_err(to_message)
}

pub fn set_comment_write_enabled(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_comment_write_enabled(enabled)
        .map_err(to_message)
}

/// Whether the "AI total control" tier is enabled (default OFF, heavily signposted).
/// Even when on, irreversible or human-data-destroying actions still require the
/// in-app double confirmation with a backup offer.
pub fn mcp_full_control_enabled(state: &AppState) -> Result<bool, String> {
    state
        .db()?
        .mcp_full_control_enabled_value()
        .map_err(to_message)
}

pub fn set_mcp_full_control_enabled(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_mcp_full_control_enabled(enabled)
        .map_err(to_message)
}

/// The context an MCP server needs to compute a SCOPE-AWARE `tools/list`: the held
/// app's live scopes plus the two global tier toggles. Read-only and side-effect
/// free (it does not bump `last_seen` — that stays on the real per-call auth). When
/// the token is invalid, revoked or disabled, `scopes` is `None` and the caller
/// advertises only the read-only tool set. This does NOT relax enforcement: every
/// `tools/call` still runs the full authenticated, scope- and toggle-gated dispatch;
/// filtering the advertised list is a UX affordance so an app is not shown tools it
/// cannot use, and `tools/list` is per-session so a per-token view is spec-legal.
#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone)]
pub struct McpCatalogContext {
    /// The app's granted scopes, or `None` when the token does not resolve.
    pub scopes: Option<Vec<String>>,
    /// The default-OFF "total control" tier toggle (gates the `request_*` tools).
    pub total_control_enabled: bool,
    /// The default-OFF final-removal opt-in (additionally gates `request_final_remove`).
    pub final_remove_enabled: bool,
}

#[cfg(feature = "agent_automation")]
impl McpCatalogContext {
    /// Whether the app holds a given scope. False when the token did not resolve.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes
            .as_ref()
            .map(|scopes| scopes.iter().any(|candidate| candidate == scope))
            .unwrap_or(false)
    }
}

/// Resolve the [`McpCatalogContext`] for a held app token (its scopes + the tier
/// toggles). Hashes the token with the same one-way hash the auth path uses and
/// looks the agent up read-only. A DB error surfaces so the caller can fail closed.
#[cfg(feature = "agent_automation")]
pub fn mcp_catalog_context(state: &AppState, token: &str) -> Result<McpCatalogContext, String> {
    let db = state.db()?;
    let scopes = if token.is_empty() {
        None
    } else {
        db.automation_scopes_for_token(&automation_token_hash(token))
            .map_err(to_message)?
    };
    Ok(McpCatalogContext {
        scopes,
        total_control_enabled: db.mcp_full_control_enabled_value().map_err(to_message)?,
        final_remove_enabled: db.final_remove_enabled_value().map_err(to_message)?,
    })
}

pub fn mcp_read_only_mode(state: &AppState) -> Result<bool, String> {
    state.db()?.mcp_read_only_mode_value().map_err(to_message)
}

pub fn set_mcp_read_only_mode(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_mcp_read_only_mode(enabled)
        .map_err(to_message)
}

pub fn roots_list(state: &AppState) -> Result<Vec<ScanRoot>, String> {
    state.db()?.roots_list().map_err(to_message)
}

pub fn roots_add(state: &AppState, path: String) -> Result<ScanRoot, String> {
    let normalized = normalize_root_path(path)?;
    state.db()?.roots_add(&normalized).map_err(to_message)
}

pub fn roots_set_enabled(
    state: &AppState,
    root_id: i64,
    enabled: bool,
) -> Result<ScanRoot, String> {
    if !enabled && state.jobs.has_running_job_for_root(root_id) {
        return Err("Cancel the active scan before disabling this root.".to_string());
    }
    state
        .db()?
        .roots_set_enabled(root_id, enabled)
        .map_err(to_message)
}

pub fn roots_unregister(state: &AppState, root_id: i64) -> Result<(), String> {
    if state.jobs.has_running_job_for_root(root_id) {
        return Err("Cancel the active scan before unregistering this root.".to_string());
    }
    state.db()?.roots_unregister(root_id).map_err(to_message)
}

pub fn projects_unregister(state: &AppState, project_id: i64) -> Result<(), String> {
    state
        .db()?
        .project_unregister(project_id)
        .map_err(to_message)
}

/// Reset all: unregister every scan root and every real project at once, in one
/// atomic transaction. Demo projects are kept; files on disk are never touched.
/// Returns the number of real projects removed.
pub fn reset_all_projects(state: &AppState) -> Result<u64, String> {
    if state.jobs.has_any_running_job() {
        return Err("Cancel the active scan before resetting all projects.".to_string());
    }
    state.db()?.reset_local_inventory().map_err(to_message)
}

/// Disk footprint of the database file before/after a compaction, so the UI can report what the
/// VACUUM reclaimed. Bytes cover the main file plus its sidecar `-wal`/`-shm` files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbMaintenanceReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub freed_bytes: u64,
}

fn database_file_bytes(db_path: &Path) -> u64 {
    let mut total = 0_u64;
    for suffix in ["", "-wal", "-shm"] {
        let path = if suffix.is_empty() {
            db_path.to_path_buf()
        } else {
            let mut name = db_path.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        };
        if let Ok(meta) = std::fs::metadata(&path) {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Compact the local inventory database (VACUUM + WAL truncate) to return the space a big re-scan
/// freed back to the OS. Refused while a scan is running (VACUUM needs the database to itself).
pub fn compact_database(state: &AppState) -> Result<DbMaintenanceReport, String> {
    if state.jobs.has_any_running_job() {
        return Err("Cancel the active scan before compacting the database.".to_string());
    }
    let before_bytes = database_file_bytes(state.db_path());
    state.db()?.compact().map_err(to_message)?;
    let after_bytes = database_file_bytes(state.db_path());
    Ok(DbMaintenanceReport {
        before_bytes,
        after_bytes,
        freed_bytes: before_bytes.saturating_sub(after_bytes),
    })
}

pub fn scan_start(
    state: &AppState,
    root_ids: Option<Vec<i64>>,
    performance_mode: Option<String>,
) -> Result<String, String> {
    let root_ids = root_ids.unwrap_or_default();
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    let db = state.db()?;
    let targets = db.scan_targets_for_ids(&root_ids).map_err(to_message)?;
    for target in &targets {
        if state.jobs.has_running_job_for_root(target.root_id) {
            return Err(format!(
                "A scan is already running for {}.",
                target.display_path
            ));
        }
    }
    let target_root_ids: Vec<i64> = targets.iter().map(|target| target.root_id).collect();
    let cached_estimate = db
        .complete_scan_estimate_for_roots(&target_root_ids)
        .map_err(to_message)?;
    let job_id = state.jobs.create_running_for_roots_with_estimate(
        if let Some(estimate) = cached_estimate {
            format!(
                "Using previous inventory estimate: {} items. Starting local metadata scan.",
                estimate
            )
        } else if mode.is_boost() {
            format!(
                "Estimating read-only inventory size in {} mode.",
                mode.label()
            )
        } else {
            "Estimating read-only inventory size.".to_string()
        },
        target_root_ids,
        targets
            .iter()
            .map(|target| target.display_path.clone())
            .collect(),
        cached_estimate,
    );
    state
        .jobs
        .set_worker_count(&job_id, scan_limits(false, mode).worker_count as u64);
    let jobs = state.jobs.clone();
    let worker_job_id = job_id.clone();

    thread::spawn(move || {
        let _performance = PerformanceScope::enter(mode);
        if let Some(estimated_files) = cached_estimate {
            jobs.set_estimate(
                &worker_job_id,
                estimated_files,
                0,
                format!(
                    "Using previous inventory estimate: {} items. Starting local metadata scan.",
                    estimated_files
                ),
            );
        } else {
            let estimate_started = Instant::now();
            let mut estimated_files = 0_u64;
            let mut estimated_bytes = 0_u64;
            for target in &targets {
                if jobs.is_cancelled(&worker_job_id) {
                    jobs.cancel(&worker_job_id, 0, 0);
                    return;
                }
                let estimate_jobs = jobs.clone();
                let estimate_job_id = worker_job_id.clone();
                let outcome = hangar_fs::estimate_inventory(
                    Path::new(&target.raw_path),
                    None,
                    || jobs.is_cancelled(&worker_job_id),
                    |counted, bytes, current_path| {
                        estimate_jobs.update_estimation(
                            &estimate_job_id,
                            Some(current_path.to_string()),
                            format!(
                                "Estimating {}: {} items, {} seen.",
                                target.display_path,
                                counted,
                                format_bytes_for_message(bytes)
                            ),
                        );
                    },
                );
                let estimate = match outcome {
                    Ok(estimate) => estimate,
                    Err(err) => {
                        jobs.fail(&worker_job_id, err);
                        return;
                    }
                };
                if estimate.cancelled || jobs.is_cancelled(&worker_job_id) {
                    jobs.cancel(&worker_job_id, 0, 0);
                    return;
                }
                estimated_files = estimated_files.saturating_add(estimate.item_count);
                estimated_bytes = estimated_bytes.saturating_add(estimate.apparent_bytes);
            }
            jobs.set_estimate(
                &worker_job_id,
                estimated_files,
                estimated_bytes,
                format!(
                    "Estimate complete: {} items, {}. Starting local metadata scan.",
                    estimated_files,
                    format_bytes_for_message(estimated_bytes)
                ),
            );
            jobs.add_timing(&worker_job_id, "estimate", elapsed_ms(estimate_started));
        }

        let mut scanned = 0;
        let mut indexed = 0;
        let mut writer = match db.open_write_session() {
            Ok(writer) => writer,
            Err(err) => {
                jobs.fail(&worker_job_id, to_message(err));
                return;
            }
        };

        for target in targets {
            if jobs.is_cancelled(&worker_job_id) {
                jobs.cancel(&worker_job_id, scanned, indexed);
                return;
            }

            if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
                jobs.cancel(&worker_job_id, scanned, indexed);
                return;
            }

            let project_id = match writer.begin_root_scan(&target.raw_path) {
                Ok(project_id) => project_id,
                Err(err) => {
                    jobs.fail(&worker_job_id, to_message(err));
                    return;
                }
            };

            let mut persisted_scanned = 0;
            let mut persisted_indexed = 0;
            let progress_jobs = jobs.clone();
            let progress_job_id = worker_job_id.clone();
            let cancel_jobs = jobs.clone();
            let cancel_job_id = worker_job_id.clone();
            let batch_jobs = jobs.clone();
            let batch_job_id = worker_job_id.clone();
            let limits = scan_limits(false, mode);
            let worker_count = limits.worker_count;
            let scan_started = Instant::now();
            let scan_result = hangar_fs::scan_inventory_stream(
                Path::new(&target.raw_path),
                None,
                limits,
                || cancel_jobs.is_cancelled(&cancel_job_id),
                |root_scanned, root_indexed, current_path| {
                    progress_jobs.update_progress(
                        &progress_job_id,
                        scanned + root_scanned,
                        indexed + root_indexed,
                        Some(current_path.to_string()),
                        if mode.is_boost() {
                            format!(
                                "Scanning local metadata in {} mode with {} workers.",
                                mode.label(),
                                worker_count
                            )
                        } else {
                            format!("Scanning local metadata with {} workers.", worker_count)
                        },
                    );
                },
                |batch| {
                    if batch_jobs.is_cancelled(&batch_job_id) {
                        return Err("Cancelled".to_string());
                    }
                    match writer.root_is_enabled(target.root_id) {
                        Ok(true) => {}
                        Ok(false) => return Err("Scan root no longer active.".to_string()),
                        Err(err) => return Err(to_message(err)),
                    }
                    batch_jobs.update_phase(
                        &batch_job_id,
                        "persisting",
                        None,
                        format!(
                            "Persisting {} metadata items to the local database.",
                            batch.len()
                        ),
                    );
                    let persist_started = Instant::now();
                    let persist_result = writer.persist_batch(project_id, &batch);
                    batch_jobs.add_timing(&batch_job_id, "persist", elapsed_ms(persist_started));
                    let (batch_scanned, batch_indexed) = persist_result.map_err(to_message)?;
                    persisted_scanned += batch_scanned;
                    persisted_indexed += batch_indexed;
                    batch_jobs.update_progress(
                        &batch_job_id,
                        scanned + persisted_scanned,
                        indexed + persisted_indexed,
                        None,
                        if mode.is_boost() {
                            format!("Persisted metadata batch in {} mode.", mode.label())
                        } else {
                            "Persisted local metadata batch.".to_string()
                        },
                    );
                    Ok(())
                },
            );
            jobs.add_timing(&worker_job_id, "scan", elapsed_ms(scan_started));
            let outcome = match scan_result {
                Ok(outcome) => outcome,
                Err(err) if err.eq_ignore_ascii_case("Cancelled") => {
                    jobs.cancel(
                        &worker_job_id,
                        scanned + persisted_scanned,
                        indexed + persisted_indexed,
                    );
                    return;
                }
                Err(err) if err == "Scan root no longer active." => {
                    jobs.cancel(
                        &worker_job_id,
                        scanned + persisted_scanned,
                        indexed + persisted_indexed,
                    );
                    return;
                }
                Err(err) => {
                    jobs.fail(&worker_job_id, err);
                    return;
                }
            };

            if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
                jobs.cancel(
                    &worker_job_id,
                    scanned + persisted_scanned,
                    indexed + persisted_indexed,
                );
                return;
            }
            jobs.update_phase(
                &worker_job_id,
                "finalizing",
                None,
                "Finalizing file tree sizes, counts and local context metadata.",
            );
            let finish_jobs = jobs.clone();
            let finish_job_id = worker_job_id.clone();
            let root_scan_completed =
                !outcome.cancelled && !outcome.partial && !jobs.is_cancelled(&worker_job_id);
            let finish_cancel = jobs.cancel_token(&worker_job_id);
            let finish_started = Instant::now();
            let timing_jobs = jobs.clone();
            let timing_job_id = worker_job_id.clone();
            let finish_result = writer.finish_root_scan_interruptible_with_progress_and_timing(
                project_id,
                RootScanFinish {
                    root_path: &target.raw_path,
                    git: outcome.git.as_ref(),
                    scan_completed: root_scan_completed,
                    cancel: Some(finish_cancel),
                },
                |message| {
                    finish_jobs.update_phase(&finish_job_id, "finalizing", None, message);
                },
                |timing| {
                    timing_jobs.add_timing(&timing_job_id, "accounting_select", timing.select_ms);
                    timing_jobs.add_timing(&timing_job_id, "accounting_compute", timing.compute_ms);
                    timing_jobs.add_timing(&timing_job_id, "accounting_update", timing.update_ms);
                },
            );
            jobs.add_timing(&worker_job_id, "finalize", elapsed_ms(finish_started));
            if let Err(err) = finish_result {
                if jobs.is_cancelled(&worker_job_id) || is_cancelled_message(&to_message(&err)) {
                    if root_scan_completed {
                        if let Err(mark_err) = writer.mark_root_scan_incomplete(&target.raw_path) {
                            jobs.fail(&worker_job_id, to_message(mark_err));
                            return;
                        }
                    }
                    jobs.cancel(
                        &worker_job_id,
                        scanned + outcome.scanned_files,
                        indexed + outcome.indexed_documents,
                    );
                } else {
                    jobs.fail(&worker_job_id, to_message(err));
                }
                return;
            }

            if outcome.cancelled || jobs.is_cancelled(&worker_job_id) {
                if root_scan_completed {
                    if let Err(err) = writer.mark_root_scan_incomplete(&target.raw_path) {
                        jobs.fail(&worker_job_id, to_message(err));
                        return;
                    }
                }
                jobs.cancel(
                    &worker_job_id,
                    scanned + outcome.scanned_files,
                    indexed + outcome.indexed_documents,
                );
                return;
            }
            scanned += outcome.scanned_files;
            indexed += outcome.indexed_documents;
            if outcome.partial {
                jobs.complete_partial(
                    &worker_job_id,
                    scanned,
                    indexed,
                    outcome.partial_error.clone().unwrap_or_else(|| {
                        "Inventory scan stopped with an incomplete count.".to_string()
                    }),
                );
                return;
            }
        }

        jobs.complete(&worker_job_id, scanned, indexed);
    });

    Ok(job_id)
}

pub fn scan_resume_subtree(
    state: &AppState,
    nav_id: i64,
    performance_mode: Option<String>,
) -> Result<String, String> {
    let db = state.db()?;
    let target = db.subtree_scan_target(nav_id).map_err(to_message)?;
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    if state.jobs.has_running_job_for_root(target.root_id) {
        return Err(format!(
            "A scan is already running for {}.",
            target.display_root_path
        ));
    }
    let job_id = state.jobs.create_running_for_roots_with_estimate(
        if mode.is_boost() {
            format!(
                "Estimating {} before continuing scan in {} mode.",
                target.relative_path,
                mode.label()
            )
        } else {
            format!(
                "Estimating {} before continuing scan.",
                target.relative_path
            )
        },
        vec![target.root_id],
        vec![target.display_root_path.clone()],
        None,
    );
    state
        .jobs
        .set_worker_count(&job_id, scan_limits(true, mode).worker_count as u64);
    let jobs = state.jobs.clone();
    let worker_job_id = job_id.clone();

    thread::spawn(move || {
        let _performance = PerformanceScope::enter(mode);
        let estimate_jobs = jobs.clone();
        let estimate_job_id = worker_job_id.clone();
        let estimate_started = Instant::now();
        let estimate_result = hangar_fs::estimate_inventory(
            Path::new(&target.root_path),
            Some(&target.relative_path),
            || jobs.is_cancelled(&worker_job_id),
            |counted, bytes, current_path| {
                estimate_jobs.update_estimation(
                    &estimate_job_id,
                    Some(current_path.to_string()),
                    format!(
                        "Estimating subtree: {} items, {} seen.",
                        counted,
                        format_bytes_for_message(bytes)
                    ),
                );
            },
        );
        jobs.add_timing(&worker_job_id, "estimate", elapsed_ms(estimate_started));
        let estimate = match estimate_result {
            Ok(estimate) => estimate,
            Err(err) => {
                jobs.fail(&worker_job_id, err);
                return;
            }
        };
        if estimate.cancelled || jobs.is_cancelled(&worker_job_id) {
            jobs.cancel(&worker_job_id, 0, 0);
            return;
        }
        jobs.set_estimate(
            &worker_job_id,
            estimate.item_count,
            estimate.apparent_bytes,
            format!(
                "Estimate complete: {} items, {}. Continuing local metadata scan.",
                estimate.item_count,
                format_bytes_for_message(estimate.apparent_bytes)
            ),
        );

        let mut writer = match db.open_write_session() {
            Ok(writer) => writer,
            Err(err) => {
                jobs.fail(&worker_job_id, to_message(err));
                return;
            }
        };
        if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
            jobs.cancel(&worker_job_id, 0, 0);
            return;
        }
        if let Err(err) = writer.begin_subtree_scan(target.project_id, target.nav_id) {
            jobs.fail(&worker_job_id, to_message(err));
            return;
        }

        let mut persisted_scanned = 0;
        let mut persisted_indexed = 0;
        let progress_jobs = jobs.clone();
        let progress_job_id = worker_job_id.clone();
        let cancel_jobs = jobs.clone();
        let cancel_job_id = worker_job_id.clone();
        let batch_jobs = jobs.clone();
        let batch_job_id = worker_job_id.clone();
        let limits = scan_limits(true, mode);
        let worker_count = limits.worker_count;
        let scan_started = Instant::now();
        let scan_result = hangar_fs::scan_inventory_stream(
            Path::new(&target.root_path),
            Some(&target.relative_path),
            limits,
            || cancel_jobs.is_cancelled(&cancel_job_id),
            |scanned, indexed, current_path| {
                progress_jobs.update_progress(
                    &progress_job_id,
                    scanned,
                    indexed,
                    Some(current_path.to_string()),
                    if mode.is_boost() {
                        format!(
                            "Continuing local metadata scan in {} mode with {} workers.",
                            mode.label(),
                            worker_count
                        )
                    } else {
                        format!(
                            "Continuing local metadata scan with {} workers.",
                            worker_count
                        )
                    },
                );
            },
            |batch| {
                if batch_jobs.is_cancelled(&batch_job_id) {
                    return Err("Cancelled".to_string());
                }
                match writer.root_is_enabled(target.root_id) {
                    Ok(true) => {}
                    Ok(false) => return Err("Scan root no longer active.".to_string()),
                    Err(err) => return Err(to_message(err)),
                }
                batch_jobs.update_phase(
                    &batch_job_id,
                    "persisting",
                    None,
                    format!(
                        "Persisting {} subtree metadata items to the local database.",
                        batch.len()
                    ),
                );
                let persist_started = Instant::now();
                let persist_result = writer.persist_batch(target.project_id, &batch);
                batch_jobs.add_timing(&batch_job_id, "persist", elapsed_ms(persist_started));
                let (batch_scanned, batch_indexed) = persist_result.map_err(to_message)?;
                persisted_scanned += batch_scanned;
                persisted_indexed += batch_indexed;
                batch_jobs.update_progress(
                    &batch_job_id,
                    persisted_scanned,
                    persisted_indexed,
                    None,
                    if mode.is_boost() {
                        format!(
                            "Persisted continued metadata batch in {} mode.",
                            mode.label()
                        )
                    } else {
                        "Persisted continued metadata batch.".to_string()
                    },
                );
                Ok(())
            },
        );
        jobs.add_timing(&worker_job_id, "scan", elapsed_ms(scan_started));
        let outcome = match scan_result {
            Ok(outcome) => outcome,
            Err(err) if err.eq_ignore_ascii_case("Cancelled") => {
                jobs.cancel(&worker_job_id, persisted_scanned, persisted_indexed);
                return;
            }
            Err(err) if err == "Scan root no longer active." => {
                jobs.cancel(&worker_job_id, persisted_scanned, persisted_indexed);
                return;
            }
            Err(err) => {
                jobs.fail(&worker_job_id, err);
                return;
            }
        };

        let partial_error = if outcome.cancelled {
            Some("Cancelled")
        } else {
            outcome.partial_error.as_deref()
        };
        jobs.update_phase(
            &worker_job_id,
            "finalizing",
            None,
            "Finalizing subtree sizes, counts and local context metadata.",
        );
        let finish_jobs = jobs.clone();
        let finish_job_id = worker_job_id.clone();
        let finish_cancel = jobs.cancel_token(&worker_job_id);
        let finish_started = Instant::now();
        let timing_jobs = jobs.clone();
        let timing_job_id = worker_job_id.clone();
        let finish_result = writer.finish_subtree_scan_interruptible_with_progress_and_timing(
            target.project_id,
            target.nav_id,
            partial_error,
            finish_cancel,
            |message| {
                finish_jobs.update_phase(&finish_job_id, "finalizing", None, message);
            },
            |timing| {
                timing_jobs.add_timing(&timing_job_id, "accounting_select", timing.select_ms);
                timing_jobs.add_timing(&timing_job_id, "accounting_compute", timing.compute_ms);
                timing_jobs.add_timing(&timing_job_id, "accounting_update", timing.update_ms);
            },
        );
        jobs.add_timing(&worker_job_id, "finalize", elapsed_ms(finish_started));
        if let Err(err) = finish_result {
            if jobs.is_cancelled(&worker_job_id) || is_cancelled_message(&to_message(&err)) {
                if let Err(mark_err) =
                    writer.mark_subtree_scan_incomplete(target.nav_id, "Cancelled")
                {
                    jobs.fail(&worker_job_id, to_message(mark_err));
                    return;
                }
                jobs.cancel(
                    &worker_job_id,
                    outcome.scanned_files,
                    outcome.indexed_documents,
                );
            } else {
                jobs.fail(&worker_job_id, to_message(err));
            }
            return;
        }
        if outcome.cancelled || jobs.is_cancelled(&worker_job_id) {
            jobs.cancel(
                &worker_job_id,
                outcome.scanned_files,
                outcome.indexed_documents,
            );
            return;
        }
        if outcome.partial {
            jobs.complete_partial(
                &worker_job_id,
                outcome.scanned_files,
                outcome.indexed_documents,
                outcome.partial_error.clone().unwrap_or_else(|| {
                    "Subtree scan stopped with an incomplete count.".to_string()
                }),
            );
            return;
        }
        jobs.complete(
            &worker_job_id,
            outcome.scanned_files,
            outcome.indexed_documents,
        );
    });

    Ok(job_id)
}

pub fn scan_cancel(state: &AppState, job_id: String) -> Result<(), String> {
    state.jobs.request_cancel(&job_id);
    Ok(())
}

pub fn scan_status(state: &AppState, job_id: String) -> Result<ScanStatus, String> {
    state
        .jobs
        .status(&job_id)
        .ok_or_else(|| format!("Unknown scan job: {job_id}"))
}

pub fn zones_list(state: &AppState) -> Result<Vec<hangar_core::ProtectedZone>, String> {
    state.db()?.zones_list().map_err(to_message)
}

pub fn security_status() -> Result<SecurityStatus, String> {
    Ok(hangar_security::base_security_status())
}

fn normalize_root_path(path: String) -> Result<String, String> {
    let path_buf = PathBuf::from(path);
    let canonical = path_buf
        .canonicalize()
        .map_err(|err| format!("Cannot register scan root: {err}"))?;
    Ok(canonical.to_string_lossy().to_string())
}

fn same_display_path(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        display_path_for_path(value)
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

fn to_message(error: impl ToString) -> String {
    error.to_string()
}

fn is_cancelled_message(message: &str) -> bool {
    message.eq_ignore_ascii_case("cancelled") || message.ends_with(": Cancelled")
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn format_bytes_for_message(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.1} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.1} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_payload_omits_paths_names_messages_notes_and_descriptions() {
        let startup = StartupStatus {
            state: "ready".to_string(),
            message: r"Database opened at C:\Users\person\private.sqlite3".to_string(),
            elapsed_ms: 25,
            db_open_ms: Some(10),
        };
        let security = SecurityStatus {
            outbound_network: "disabled".to_string(),
            mutation_executor: "compiled".to_string(),
            agent_ipc: "not compiled".to_string(),
            active_features: vec!["core".to_string()],
            notes: vec!["private security note".to_string()],
        };
        let dashboard = DashboardSummary {
            total_projects: 3,
            total_items: 30,
            context_files: 4,
            indexed_documents: 5,
            non_indexed_items: 25,
            partial_items: 1,
            git_projects: 2,
            sensitive_files: 6,
            protected_files: 7,
            scan_roots: 2,
            largest_projects: vec![hangar_core::ProjectFootprintSummary {
                project_id: 1,
                name: "PrivateProjectName".to_string(),
                path: r"C:\Users\person\PrivateProjectName".to_string(),
                apparent_bytes: 100,
                allocated_bytes: Some(100),
                physical_bytes: Some(100),
                footprint_partial: false,
            }],
            stale_or_dirty: "current".to_string(),
            adapters_needing_review: 0,
        };
        let adapters = vec![AdapterSummary {
            id: 1,
            name: "generic_git_project".to_string(),
            version: "1".to_string(),
            adapter_type: "builtin".to_string(),
            source: r"C:\private\adapter".to_string(),
            enabled: true,
            description: "private adapter description".to_string(),
        }];
        let resources = SystemResourceProfile {
            logical_cpu_count: 8,
            total_memory_bytes: Some(16 * 1024 * 1024 * 1024),
            available_memory_bytes: Some(8 * 1024 * 1024 * 1024),
            gpu_acceleration: r"driver at C:\private\gpu".to_string(),
            dedicated_vram_bytes: Some(4 * 1024 * 1024 * 1024),
            plans: Vec::new(),
        };

        let encoded = serde_json::to_string(&diagnostics_payload(
            &startup, &security, &dashboard, &adapters, &resources, 2, "Local",
        ))
        .unwrap();

        assert!(encoded.contains("code-hangar/diagnostics/v1"));
        assert!(encoded.contains("generic_git_project"));
        assert!(encoded.contains("savedProjectCheckpoints\":2"));
        for private_value in [
            r"C:\Users\person",
            "PrivateProjectName",
            "private security note",
            "private adapter description",
            r"C:\private",
        ] {
            assert!(!encoded.contains(private_value), "leaked {private_value}");
        }
    }

    // The WSL registry probe reads distro `DistributionName` values without ever
    // invoking wsl.exe; here we mock that raw read and exercise the pure parsing
    // seam (`filter_wsl_distro_names`) that shapes the presence offer.
    #[test]
    fn wsl_registry_filter_trims_dedups_and_skips_system_distros() {
        let raw = vec![
            "  Ubuntu-24.04  ".to_string(), // leading/trailing whitespace
            "ubuntu-24.04".to_string(),     // case-insensitive duplicate
            "docker-desktop".to_string(),   // container-runtime distro
            "docker-desktop-data".to_string(),
            "rancher-desktop".to_string(),
            "Debian".to_string(),
            String::new(), // empty
        ];
        // First spelling wins on the dedup; input order is preserved.
        assert_eq!(
            filter_wsl_distro_names(raw),
            vec!["Ubuntu-24.04".to_string(), "Debian".to_string()]
        );
    }

    #[test]
    fn wsl_registry_filter_reports_absent_when_no_user_distros() {
        // Only container-runtime distros present → no offer.
        assert!(filter_wsl_distro_names(vec![
            "docker-desktop".to_string(),
            "Rancher-Desktop-Data".to_string(),
        ])
        .is_empty());
        assert!(filter_wsl_distro_names(Vec::new()).is_empty());
    }

    #[test]
    fn wsl_system_distro_names_are_recognized_case_insensitively() {
        assert!(is_system_wsl_distro_name("docker-desktop"));
        assert!(is_system_wsl_distro_name("Docker-Desktop-Data"));
        assert!(is_system_wsl_distro_name("rancher-desktop"));
        assert!(!is_system_wsl_distro_name("Ubuntu"));
        assert!(!is_system_wsl_distro_name("Debian"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "real-machine WSL lifecycle check; run with scripts/acceptance-v011.ps1 -Lane WslOff"]
    fn real_wsl_opt_out_does_not_start_a_stopped_distro() {
        use std::os::windows::process::CommandExt;

        fn running_distros() -> Vec<String> {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let output = std::process::Command::new("wsl.exe")
                .args(["--list", "--running", "--quiet"])
                .env("WSL_UTF8", "1")
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .expect("wsl.exe --list --running must be available for this lane");
            assert!(
                output.status.success(),
                "wsl.exe --list --running failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut values = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line.trim().trim_matches('\u{0}').trim().to_string())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();
            values.sort_by_key(|value| value.to_ascii_lowercase());
            values
        }

        let before = running_distros();
        let state = AppState::memory().unwrap();
        set_wsl_scan_enabled(&state, false).unwrap();
        assert!(!wsl_scan_enabled(&state));

        let apps = detect_installed_apps();
        let wsl_offer = apps.iter().find(|app| app.id == "wsl");
        if wsl_offer.is_none() {
            println!("WSL opt-out probe skipped: this user has no registered WSL distro");
            return;
        }
        let label = &wsl_offer.unwrap().label;
        assert!(
            label.contains("Enable WSL scanning"),
            "unexpected WSL offer: {label}"
        );
        assert!(
            !apps.iter().any(|app| app.id.starts_with("wsl:")),
            "per-app WSL probes must remain absent while scanning is off"
        );

        let after = running_distros();
        assert_eq!(
            after, before,
            "opted-out installed-app detection changed the running WSL distro set"
        );
        println!(
            "WSL opt-out preserved running distros {:?}; offer: {}",
            after, label
        );
    }

    #[cfg(feature = "agent_automation")]
    fn automation_request(
        token: Option<&str>,
        method: hangar_agent::AgentMethod,
        params: serde_json::Value,
    ) -> hangar_agent::AgentRequest {
        hangar_agent::AgentRequest {
            protocol: hangar_agent::PROTOCOL_VERSION.to_string(),
            request_id: "test-request".to_string(),
            token: token.map(ToString::to_string),
            method,
            params,
        }
    }

    #[cfg(feature = "agent_automation")]
    fn register_test_automation(
        state: &AppState,
        name: &str,
        token: &str,
        scopes: &[&str],
        project_ids: &[i64],
    ) -> AutomationAgentSummary {
        state
            .db()
            .unwrap()
            .automation_register(
                name,
                &automation_token_hash(token),
                &scopes
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>(),
                project_ids,
            )
            .unwrap()
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_registration_never_promotes_empty_scope_to_all_projects() {
        let state = AppState::memory().unwrap();
        let error = mcp_appconfig_register(&state, "cursor".to_string(), Vec::new())
            .expect_err("an empty project scope must fail closed");
        assert_eq!(
            error,
            "Choose at least one project before connecting an AI app."
        );
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn invalid_connected_app_scope_does_not_revoke_the_current_credential() {
        let state = AppState::memory().unwrap();
        let existing =
            register_test_automation(&state, "Cursor", "existing-token", &["read_structure"], &[]);

        let error = mcp_appconfig_register(&state, "cursor".to_string(), vec![9_999])
            .expect_err("an unknown project must be rejected before credential rotation");
        assert_eq!(error, "One or more selected projects no longer exist.");
        let agents = state.db().unwrap().automation_agents().unwrap();
        assert!(agents
            .iter()
            .any(|agent| agent.id == existing.id && agent.enabled));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn every_request_method_counts_as_a_write_for_read_only_mode() {
        use hangar_agent::AgentMethod;
        // Filing ANY pending-request row is a write, so the read-only panic switch
        // must refuse all of them — including RequestReadBody, whose request is only
        // to READ (the row it inserts is still a mutation of the request queue).
        assert!(automation_method_is_write(&AgentMethod::RequestReadBody));
        assert!(automation_method_is_write(
            &AgentMethod::RequestCommentChange
        ));
        assert!(automation_method_is_write(
            &AgentMethod::RequestBackupProtected
        ));
        assert!(automation_method_is_write(
            &AgentMethod::RequestMoveToHolding
        ));
        assert!(automation_method_is_write(
            &AgentMethod::RequestPermanentDelete
        ));
        // A DIRECT read is not a write: AgentReadBody returns a body without touching
        // any row, and ListMyRequests only SELECTs the caller's own requests.
        assert!(!automation_method_is_write(&AgentMethod::AgentReadBody));
        assert!(!automation_method_is_write(&AgentMethod::ListMyRequests));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn switching_remote_host_drops_the_prior_key_decision() {
        // Same host (default port vs explicit, path change) -> keep the key.
        assert!(!remote_host_changed(
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/chat"
        ));
        assert!(!remote_host_changed(
            "https://api.openai.com/v1",
            "https://api.openai.com:443/v1"
        ));
        // A different host -> the key would go to a NEW provider, so it must be dropped.
        assert!(remote_host_changed(
            "https://api.openai.com/v1",
            "https://openrouter.ai/api/v1"
        ));
        // No comparable prior origin (fresh install / never configured) -> a FIRST set,
        // not a change: do NOT clear (that would wipe a key just entered for this host).
        assert!(!remote_host_changed("", "https://api.openai.com/v1"));
        // A former LOCAL endpoint parses to its own origin, so switching to a remote host
        // IS a change of origin -> clear (a local server's context must not carry over).
        assert!(remote_host_changed(
            "http://localhost:11434/v1",
            "https://api.openai.com/v1"
        ));
    }

    // A full round trip through `ai_provider_set`: saving a key, then switching the
    // remote host, must leave no key saved. Uses the REAL OS keychain, so it is gated
    // to Windows (where the desktop app runs) and is self-cleaning — it removes any key
    // it wrote whether it passes or fails, and no-ops if the keychain is unavailable.
    #[cfg(all(windows, feature = "agent_automation"))]
    #[test]
    fn ai_provider_set_new_remote_host_clears_saved_key() {
        let state = AppState::memory().unwrap();
        // Skip cleanly if this machine has no usable credential store (CI container).
        if ai_assist::ai_key_set("sk-test-key-1234567890").is_err() {
            return;
        }
        // Establish a first remote provider while a key is saved.
        ai_provider_set(
            &state,
            "api",
            "https://api.openai.com/v1",
            "gpt-4o-mini",
            "chat_completions",
        )
        .unwrap();
        assert!(ai_assist::ai_key_status(), "key should still be saved");
        // Switch to a DIFFERENT remote host: the prior key must be dropped.
        ai_provider_set(
            &state,
            "api",
            "https://openrouter.ai/api/v1",
            "gpt-4o-mini",
            "chat_completions",
        )
        .unwrap();
        let still_saved = ai_assist::ai_key_status();
        // Always leave the keychain as we found it, regardless of the assertion.
        let _ = ai_assist::ai_key_clear();
        assert!(!still_saved, "switching remote host must clear the key");
    }

    #[test]
    fn exposes_fixture_projects() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        assert!(!projects.is_empty());
    }

    #[test]
    fn exposes_project_graph_map_without_mutation_or_network() {
        let state = AppState::memory().unwrap();
        let project = projects_list(&state).unwrap().remove(0);
        let map = project_graph_map(&state, project.id, Some(100)).unwrap();
        assert_eq!(map.project_id, project.id);
        assert!(map.nodes.iter().any(|node| node.graph_kind == "project"));
    }

    #[test]
    fn memory_state_reports_ready_startup() {
        let state = AppState::memory().unwrap();
        let status = startup_status(&state);
        assert_eq!(status.state, "ready");
        assert_eq!(status.db_open_ms, Some(0));
    }

    #[test]
    fn exposes_security_status() {
        let status = security_status().unwrap();
        #[cfg(not(feature = "agent_automation"))]
        assert!(status.outbound_network.contains("not implemented"));
        #[cfg(feature = "agent_automation")]
        assert!(status.outbound_network.contains("AI Assist"));
        #[cfg(not(feature = "mutation"))]
        assert!(status.mutation_executor.contains("not compiled"));
        #[cfg(feature = "mutation")]
        assert!(status.mutation_executor.contains("feature-gated"));
        #[cfg(feature = "agent_automation")]
        assert!(status.agent_ipc.contains("local named pipe"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn ai_assist_resolves_only_registered_unprotected_nodes() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let normal_project_id = projects
            .iter()
            .find(|project| project.name == "Fixture Markdown Project")
            .expect("normal fixture project")
            .id;
        let sensitive_project_id = projects
            .iter()
            .find(|project| project.name == "Fixture Sensitive Project")
            .expect("sensitive fixture project")
            .id;
        let normal = quick_open(&state, "README.md".to_string(), Some(20))
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == normal_project_id)
            .expect("normal fixture file");
        assert!(resolve_ai_explain_inventory_target(&state, normal.node_id)
            .unwrap()
            .0
            .starts_with("fixture://markdown-project/"));

        let sensitive = quick_open(&state, ".env".to_string(), Some(20))
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == sensitive_project_id)
            .expect("sensitive fixture file");
        assert!(
            resolve_ai_explain_inventory_target(&state, sensitive.node_id)
                .unwrap_err()
                .contains("Protected Zone")
        );
        assert!(resolve_ai_explain_inventory_target(&state, i64::MAX)
            .unwrap_err()
            .contains("not a present item"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn ai_assist_disk_target_must_remain_inside_the_registered_project() {
        let project = unique_temp_dir("codehangar-ai-boundary");
        let outside = unique_temp_dir("codehangar-ai-outside");
        let file = project.join("src").join("main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}").unwrap();

        validate_ai_explain_disk_target(
            &file.to_string_lossy(),
            &[project.to_string_lossy().to_string()],
        )
        .unwrap();
        assert!(validate_ai_explain_disk_target(
            &file.to_string_lossy(),
            &[outside.to_string_lossy().to_string()],
        )
        .unwrap_err()
        .contains("outside its registered project boundary"));

        let _ = fs::remove_dir_all(project);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn follow_up_memory_is_section_scoped_and_capped_at_three_turns() {
        let state = AppState::memory().unwrap();
        let first = reserve_follow_up_turn(&state, 7, "section-a", None, "First?").unwrap();
        assert!(first.history.is_empty());
        assert_eq!(first.turn, 1);
        let id = first.conversation_id;
        finish_follow_up_turn(&state, &id, first.turn, Some("First answer"));
        assert!(follow_up_history(&state, 7, "section-b", Some(&id)).is_err());

        for expected_turn in 2..=3 {
            let reservation = reserve_follow_up_turn(
                &state,
                7,
                "section-a",
                Some(&id),
                &format!("Question {expected_turn}?"),
            )
            .unwrap();
            assert_eq!(reservation.history.len(), expected_turn - 1);
            assert_eq!(reservation.turn, expected_turn);
            finish_follow_up_turn(
                &state,
                &id,
                reservation.turn,
                Some(&format!("Answer {expected_turn}")),
            );
        }
        assert!(
            reserve_follow_up_turn(&state, 7, "section-a", Some(&id), "Fourth?")
                .unwrap_err()
                .contains("three-turn")
        );
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_guest_is_capabilities_only_and_project_scope_is_enforced() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let allowed_project = projects[0].id;
        let denied_project = projects[1].id;
        let token = "local-test-token";
        register_test_automation(
            &state,
            "Scoped test agent",
            token,
            &["read_structure"],
            &[allowed_project],
        );

        let guest = handle_automation_request(
            &state,
            automation_request(
                None,
                hangar_agent::AgentMethod::Status,
                serde_json::json!({}),
            ),
        );
        assert!(guest.ok);
        assert_eq!(
            guest.result.unwrap()["guestAccess"],
            serde_json::json!("capabilities_only")
        );

        let allowed = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentProjectContext,
                serde_json::json!({ "projectId": allowed_project }),
            ),
        );
        assert!(allowed.ok);
        assert_eq!(allowed.result.unwrap()["bodyContentIncluded"], false);

        let denied = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentProjectContext,
                serde_json::json!({ "projectId": denied_project }),
            ),
        );
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("not scoped"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn list_catalog_is_intersected_with_granted_projects() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let granted = projects[0].id;
        let ungranted = projects[1].id;
        let token = "catalog-token";
        register_test_automation(
            &state,
            "Catalog agent",
            token,
            &["read_structure"],
            &[granted],
        );

        let response = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::ListCatalog,
                serde_json::json!({}),
            ),
        );
        assert!(response.ok);
        let result = response.result.unwrap();
        let ids: Vec<i64> = result["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|project| project["id"].as_i64().unwrap())
            .collect();
        assert!(ids.contains(&granted));
        assert!(
            !ids.contains(&ungranted),
            "list_catalog leaked an un-granted project"
        );
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn graph_tools_require_read_graph_scope_and_project_membership() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let granted = projects[0].id;
        let ungranted = projects[1].id;

        // read_structure alone cannot reach the graph surface.
        let structure_token = "structure-token";
        register_test_automation(
            &state,
            "Structure agent",
            structure_token,
            &["read_structure"],
            &[granted],
        );
        let missing_scope = handle_automation_request(
            &state,
            automation_request(
                Some(structure_token),
                hangar_agent::AgentMethod::GetProjectGraph,
                serde_json::json!({ "projectId": granted }),
            ),
        );
        assert!(!missing_scope.ok);
        assert!(missing_scope.error.unwrap().contains("read_graph"));

        // With read_graph the granted project works and an un-granted one is refused.
        let graph_token = "graph-token";
        register_test_automation(
            &state,
            "Graph agent",
            graph_token,
            &["read_graph"],
            &[granted],
        );
        let allowed = handle_automation_request(
            &state,
            automation_request(
                Some(graph_token),
                hangar_agent::AgentMethod::GetProjectGraph,
                serde_json::json!({ "projectId": granted }),
            ),
        );
        assert!(allowed.ok);
        let denied = handle_automation_request(
            &state,
            automation_request(
                Some(graph_token),
                hangar_agent::AgentMethod::GetProjectGraph,
                serde_json::json!({ "projectId": ungranted }),
            ),
        );
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("not scoped"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn authenticated_project_graph_clamps_a_hostile_limit_to_one_thousand_nodes() {
        let state = AppState::memory().unwrap();
        let root = "fixture://agent-graph-limit";
        let files = (0..1_100)
            .map(|index| {
                let relative_path = format!("workflows/workflow-{index:04}.json");
                hangar_core::ScannedFile {
                    absolute_path: format!("{root}/{relative_path}"),
                    display_path: relative_path.clone(),
                    display_name: format!("workflow-{index:04}.json"),
                    relative_path,
                    item_kind: "file".to_string(),
                    is_markdown: false,
                    is_context: false,
                    is_sensitive: false,
                    protected_level: None,
                    child_count: 0,
                    fully_scanned: true,
                    collapse_default: false,
                    scan_error: None,
                    identity: None,
                    body: Some("{}".to_string()),
                }
            })
            .collect::<Vec<_>>();
        state
            .db()
            .unwrap()
            .load_scanned_root(root, &files, None)
            .unwrap();
        let project_id = projects_list(&state)
            .unwrap()
            .into_iter()
            .find(|project| project.path == root)
            .unwrap()
            .id;
        let local_map = project_graph_map(&state, project_id, Some(50_000)).unwrap();
        assert!(
            local_map.total_nodes > MAX_AUTOMATION_GRAPH_NODES as i64,
            "fixture must exceed the connected-app ceiling"
        );
        let token = "hostile-graph-limit-token";
        register_test_automation(
            &state,
            "Hostile graph limit agent",
            token,
            &["read_graph"],
            &[project_id],
        );

        let response = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::GetProjectGraph,
                serde_json::json!({ "projectId": project_id, "limit": 50_000 }),
            ),
        );

        assert!(response.ok, "{:?}", response.error);
        let map: hangar_core::GraphMap = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(map.nodes.len(), MAX_AUTOMATION_GRAPH_NODES);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn get_project_graph_never_returns_a_node_or_id_outside_the_grant() {
        // Contract guard for the cross-project leak fix: the graph of a granted
        // project must never carry a node, a shared-project id, an issue, or an edge
        // endpoint that belongs to a project the app was not granted — even though
        // the underlying graph can pull cross-project duplicate/workflow edges in.
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let granted = projects[0].id;
        let ungranted = projects[1].id;
        let token = "graph-leak-token";
        register_test_automation(
            &state,
            "Graph leak agent",
            token,
            &["read_graph"],
            &[granted],
        );

        let response = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::GetProjectGraph,
                serde_json::json!({ "projectId": granted }),
            ),
        );
        assert!(response.ok);
        let map: hangar_core::GraphMap = serde_json::from_value(response.result.unwrap()).unwrap();
        let node_ids: std::collections::HashSet<i64> =
            map.nodes.iter().map(|node| node.node_id).collect();
        for node in &map.nodes {
            assert_eq!(node.project_id, granted, "graph returned an ungranted node");
            assert!(
                !node.shared_project_ids.contains(&ungranted),
                "graph leaked an ungranted project id via shared_project_ids"
            );
            assert!(
                node.details
                    .iter()
                    .all(|d| !d.contains("registered project")),
                "graph leaked a cross-project count detail string"
            );
        }
        let leaks_count = |text: &str| {
            text.contains("registered project")
                || text.contains("model candidates")
                || text.contains("model files share")
        };
        for issue in &map.issues {
            assert!(node_ids.contains(&issue.node_id));
            assert!(issue.project_id.is_none_or(|pid| pid == granted));
            assert!(
                !leaks_count(&issue.target),
                "issue.target leaked a cross-project count"
            );
            assert!(
                issue.evidence.as_deref().is_none_or(|e| !leaks_count(e)),
                "issue.evidence leaked a cross-project count"
            );
        }
        for edge in &map.edges {
            assert!(node_ids.contains(&edge.source_node_id));
            assert!(node_ids.contains(&edge.target_node_id));
            assert!(
                edge.evidence.as_deref().is_none_or(|e| !leaks_count(e)),
                "edge.evidence leaked a cross-project count"
            );
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn redact_graph_to_grant_strips_cross_project_nodes_ids_and_counts() {
        // Drives the scrub directly with a synthetic graph carrying cross-project
        // data + the real count strings (the in-memory project fixture has no model
        // or cache nodes, so the integration guard test cannot exercise this).
        use hangar_core::{GraphEdge, GraphIssue, GraphMap, GraphNode};
        let granted = 1_i64;
        let ungranted = 2_i64;
        let node =
            |node_id: i64, project_id: i64, shared: Vec<i64>, details: Vec<&str>| GraphNode {
                node_id,
                project_id,
                path: "rel/path".into(),
                display_name: "n".into(),
                item_kind: "file".into(),
                graph_kind: "cache".into(),
                confidence: "High".into(),
                details: details.into_iter().map(String::from).collect(),
                physical_bytes: Some(10),
                protected_or_sensitive: false,
                shared_project_ids: shared,
            };
        let dup_evidence = "3 model files share 100 bytes and the first 8 KiB hash.";
        let mut map = GraphMap {
            project_id: granted,
            nodes: vec![
                node(
                    10,
                    granted,
                    vec![granted, ungranted],
                    vec!["This cache folder is inventoried by 2 registered projects."],
                ),
                node(11, granted, vec![granted], vec![]),
                node(20, ungranted, vec![ungranted], vec![]),
            ],
            edges: vec![
                // cross-project edge: endpoint 20 is ungranted -> dropped entirely.
                GraphEdge {
                    source_node_id: 10,
                    target_node_id: 20,
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    evidence: Some(dup_evidence.into()),
                },
                // in-grant duplicate edge: kept, but its count evidence is scrubbed.
                GraphEdge {
                    source_node_id: 10,
                    target_node_id: 11,
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    evidence: Some(dup_evidence.into()),
                },
            ],
            issues: vec![
                GraphIssue {
                    node_id: 10,
                    project_id: Some(granted),
                    source_path: None,
                    kind: "shared_cache_candidate".into(),
                    confidence: "High".into(),
                    target: "rel/path".into(),
                    evidence: Some("inventoried by 2 registered projects.".into()),
                },
                GraphIssue {
                    node_id: 10,
                    project_id: Some(granted),
                    source_path: None,
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    target: "3 model candidates".into(),
                    evidence: Some(dup_evidence.into()),
                },
                // issue on the ungranted node -> dropped.
                GraphIssue {
                    node_id: 20,
                    project_id: Some(ungranted),
                    source_path: None,
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    target: "3 model candidates".into(),
                    evidence: Some(dup_evidence.into()),
                },
            ],
            total_nodes: 3,
            total_edges: 2,
            total_issues: 3,
            partial: false,
        };

        redact_graph_to_grant(&mut map, &[granted]);

        // Ungranted node and everything pointing at it is gone; counts/ids scrubbed.
        assert_eq!(
            map.nodes.iter().map(|n| n.node_id).collect::<Vec<_>>(),
            vec![10, 11]
        );
        let n10 = &map.nodes[0];
        assert_eq!(n10.shared_project_ids, vec![granted]);
        assert!(
            n10.details.is_empty(),
            "cross-project count detail survived"
        );
        assert_eq!(map.edges.len(), 1, "cross-project edge survived");
        assert_eq!(map.issues.len(), 2, "ungranted-node issue survived");
        let dup = map
            .issues
            .iter()
            .find(|i| i.kind == "duplicate_model_candidate")
            .unwrap();
        assert_eq!(dup.target, "model candidates");
        assert_eq!(map.total_nodes, 2);
        assert_eq!(map.total_edges, 1);
        assert_eq!(map.total_issues, 2);
        let leaks = |t: &str| {
            t.contains("registered project")
                || t.contains("model files share")
                || t.contains(" model candidates")
        };
        for issue in &map.issues {
            assert!(!leaks(&issue.target), "issue.target leaked a count");
            assert!(issue.evidence.as_deref().is_none_or(|e| !leaks(e)));
        }
        for edge in &map.edges {
            assert!(edge.evidence.as_deref().is_none_or(|e| !leaks(e)));
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn node_relationships_refuse_a_node_in_an_ungranted_project() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let granted = projects[0].id;
        let ungranted = projects[1].id;
        // A real node id that belongs to the un-granted project.
        let ungranted_node = project_graph_map(&state, ungranted, Some(50))
            .unwrap()
            .nodes[0]
            .node_id;
        let token = "rel-token";
        register_test_automation(&state, "Rel agent", token, &["read_graph"], &[granted]);
        let denied = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::NodeRelationships,
                serde_json::json!({ "nodeId": ungranted_node }),
            ),
        );
        // An app scoped only to `granted` can never read a node owned by `ungranted`.
        assert!(!denied.ok);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn explain_folder_requires_structure_scope_and_never_leaks_unknown_nav() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let granted = projects[0].id;
        // Missing read_structure → refused before any lookup.
        let no_scope_token = "noscope-token";
        register_test_automation(
            &state,
            "No-scope agent",
            no_scope_token,
            &["read_graph"],
            &[granted],
        );
        let no_scope = handle_automation_request(
            &state,
            automation_request(
                Some(no_scope_token),
                hangar_agent::AgentMethod::ExplainFolder,
                serde_json::json!({ "navId": 999999 }),
            ),
        );
        assert!(!no_scope.ok);
        assert!(no_scope.error.unwrap().contains("read_structure"));
        // With scope but an unknown nav id → a not-found message, never another
        // project's explanation slipping through the (membership-unchecked) lookup.
        let token = "explain-token";
        register_test_automation(
            &state,
            "Explain agent",
            token,
            &["read_structure"],
            &[granted],
        );
        let unknown = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::ExplainFolder,
                serde_json::json!({ "navId": 999999 }),
            ),
        );
        assert!(!unknown.ok);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_register_rejects_duplicate_active_agent_name() {
        let state = AppState::memory().unwrap();
        let project = projects_list(&state).unwrap().remove(0);
        // First agent claims the name (created enabled via the test helper).
        register_test_automation(
            &state,
            "Assistant",
            "tok-1",
            &["comments_read"],
            &[project.id],
        );
        // A second registration with the same name (any case) is refused, so the
        // comment-ownership key (the name) stays a 1:1 proxy for an active identity.
        let error = automation_register(
            &state,
            "assistant".to_string(),
            vec!["comments_read".to_string()],
            vec![project.id],
        )
        .unwrap_err();
        assert!(error.to_lowercase().contains("already exists"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_comment_tools_respect_write_gate_and_human_boundary() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        // Two distinct projects that each expose at least one context-file node.
        let mut with_files = projects.iter().filter_map(|project| {
            project_context_files(&state, project.id)
                .ok()
                .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
        });
        let (allowed_project, node_id) = with_files.next().expect("a project with a context file");
        let (_denied_project, outside_node) = with_files
            .next()
            .expect("a second project with a context file");

        // "user" is reserved for the local human — the real registration path
        // refuses it in any case, so an app can never adopt that identity.
        assert!(automation_register(
            &state,
            "User".to_string(),
            vec!["comments_read".to_string()],
            vec![allowed_project],
        )
        .unwrap_err()
        .contains("reserved"));

        let token = "comment-tools-token";
        register_test_automation(
            &state,
            "hermes-local",
            token,
            &["comments_read", "comments_write"],
            &[allowed_project],
        );

        // Reads work immediately within scope.
        let listed = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsList,
                serde_json::json!({ "nodeId": node_id }),
            ),
        );
        assert!(listed.ok);

        // With AI write mode OFF (default) a write is refused even with the scope.
        let blocked = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsAdd,
                serde_json::json!({ "nodeId": node_id, "body": "from the agent" }),
            ),
        );
        assert!(!blocked.ok);
        assert!(blocked.error.unwrap().to_lowercase().contains("write mode"));

        // Enable AI write mode; the agent can now add. The stored author/source is
        // the authenticated agent's name — server-assigned, never "user".
        state.db().unwrap().set_comment_write_enabled(true).unwrap();
        let added = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsAdd,
                serde_json::json!({ "nodeId": node_id, "body": "from the agent" }),
            ),
        );
        assert!(added.ok);
        let created: Comment = serde_json::from_value(added.result.unwrap()).unwrap();
        assert_eq!(created.source, "hermes-local");
        assert_eq!(created.author, "hermes-local");

        // The agent may edit its OWN comment.
        let edited = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsEdit,
                serde_json::json!({ "commentId": created.id, "body": "revised by the agent" }),
            ),
        );
        assert!(edited.ok);

        // A HUMAN comment on the same node is untouchable by the agent.
        let human = state
            .db()
            .unwrap()
            .comment_add(node_id, "human note", "user", "user")
            .unwrap();
        let tamper = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsEdit,
                serde_json::json!({ "commentId": human.id, "body": "hijacked" }),
            ),
        );
        assert!(!tamper.ok);
        let human_after = state
            .db()
            .unwrap()
            .comments_for_node(node_id)
            .unwrap()
            .into_iter()
            .find(|comment| comment.id == human.id)
            .unwrap();
        assert_eq!(human_after.body, "human note");

        // Project scope is enforced: a node outside the agent's project is refused.
        let scoped_out = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsList,
                serde_json::json!({ "nodeId": outside_node }),
            ),
        );
        assert!(!scoped_out.ok);
        assert!(scoped_out.error.unwrap().contains("not scoped"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn total_control_request_is_queued_not_executed_until_user_approves_with_backup() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let (project_id, node_id) = projects
            .iter()
            .find_map(|project| {
                project_context_files(&state, project.id)
                    .ok()
                    .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
            })
            .expect("a project with a context-file node");
        let human = state
            .db()
            .unwrap()
            .comment_add(node_id, "human note", "user", "user")
            .unwrap();

        let token = "total-control-token";
        register_test_automation(
            &state,
            "hermes-smart",
            token,
            &["comments_read", "comments_write"],
            &[project_id],
        );

        let edit_request = |body: &str| {
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestCommentChange,
                serde_json::json!({ "commentId": human.id, "action": "edit", "body": body }),
            )
        };

        // With total control OFF (default), the request is refused outright.
        let blocked = handle_automation_request(&state, edit_request("rewrite 1"));
        assert!(!blocked.ok);
        assert!(blocked
            .error
            .unwrap()
            .to_lowercase()
            .contains("total control is off"));

        // Turn total control on. Now the agent may FILE a request — but nothing is
        // executed; the human comment is untouched and a pending row appears.
        state
            .db()
            .unwrap()
            .set_mcp_full_control_enabled(true)
            .unwrap();
        let queued = handle_automation_request(&state, edit_request("rewrite 2"));
        assert!(queued.ok);
        assert_eq!(queued.result.unwrap()["status"], "queued");
        assert_eq!(
            state
                .db()
                .unwrap()
                .comment_get(human.id)
                .unwrap()
                .unwrap()
                .body,
            "human note",
            "the agent must not have changed the human comment"
        );
        let pending = agent_requests_pending(&state).unwrap();
        assert_eq!(pending.len(), 1);

        // Rejecting leaves the human record untouched.
        agent_request_resolve(&state, pending[0].id, false, ResolveInputs::default()).unwrap();
        assert_eq!(
            state
                .db()
                .unwrap()
                .comment_get(human.id)
                .unwrap()
                .unwrap()
                .body,
            "human note"
        );
        assert!(agent_requests_pending(&state).unwrap().is_empty());

        // A second request, approved WITH a backup, executes as the user: the
        // comment changes and a backup file lands in the chosen folder.
        let queued2 = handle_automation_request(&state, edit_request("approved rewrite"));
        assert!(queued2.ok);
        let request_id = agent_requests_pending(&state).unwrap()[0].id;
        let backup_dir = tempfile::tempdir().unwrap();
        let resolved = agent_request_resolve(
            &state,
            request_id,
            true,
            ResolveInputs {
                backup_dir: Some(backup_dir.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(resolved.status, "approved");
        // The edit was applied AS the user (source stays "user").
        let after = state.db().unwrap().comment_get(human.id).unwrap().unwrap();
        assert_eq!(after.body, "approved rewrite");
        assert_eq!(after.source, "user");
        // A backup file of the prior state was written to the safe folder.
        let backups: Vec<_> = std::fs::read_dir(backup_dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0]
            .file_name()
            .to_string_lossy()
            .starts_with("codehangar-comment-"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn approving_a_revoked_agents_request_is_refused() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let (project_id, node_id) = projects
            .iter()
            .find_map(|project| {
                project_context_files(&state, project.id)
                    .ok()
                    .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
            })
            .expect("a project with a context-file node");
        let human = state
            .db()
            .unwrap()
            .comment_add(node_id, "human note", "user", "user")
            .unwrap();

        let token = "revoke-test-token";
        let agent = register_test_automation(
            &state,
            "hermes-revoked",
            token,
            &["comments_read", "comments_write"],
            &[project_id],
        );
        state
            .db()
            .unwrap()
            .set_mcp_full_control_enabled(true)
            .unwrap();

        // The agent files a request to edit the human comment.
        let queued = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestCommentChange,
                serde_json::json!({ "commentId": human.id, "action": "edit", "body": "sneaky rewrite" }),
            ),
        );
        assert!(queued.ok);
        let request_id = agent_requests_pending(&state).unwrap()[0].id;

        // The user revokes the agent BEFORE getting to the approval.
        state.db().unwrap().automation_revoke(agent.id).unwrap();

        // Approving the now-revoked agent's queued request is refused, and the human
        // comment is left untouched — a revoked agent's queued authority does not
        // survive.
        let result = agent_request_resolve(&state, request_id, true, ResolveInputs::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_lowercase().contains("revoked"));
        assert_eq!(
            state
                .db()
                .unwrap()
                .comment_get(human.id)
                .unwrap()
                .unwrap()
                .body,
            "human note"
        );
        assert!(agent_requests_pending(&state).unwrap().is_empty());
    }

    /// Build a `ContextFile` for the context-assembly tests. Only the fields the assembly reads
    /// (path/display_name/recommended/is_sensitive/protected_level) carry meaning here.
    #[cfg(feature = "agent_automation")]
    fn test_context_file(
        path: &str,
        recommended: bool,
        is_sensitive: bool,
        protected_level: Option<&str>,
    ) -> ContextFile {
        let display_name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
        ContextFile {
            nav_id: 0,
            node_id: 0,
            project_id: 0,
            path: path.to_string(),
            display_name,
            priority: 0,
            context_rank: if recommended { 0 } else { 100 },
            context_group: "docs".to_string(),
            recommendation_reason: String::new(),
            recommended,
            is_sensitive,
            protected_level: protected_level.map(str::to_string),
        }
    }

    // A3: the curated "Recommended context" file names are the best signal the app has about a
    // project, so they MUST appear in the assembled AI-summary input.
    #[cfg(feature = "agent_automation")]
    #[test]
    fn ai_summary_context_includes_curated_context_file_names() {
        let summary = hangar_core::ProjectContextSummary {
            readme_title: Some("My App".to_string()),
            readme_excerpt: Some("It does a thing.".to_string()),
            kinds: vec!["Rust".to_string()],
            run_commands: vec!["cargo run".to_string()],
            manifest_files: vec!["Cargo.toml".to_string()],
            markdown_files: vec!["README.md".to_string()],
        };
        let context_files = vec![
            test_context_file("AGENTS.md", true, false, None),
            test_context_file("docs/architecture.md", true, false, None),
            // A NON-recommended file (deep nested readme) is listed by the DB but must not be
            // presented as recommended context.
            test_context_file("crates/x/sub/README.md", false, false, None),
        ];
        // Non-existent root: no excerpt can be read, so this isolates the NAMES behavior.
        let assembled = project_ai_context_text(&summary, "Z:\\does\\not\\exist", &context_files);
        assert!(
            assembled.contains("Recommended context files:"),
            "assembled: {assembled}"
        );
        assert!(assembled.contains("AGENTS.md"));
        assert!(assembled.contains("docs/architecture.md"));
        // The non-recommended file is not surfaced as recommended context.
        assert!(!assembled.contains("crates/x/sub/README.md"));
    }

    // A3 SECURITY: a sensitive/Protected/secret-bearing context file must never contribute its
    // bytes NOR its PATH to the summary prompt, even when it is on disk and marked recommended.
    // A recommended file's name like `docs/credentials.md` is itself a leak, so such files are
    // filtered at the source (see `project_ai_context_text`). A clean file's name AND excerpt IS
    // included, proving the wiring still works.
    #[cfg(feature = "agent_automation")]
    #[test]
    fn ai_summary_excerpts_exclude_sensitive_and_protected_context_files() {
        let root = unique_temp_dir("codehangar-a3-ctx");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();

        // Clean recommended doc -> name + excerpt included.
        std::fs::write(
            root.join("docs").join("guide.md"),
            "# Guide\n\nHow this project is structured and run.",
        )
        .unwrap();
        // Recommended per the DB, but flagged sensitive by the inventory -> name + body withheld.
        std::fs::write(
            root.join("docs").join("credentials.md"),
            "internal-only design notes that must not be summarized",
        )
        .unwrap();
        // Recommended, but sits in the Protected Zone -> name + body withheld.
        std::fs::write(
            root.join("docs").join("secrets.md"),
            "protected-zone material that must not be summarized",
        )
        .unwrap();
        // Recommended, not flagged, but its BYTES carry a secret -> name is safe, excerpt refused.
        std::fs::write(
            root.join("docs").join("keys.md"),
            "deploy token ghp_abcdefghijklmnopqrstuvwxyz0123456789 do not share",
        )
        .unwrap();

        let summary = hangar_core::ProjectContextSummary::default();
        let context_files = vec![
            test_context_file("docs/guide.md", true, false, None),
            test_context_file("docs/credentials.md", true, true, None),
            test_context_file("docs/secrets.md", true, false, Some("Protected")),
            test_context_file("docs/keys.md", true, false, None),
        ];
        let assembled = project_ai_context_text(&summary, &root.to_string_lossy(), &context_files);

        // The clean file's NAME and CONTENT are present.
        assert!(assembled.contains("docs/guide.md"));
        assert!(
            assembled.contains("How this project is structured"),
            "clean excerpt must be included: {assembled}"
        );
        // A non-sensitive file's name is still safe to list even when its excerpt is gated out.
        assert!(assembled.contains("docs/keys.md"));

        // SECURITY: the sensitive / Protected files' PATHS must NOT appear anywhere in the prompt
        // (the path itself is the leak this fix closes), and neither must their bodies.
        assert!(
            !assembled.contains("credentials.md"),
            "a sensitive context file's PATH must never reach the prompt: {assembled}"
        );
        assert!(
            !assembled.contains("secrets.md"),
            "a Protected context file's PATH must never reach the prompt: {assembled}"
        );
        assert!(
            !assembled.contains("internal-only design notes"),
            "a sensitive-flagged context file's bytes must never be included"
        );
        assert!(
            !assembled.contains("protected-zone material"),
            "a Protected context file's bytes must never be included"
        );
        // The secret-bearing file's content (and the secret) is excluded by the gate.
        assert!(
            !assembled.contains("ghp_"),
            "a secret in a context file must never reach the prompt"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn read_only_mode_refuses_writes_but_allows_reads() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let project_id = projects[0].id;
        let node_id = project_context_files(&state, project_id).unwrap()[0].node_id;
        let token = "read-only-token";
        register_test_automation(
            &state,
            "hermes-frozen",
            token,
            &["read_structure", "comments_read", "comments_write"],
            &[project_id],
        );
        // Writes are globally enabled, but the read-only panic switch is the override.
        state.db().unwrap().set_comment_write_enabled(true).unwrap();
        state.db().unwrap().set_mcp_read_only_mode(true).unwrap();

        // A read still works while frozen.
        let read = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentProjectContext,
                serde_json::json!({ "projectId": project_id }),
            ),
        );
        assert!(read.ok, "reads must still work in read-only mode");

        // A write is refused with the read-only message, even with the write toggle on.
        let write = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsAdd,
                serde_json::json!({ "nodeId": node_id, "body": "noise" }),
            ),
        );
        assert!(!write.ok);
        assert!(write.error.unwrap().to_lowercase().contains("read-only"));

        // Turning it off lets the same write through.
        state.db().unwrap().set_mcp_read_only_mode(false).unwrap();
        let allowed = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::CommentsAdd,
                serde_json::json!({ "nodeId": node_id, "body": "noise" }),
            ),
        );
        assert!(allowed.ok);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn total_control_mutation_requests_are_gated_at_filing_and_resolve() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let project_id = projects[0].id;
        let node_id = project_context_files(&state, project_id).unwrap()[0].node_id;
        let token = "exec-token";
        let agent = register_test_automation(
            &state,
            "hermes-exec",
            token,
            &["read_structure", "execute_plan"],
            &[project_id],
        );

        let read_body = |node: i64| {
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestReadBody,
                serde_json::json!({ "nodeId": node }),
            )
        };

        // A request needs total control ON; with it off, filing is refused.
        let blocked = handle_automation_request(&state, read_body(node_id));
        assert!(!blocked.ok);
        assert!(blocked
            .error
            .unwrap()
            .to_lowercase()
            .contains("total control"));

        state
            .db()
            .unwrap()
            .set_mcp_full_control_enabled(true)
            .unwrap();

        // read_body files a queued request and grants nothing until approved.
        let queued = handle_automation_request(&state, read_body(node_id));
        assert!(queued.ok);
        assert_eq!(queued.result.unwrap()["status"], "queued");
        let now = Utc::now().timestamp_millis();
        assert!(!state
            .db()
            .unwrap()
            .automation_has_read_grant(agent.id, node_id, now)
            .unwrap());
        let request_id = agent_requests_pending(&state).unwrap()[0].id;
        agent_request_resolve(&state, request_id, true, ResolveInputs::default()).unwrap();
        // Approval mints the per-node grant.
        assert!(state
            .db()
            .unwrap()
            .automation_has_read_grant(agent.id, node_id, Utc::now().timestamp_millis())
            .unwrap());

        // A final-remove request for a holding entry that does not exist is refused at
        // filing — the entry must resolve so the target is concretely identified and
        // scoped (never an opaque numeric id pointed at an unknown entry).
        let del = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestPermanentDelete,
                serde_json::json!({ "entryId": 999_999 }),
            ),
        );
        assert!(!del.ok);
        assert!(del.error.unwrap().to_lowercase().contains("not found"));
        assert!(agent_requests_pending(&state).unwrap().is_empty());

        // Read-only mode refuses filing a mutation request outright.
        state.db().unwrap().set_mcp_read_only_mode(true).unwrap();
        let frozen = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestPermanentDelete,
                serde_json::json!({ "entryId": 2 }),
            ),
        );
        assert!(!frozen.ok);
        assert!(frozen.error.unwrap().to_lowercase().contains("read-only"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_read_gate_and_revoke_do_not_bypass_sensitive_policy() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let normal_project = projects
            .iter()
            .find(|project| project.name.contains("Markdown"))
            .unwrap();
        let sensitive_project = projects
            .iter()
            .find(|project| project.name.contains("Sensitive"))
            .unwrap();
        let normal_node = project_context_files(&state, normal_project.id).unwrap()[0].node_id;
        let sensitive_node = state
            .db()
            .unwrap()
            .project_nav_tree(sensitive_project.id)
            .unwrap()
            .into_iter()
            .find(|item| item.is_sensitive && item.node_id.is_some())
            .and_then(|item| item.node_id)
            .unwrap();
        let token = "read-gate-token";
        let agent = register_test_automation(
            &state,
            "Read grant agent",
            token,
            &["read_structure"],
            &[normal_project.id, sensitive_project.id],
        );

        let denied = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentReadBody,
                serde_json::json!({ "nodeId": normal_node }),
            ),
        );
        assert!(!denied.ok);

        let expires = Utc::now().timestamp_millis() + 60_000;
        state
            .db()
            .unwrap()
            .automation_grant_read(agent.id, normal_node, expires)
            .unwrap();
        let allowed = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentReadBody,
                serde_json::json!({ "nodeId": normal_node }),
            ),
        );
        assert!(allowed.ok);

        state
            .db()
            .unwrap()
            .automation_grant_read(agent.id, sensitive_node, expires)
            .unwrap();
        let sensitive = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentReadBody,
                serde_json::json!({ "nodeId": sensitive_node }),
            ),
        );
        assert!(sensitive.ok);
        let preview: FilePreview = serde_json::from_value(sensitive.result.unwrap()).unwrap();
        assert_eq!(preview.state, hangar_core::PreviewState::Blocked);
        assert!(preview.source.is_none());

        assert!(automation_revoke(&state, agent.id).unwrap());
        let revoked = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentReadBody,
                serde_json::json!({ "nodeId": normal_node }),
            ),
        );
        assert!(!revoked.ok);
        assert!(revoked.error.unwrap().contains("revoked token"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_execution_still_requires_fresh_human_confirmation() {
        let state = AppState::memory().unwrap();
        let project = projects_list(&state).unwrap().remove(0);
        let token = "execute-test-token";
        register_test_automation(
            &state,
            "Execution test agent",
            token,
            &["build_plan", "execute_plan"],
            &[project.id],
        );
        let plan = operation_plan_build(
            &state,
            project.id,
            "Verified backup review".to_string(),
            Some("balanced".to_string()),
        )
        .unwrap();
        let response = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::AgentPlanExecute,
                serde_json::json!({
                    "plan": plan,
                    "action": "backup",
                    "destinationRoot": unique_temp_dir("automation-backup"),
                    "level": "standard",
                    "allowSameVolume": false,
                    "confirmToken": "not-a-human-token"
                }),
            ),
        );
        assert!(!response.ok);
        assert!(response
            .error
            .unwrap()
            .contains("fresh mutation confirmation token"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn automation_public_registration_hashes_token_and_history_needs_project() {
        let state = AppState::memory().unwrap();
        *state.automation_endpoint.lock().unwrap() = Some(r"\\.\pipe\codehangar-test".to_string());
        let project = projects_list(&state).unwrap().remove(0);
        let credential = automation_register(
            &state,
            "Public API test".to_string(),
            vec![
                "read_structure".to_string(),
                "build_plan".to_string(),
                "history_search".to_string(),
            ],
            vec![project.id],
        )
        .unwrap();
        let stored_hash: String = state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.query_row(
                    "SELECT token_hash FROM automation_agent WHERE id = ?1",
                    [credential.agent.id],
                    |row| row.get(0),
                )
                .map_err(DbError::from)
            })
            .unwrap();
        assert_ne!(stored_hash, credential.token);
        assert_eq!(stored_hash, automation_token_hash(&credential.token));

        let plan = handle_automation_request(
            &state,
            automation_request(
                Some(&credential.token),
                hangar_agent::AgentMethod::AgentPlanBuild,
                serde_json::json!({
                    "targetNodeId": project.id,
                    "actionLabel": "Local impact review"
                }),
            ),
        );
        assert!(plan.ok);
        let plan: OperationPlan = serde_json::from_value(plan.result.unwrap()).unwrap();
        assert!(plan.read_only_preview);

        let history = handle_automation_request(
            &state,
            automation_request(
                Some(&credential.token),
                hangar_agent::AgentMethod::DeepHistorySearch,
                serde_json::json!({ "query": "local context" }),
            ),
        );
        assert!(!history.ok);
        assert!(history.error.unwrap().contains("explicit projectId"));
    }

    #[cfg(not(feature = "mutation"))]
    #[test]
    fn recovery_check_is_inert_without_mutation_feature() {
        let state = AppState::memory().unwrap();
        let pending = recovery_pending(&state).unwrap();
        assert!(!pending.enabled);
        assert!(!pending.pending);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn recovery_resolve_clears_interrupted_journal() {
        let state = AppState::memory().unwrap();
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|err| DbError::FileRead(err.to_string()))?;
                conn.execute(
                    "INSERT INTO operation(kind, status, plan_json, created_at)
                     VALUES('move_review', 'executing', '{}', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let pending = recovery_pending(&state).unwrap();
        assert!(pending.enabled);
        assert!(pending.pending);
        assert_eq!(pending.operations.len(), 1);

        let result = recovery_resolve(&state, "rollback".to_string()).unwrap();
        assert_eq!(result.recovered_operations, 1);
        assert_eq!(result.action, "rollback");

        let pending_after = recovery_pending(&state).unwrap();
        assert!(!pending_after.pending);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn recovery_guard_blocks_only_unreconciled_operation_states() {
        let state = AppState::memory().unwrap();
        let outcomes = state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|error| DbError::FileRead(error.to_string()))?;
                let mut outcomes = Vec::new();
                for status in [
                    "executing",
                    "backup_running",
                    "verifying",
                    "failed",
                    "done",
                    "rolled_back",
                ] {
                    conn.execute(
                        "INSERT INTO operation(kind, status, plan_json, created_at)
                         VALUES('quarantine', ?1, '{}', '2026-01-01T00:00:00Z')",
                        [status],
                    )?;
                    outcomes.push((status, ensure_no_pending_recovery(conn).is_err()));
                    conn.execute("DELETE FROM operation", [])?;
                }
                Ok(outcomes)
            })
            .unwrap();

        assert_eq!(
            outcomes,
            vec![
                ("executing", true),
                ("backup_running", true),
                ("verifying", true),
                ("failed", false),
                ("done", false),
                ("rolled_back", false),
            ]
        );
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn recovery_refuses_a_fake_continue_decision() {
        let state = AppState::memory().unwrap();
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|err| DbError::FileRead(err.to_string()))?;
                conn.execute(
                    "INSERT INTO operation(kind, status, plan_json, created_at)
                     VALUES('move_review', 'executing', '{}', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let error = recovery_resolve(&state, "continue".to_string()).unwrap_err();
        assert!(error.contains("only be rolled back safely"));

        let pending = recovery_pending(&state).unwrap();
        assert!(
            pending.pending,
            "a refused decision must leave the journal untouched"
        );
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn final_remove_requires_explicit_qa_gate() {
        let state = AppState::memory().unwrap();
        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");

        // OFF by default: issuing a final-remove token is refused on a fresh state, and so is the
        // final-remove command itself — defense in depth, so a bypassed UI can't run it.
        let token_error = mutation_token_issue(&state, "final_remove".to_string()).unwrap_err();
        assert!(
            token_error.to_lowercase().contains("turned off"),
            "{token_error}"
        );
        let command_error =
            mutation_final_remove_start(&state, 1, "not-a-real-token".to_string()).unwrap_err();
        assert!(
            command_error.to_lowercase().contains("turned off"),
            "{command_error}"
        );

        // Explicit opt-in makes final removal AVAILABLE again: the token issues. The command still needs
        // a verified backup + valid token (covered by the #[ignore] gate3 journey); here we only
        // prove the setting flips the availability gate.
        state.db().unwrap().set_final_remove_enabled(true).unwrap();
        assert!(
            mutation_token_issue(&state, "final_remove".to_string()).is_ok(),
            "explicit opt-in must make the final-remove token issuable"
        );
    }

    /// Sandbox QA (CI-safe, fake temp files): the full "fase fatal" journey driven by the in-app
    /// opt-in (the encrypted setting, NOT the global env var). Verified backup -> move-to-holding
    /// -> final remove is REFUSED until opted in, then SUCCEEDS — the held copy is gone, the entry
    /// is permanently deleted, and the verified backup that authorised it still survives on disk.
    /// Uses an isolated temp dir + the per-db setting, so it runs in normal CI (unlike the env-var
    /// `gate3_final_remove_journey_on_real_files` journey, which stays #[ignore]).
    #[cfg(feature = "mutation")]
    #[test]
    fn final_remove_journey_via_in_app_opt_in() {
        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-r2-optin");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        let holding_dir = temp_root.join("holding");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "sandbox final-remove fixture").unwrap();
        insert_mutation_fixture_project(&state, &project_dir, &source);
        let plan =
            operation_plan_build(&state, 90_000, "R2 in-app opt-in journey".to_string(), None)
                .unwrap();

        // Verified backup, then move the file into the holding area.
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap();
        assert!(backup.verified);
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let moved = mutation_move_start(
            &state,
            plan,
            holding_dir.to_string_lossy().to_string(),
            backup.backup_id,
            false,
            token,
        )
        .unwrap();
        assert_eq!(moved.moved, 1);
        assert!(!source.exists());
        let stored_id = mutation_activity_log(&state, Some(20))
            .unwrap()
            .stored_entries
            .iter()
            .find(|entry| entry.status == "quarantined")
            .expect("a quarantined entry should exist")
            .id;

        // Final removal is OFF by default; confirm the token is refused...
        assert!(mutation_token_issue(&state, "final_remove".to_string()).is_err());
        // ...then succeeds once opted in, and the verified backup survives the irreversible delete.
        state.db().unwrap().set_final_remove_enabled(true).unwrap();
        let token = mutation_token_issue(&state, "final_remove".to_string())
            .unwrap()
            .token;
        let removed = mutation_final_remove_start(&state, stored_id, token).unwrap();
        assert_eq!(removed.entry_id, stored_id);
        assert!(removed.freed_bytes > 0);
        let after = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(after
            .stored_entries
            .iter()
            .any(|entry| entry.id == stored_id && entry.status == "permanently_deleted"));
        assert!(
            Path::new(&backup.manifest_path).exists(),
            "the verified backup must survive the irreversible delete"
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn value_edit_snapshot_restore_and_external_change_refusal_round_trip() {
        let temp_root = unique_temp_dir("codehangar-value-edit-roundtrip");
        let project_dir = temp_root.join("project");
        let db_path = temp_root.join("data").join("codehangar.sqlite3");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let source = project_dir.join("settings.json");
        let original = "{\n  \"enabled\": false,\n  \"count\": 2\n}\n";
        std::fs::write(&source, original).unwrap();

        let state = AppState::open(&db_path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = startup_status(&state);
            if status.state == "ready" {
                break;
            }
            assert_ne!(status.state, "failed", "{}", status.message);
            assert!(
                std::time::Instant::now() < deadline,
                "database startup timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        insert_mutation_fixture_project(&state, &project_dir, &source);

        let values = editable_values(&state, 90_001).unwrap();
        let enabled = values
            .values
            .iter()
            .find(|value| value.path == "$/enabled")
            .unwrap();
        let request = hangar_core::ValueEditRequest {
            value_id: enabled.id.clone(),
            expected_source_hash: values.source_hash.clone(),
            expected_raw_value: enabled.raw_value.clone(),
            new_value: "true".to_string(),
        };
        let preview = preview_value_edit(&state, 90_001, &request).unwrap();
        let unreviewed =
            apply_reviewed_value_edit(&state, 90_001, &request, "not-the-reviewed-hash")
                .unwrap_err();
        assert!(unreviewed.contains("changed after review"));
        assert!(edit_snapshots_for_node(&state, 90_001, 20)
            .unwrap()
            .is_empty());
        let changed =
            apply_reviewed_value_edit(&state, 90_001, &request, &preview.after_hash).unwrap();
        assert!(std::fs::read_to_string(&source)
            .unwrap()
            .contains("\"enabled\": true"));
        let snapshots = edit_snapshots_for_node(&state, 90_001, 20).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, changed.snapshot_id);
        assert_eq!(snapshots[0].origin, "value");
        let ledger = project_review_ledger(&state, 90_000, Some(20)).unwrap();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].node_id, Some(90_001));
        assert_eq!(ledger[0].origin.as_deref(), Some("value"));
        assert_eq!(ledger[0].before_hash.as_deref().map(str::len), Some(64));
        assert_eq!(ledger[0].after_hash.as_deref().map(str::len), Some(64));
        assert_eq!(ledger[0].entry_hash.len(), 64);
        assert_eq!(ledger[0].change_set.files[0].path, "settings.json");

        let restored = edit_snapshot_restore(&state, changed.snapshot_id).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert_ne!(restored.safety_snapshot_id, changed.snapshot_id);
        let snapshots = edit_snapshots_for_node(&state, 90_001, 20).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].origin, "restore");
        let ledger = project_review_ledger(&state, 90_000, Some(20)).unwrap();
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger[0].origin.as_deref(), Some("restore"));
        assert_eq!(
            ledger[0].previous_entry_hash.as_deref(),
            Some(ledger[1].entry_hash.as_str())
        );
        assert!(snapshots
            .iter()
            .find(|snapshot| snapshot.id == changed.snapshot_id)
            .unwrap()
            .restored_at
            .is_some());

        let stale = editable_values(&state, 90_001).unwrap();
        let count = stale
            .values
            .iter()
            .find(|value| value.path == "$/count")
            .unwrap();
        std::fs::write(&source, "{\n  \"enabled\": false,\n  \"count\": 9\n}\n").unwrap();
        let error = apply_value_edit(
            &state,
            90_001,
            &hangar_core::ValueEditRequest {
                value_id: count.id.clone(),
                expected_source_hash: stale.source_hash.clone(),
                expected_raw_value: count.raw_value.clone(),
                new_value: "3".to_string(),
            },
        )
        .unwrap_err();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(
            edit_snapshots_for_node(&state, 90_001, 20).unwrap().len(),
            2
        );

        drop(state);
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn reviewed_text_change_is_hash_bound_and_comparable_before_restore() {
        let temp_root = unique_temp_dir("codehangar-reviewed-text-change");
        let project_dir = temp_root.join("project");
        let db_path = temp_root.join("data").join("codehangar.sqlite3");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let source = project_dir.join("settings.json");
        let original = "{\n  \"enabled\": false\n}\n";
        let proposed = "{\n  \"enabled\": true\n}\n";
        std::fs::write(&source, original).unwrap();

        let state = AppState::open(&db_path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = startup_status(&state);
            if status.state == "ready" {
                break;
            }
            assert_ne!(status.state, "failed", "{}", status.message);
            assert!(
                std::time::Instant::now() < deadline,
                "database startup timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        insert_mutation_fixture_project(&state, &project_dir, &source);

        let preview = file_edit_preview(&state, 90_001, proposed, Some(original)).unwrap();
        assert_eq!(preview.added_lines, 1);
        assert_eq!(preview.removed_lines, 1);
        assert_eq!(preview.validation.status, "passed");
        assert_ne!(preview.before_hash, preview.after_hash);

        let missing_review =
            write_reviewed_file_content(&state, 90_001, proposed, "manual", Some(original), None)
                .unwrap_err();
        assert!(missing_review.contains("review this change"));
        assert!(edit_snapshots_for_node(&state, 90_001, 20)
            .unwrap()
            .is_empty());

        let changed_review = write_reviewed_file_content(
            &state,
            90_001,
            proposed,
            "manual",
            Some(original),
            Some("not-the-reviewed-hash"),
        )
        .unwrap_err();
        assert!(changed_review.contains("changed after review"));
        let unsupported_origin = write_reviewed_file_content(
            &state,
            90_001,
            proposed,
            "ai_suggestion",
            Some(original),
            Some(&preview.after_hash),
        )
        .unwrap_err();
        assert!(unsupported_origin.contains("origin is not supported"));

        let previous = write_reviewed_file_content(
            &state,
            90_001,
            proposed,
            "manual",
            Some(original),
            Some(&preview.after_hash),
        )
        .unwrap();
        assert_eq!(previous, original);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), proposed);

        let snapshots = edit_snapshots_for_node(&state, 90_001, 20).unwrap();
        assert_eq!(snapshots.len(), 1);
        let comparison = edit_snapshot_compare(&state, snapshots[0].id).unwrap();
        assert!(!comparison.already_current);
        assert_eq!(comparison.added_lines, 1);
        assert_eq!(comparison.removed_lines, 1);

        std::fs::write(&source, "{\n  \"enabled\": null\n}\n").unwrap();
        let stale = write_reviewed_file_content(
            &state,
            90_001,
            original,
            "manual",
            Some(proposed),
            Some(blake3::hash(original.as_bytes()).to_hex().as_ref()),
        )
        .unwrap_err();
        assert!(stale.contains("changed on disk"), "{stale}");
        assert_eq!(
            edit_snapshots_for_node(&state, 90_001, 20).unwrap().len(),
            1
        );

        drop(state);
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(all(feature = "mutation", feature = "agent_automation"))]
    #[test]
    fn ai_edit_session_undo_is_scoped_to_the_selected_file() {
        let temp_root = unique_temp_dir("codehangar-ai-session-undo");
        let project_dir = temp_root.join("project");
        let db_path = temp_root.join("data").join("codehangar.sqlite3");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let source = project_dir.join("settings.json");
        let other_source = project_dir.join("other.json");
        let original = "{\"message\":\"before\"}\n";
        let first = "{\"message\":\"first\"}\n";
        let second = "{\"message\":\"second\"}\n";
        let other_original = "{\"message\":\"other before\"}\n";
        let other_changed = "{\"message\":\"other changed\"}\n";
        std::fs::write(&source, original).unwrap();
        std::fs::write(&other_source, other_original).unwrap();

        let state = AppState::open(&db_path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = startup_status(&state);
            if status.state == "ready" {
                break;
            }
            assert_ne!(status.state, "failed", "{}", status.message);
            assert!(
                std::time::Instant::now() < deadline,
                "database startup timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        insert_mutation_fixture_project(&state, &project_dir, &source);
        let other_identity = hangar_fs::inspect_path_identity(&other_source);
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count,
                                      size_apparent, size_allocated, first_seen_at, last_seen_at, present)
                     VALUES(90002, 'file', ?1, 'other.json', ?2, ?3,
                            1, 32, 32, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1)",
                    params![
                        other_source.to_string_lossy().as_ref(),
                        other_identity.volume_id.as_deref(),
                        other_identity.inode_key.as_deref()
                    ],
                )?;
                conn.execute(
                    "INSERT INTO nav_item(id, project_id, node_id, path, display_path, display_name,
                                          item_kind, priority, sort_key, fully_scanned)
                     VALUES(90002, 90000, 90002, 'other.json', 'other.json', 'other.json',
                            'file', 0, 'other.json', 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        write_file_content_with_origin(
            &state,
            90_001,
            first,
            "ai_suggestion",
            Some("ai-edit-test"),
            Some(original),
        )
        .unwrap();
        write_file_content_with_origin(
            &state,
            90_002,
            other_changed,
            "ai_suggestion",
            Some("ai-edit-test"),
            Some(other_original),
        )
        .unwrap();
        write_file_content_with_origin(
            &state,
            90_001,
            second,
            "ai_suggestion",
            Some("ai-edit-test"),
            Some(first),
        )
        .unwrap();

        let sessions = ai_edit_sessions_for_node(&state, 90_001, 20).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "ai-edit-test");
        assert_eq!(sessions[0].edit_count, 2);
        let restored_b = undo_ai_edit_session(&state, 90_002, "ai-edit-test").unwrap();
        assert_eq!(restored_b.node_id, 90_002);
        assert_eq!(
            std::fs::read_to_string(&other_source).unwrap(),
            other_original
        );
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            second,
            "undoing file B must not restore file A's earlier snapshot"
        );

        let restored_a = undo_ai_edit_session(&state, 90_001, "ai-edit-test").unwrap();
        assert_eq!(restored_a.node_id, 90_001);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(&other_source).unwrap(),
            other_original
        );

        drop(state);
        let _ = std::fs::remove_dir_all(temp_root);
    }

    // Regression: the total-control "final_remove" approval path in agent_request_resolve must
    // issue its confirm token with an action string parse_confirm_action recognizes. It once
    // passed "permanent_delete" (unrecognized), so approving a connected app's final-remove
    // request failed with "Unknown mutation confirmation action" instead of performing the gated
    // delete. Driven end-to-end through agent_request_resolve; with the QA env gate OFF the
    // resolve must stop at the QA-gate refusal (which proves the action string parsed), never the
    // confirm-action parse error. (Env is left off so this is suite-safe — it never mutates the
    // process-global CODEHANGAR_ENABLE_FINAL_REMOVE that other tests read; the env-ON full delete
    // is covered by the #[ignore] gate3_final_remove_journey_on_real_files journey.)
    #[cfg(all(feature = "mutation", feature = "agent_automation"))]
    #[test]
    fn final_remove_resolve_uses_recognized_confirm_action() {
        let state = AppState::memory().unwrap();
        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");

        // A live agent with the execute_plan scope final_remove requires (no project, so the
        // non-cross-scope project check is skipped).
        let agent = register_test_automation(
            &state,
            "claude-code",
            "tok-final-remove",
            &["execute_plan"],
            &[],
        );

        // Queue a final_remove request for a holding-area entry. The entry id is only used AFTER
        // the confirm token is issued (where the bug lived), so it need not be a real entry here.
        let request = state
            .db()
            .unwrap()
            .agent_request_create(&hangar_db::NewAgentRequest {
                agent_id: Some(agent.id),
                agent_name: agent.name.clone(),
                kind: "final_remove".to_string(),
                target_comment_id: None,
                proposed_body: None,
                detail: Some("remove the held copy".to_string()),
                target_kind: Some("holding_entry".to_string()),
                target_id: Some(1),
                project_id: None,
                payload_json: None,
                cross_scope: false,
            })
            .unwrap();

        // Final removal is OFF by default, so this exercises the gate-off refusal
        // path (we only care here that final_remove resolution does not hit the confirm-action bug).
        let err =
            agent_request_resolve(&state, request.id, true, ResolveInputs::default()).unwrap_err();
        assert!(
            !err.contains("Unknown mutation confirmation action"),
            "final_remove approval hit the confirm-action regression: {err}"
        );
        assert!(
            err.to_lowercase().contains("turned off"),
            "expected the final-remove QA-gate refusal (gate off), got: {err}"
        );
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn mutation_commands_require_token_and_journal_activity() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-api-mutation");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        let holding_dir = temp_root.join("holding");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "local mutation fixture").unwrap();

        insert_mutation_fixture_project(&state, &project_dir, &source);
        let plan = operation_plan_build(
            &state,
            90_000,
            "Future backup or holding review".to_string(),
            None,
        )
        .unwrap();

        let missing_token = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            "not-a-token".to_string(),
        )
        .unwrap_err();
        assert!(missing_token.contains("confirmation token"));

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap();
        assert!(backup.verified);
        assert_eq!(backup.item_count, 1);
        assert!(Path::new(&backup.manifest_path).exists());

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let moved = mutation_move_start(
            &state,
            plan,
            holding_dir.to_string_lossy().to_string(),
            backup.backup_id,
            false,
            token,
        )
        .unwrap();
        assert_eq!(moved.moved, 1);
        assert_eq!(moved.failed, 0);
        assert!(!source.exists());

        let activity = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(activity.enabled);
        assert!(!activity.operations.is_empty());
        assert_eq!(activity.backups.len(), 1);
        let stored = activity
            .stored_entries
            .iter()
            .find(|entry| entry.status == "quarantined")
            .expect("stored entry should be journaled");

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let restored = mutation_restore_start(&state, stored.id, token).unwrap();
        assert_eq!(restored.outcome, "restored");
        assert!(source.exists());

        let final_activity = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(final_activity
            .stored_entries
            .iter()
            .any(|entry| entry.id == stored.id && entry.status == "restored"));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(all(feature = "mutation", windows))]
    #[test]
    fn controlled_project_check_is_approved_bounded_restorable_and_manifest_bound() {
        let temp_root = unique_temp_dir("codehangar-controlled-check");
        let project_dir = temp_root.join("project");
        let db_path = temp_root.join("data").join("codehangar.sqlite3");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let source = project_dir.join("settings.json");
        let original = "{\n  \"enabled\": false\n}\n";
        std::fs::write(&source, original).unwrap();
        std::fs::write(
            project_dir.join("package.json"),
            r#"{
  "name": "codehangar-controlled-check-fixture",
  "private": true,
  "scripts": {
    "test": "node -e \"process.stdout.write('controlled-ok')\""
  }
}
"#,
        )
        .unwrap();

        let state = AppState::open(&db_path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = startup_status(&state);
            if status.state == "ready" {
                break;
            }
            assert_ne!(status.state, "failed", "{}", status.message);
            assert!(
                std::time::Instant::now() < deadline,
                "database startup timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        insert_mutation_fixture_project(&state, &project_dir, &source);

        let static_report = static_correction_check(&state, 90_001).unwrap();
        assert_eq!(static_report.status, "passed");
        assert!(!static_report.executed_project_code);

        let detected = project_checks_detect(&state, 90_000).unwrap();
        let npm_test = detected
            .iter()
            .find(|check| check.id == "npm:test")
            .expect("npm test should be detected")
            .clone();
        assert!(!npm_test.approved);
        assert_eq!(npm_test.command_label, "npm test");
        assert!(npm_test.risk_disclosure.contains("not a sandbox"));

        let approved =
            project_check_approve(&state, 90_000, &npm_test.id, &npm_test.fingerprint).unwrap();
        assert!(approved.approved);
        assert!(approved.approved_at.is_some());

        write_file_content(&state, 90_001, "{\n  \"enabled\": true\n}\n").unwrap();
        let run =
            project_check_run(&state, 90_000, 90_001, &npm_test.id, &npm_test.fingerprint).unwrap();
        assert_eq!(run.status, "passed", "{}", run.stderr);
        assert_eq!(run.exit_code, Some(0));
        assert!(run.stdout.contains("controlled-ok"));
        assert!(run.rollback_available);
        assert!(run.rollback_snapshot_id.is_some());
        assert!(run.limits_summary.contains("120s wall timeout"));

        edit_snapshot_restore(&state, run.rollback_snapshot_id.unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);

        std::fs::write(
            project_dir.join("package.json"),
            r#"{
  "name": "codehangar-controlled-check-fixture",
  "private": true,
  "scripts": {
    "test": "node -e \"process.stdout.write('manifest-changed')\""
  }
}
"#,
        )
        .unwrap();
        let changed = project_checks_detect(&state, 90_000)
            .unwrap()
            .into_iter()
            .find(|check| check.id == "npm:test")
            .unwrap();
        assert_ne!(changed.fingerprint, npm_test.fingerprint);
        assert!(!changed.approved);
        let error = project_check_run(&state, 90_000, 90_001, &npm_test.id, &npm_test.fingerprint)
            .unwrap_err();
        assert!(error.contains("manifest changed"), "{error}");
        assert!(project_check_revoke(&state, 90_000, &npm_test.id).unwrap());

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn protected_and_reparse_items_reach_confirmation_and_backup() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-protected-mutation");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        let sensitive = project_dir.join(".env");
        let reparse = project_dir.join("linked-outside");
        std::fs::write(&source, "local mutation fixture").unwrap();
        std::fs::write(&sensitive, "TOKEN=local-only").unwrap();
        insert_mutation_fixture_project(&state, &project_dir, &source);

        let sensitive_identity = hangar_fs::inspect_path_identity(&sensitive);
        let sensitive_path = sensitive.to_string_lossy().to_string();
        let reparse_path = reparse.to_string_lossy().to_string();
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO node(id, kind, path, name, protected_level, volume_id, inode_key,
                                      link_count, size_apparent, size_allocated, first_seen_at,
                                      last_seen_at, present)
                     VALUES(90002, 'file', ?1, '.env', 'no_preview', ?2, ?3, 1, ?4, ?4,
                            '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1)",
                    params![
                        sensitive_path,
                        sensitive_identity.volume_id.as_deref(),
                        sensitive_identity.inode_key.as_deref(),
                        sensitive_identity.size_apparent.unwrap_or(0) as i64
                    ],
                )?;
                conn.execute(
                    "INSERT INTO nav_item(id, project_id, node_id, path, display_path, display_name,
                                          item_kind, priority, sort_key, is_sensitive,
                                          protected_level, fully_scanned)
                     VALUES(90002, 90000, 90002, '.env', '.env', '.env', 'file', 0, '.env',
                            1, 'no_preview', 1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO node(id, kind, path, name, is_reparse, reparse_kind,
                                      size_apparent, first_seen_at, last_seen_at, present)
                     VALUES(90003, 'directory', ?1, 'linked-outside', 1, 'junction', 0,
                            '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1)",
                    params![reparse_path],
                )?;
                conn.execute(
                    "INSERT INTO nav_item(id, project_id, node_id, path, display_path, display_name,
                                          item_kind, priority, sort_key, fully_scanned)
                     VALUES(90003, 90000, 90003, 'linked-outside', 'linked-outside',
                            'linked-outside', 'directory', 10, 'linked-outside', 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let plan = operation_plan_build(
            &state,
            90_000,
            "Protected mutation regression".to_string(),
            None,
        )
        .unwrap();
        let protected = mutation_preview_protected(&state, plan.clone()).unwrap();
        assert_eq!(protected.protected, vec![sensitive_path]);
        assert_eq!(protected.reparse, vec![reparse_path]);

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan,
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            true,
            token,
        )
        .unwrap();
        assert!(backup.verified);
        assert_eq!(backup.item_count, 2);
        assert_eq!(
            std::fs::read_to_string(backup_dir.join(".env")).unwrap(),
            "TOKEN=local-only"
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    /// Gate 3 adversarial QA on real files: the full "fase fatal" journey through the real
    /// command surface the GUI uses — move-without-backup refused, backup -> move, final
    /// remove refused without the QA opt-in, then (with the opt-in) a SUCCESSFUL final
    /// remove whose verified backup still survives on disk. Ignored by default because it
    /// sets the process-global CODEHANGAR_ENABLE_FINAL_REMOVE env var and touches the real
    /// filesystem; run alone with `--ignored --test-threads=1`.
    #[cfg(feature = "mutation")]
    #[test]
    #[ignore = "real-machine Gate 3 QA: sets CODEHANGAR_ENABLE_FINAL_REMOVE; run with --ignored --test-threads=1"]
    fn gate3_final_remove_journey_on_real_files() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-qa-final");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        let holding_dir = temp_root.join("holding");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "local mutation fixture").unwrap();

        insert_mutation_fixture_project(&state, &project_dir, &source);
        let plan =
            operation_plan_build(&state, 90_000, "QA final-remove journey".to_string(), None)
                .unwrap();

        // (A) Move WITHOUT a verified backup is refused; the source is untouched.
        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let refused = mutation_move_start(
            &state,
            plan.clone(),
            holding_dir.to_string_lossy().to_string(),
            0,
            false,
            token,
        )
        .unwrap_err();
        assert!(
            refused.to_lowercase().contains("backup"),
            "move without a backup must be refused: {refused}"
        );
        assert!(source.exists(), "a refused move must not touch the source");

        // (B) Verified backup, then move the file into the holding area.
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap();
        assert!(backup.verified);
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let moved = mutation_move_start(
            &state,
            plan,
            holding_dir.to_string_lossy().to_string(),
            backup.backup_id,
            false,
            token,
        )
        .unwrap();
        assert_eq!(moved.moved, 1);
        assert!(!source.exists());
        let stored_id = mutation_activity_log(&state, Some(20))
            .unwrap()
            .stored_entries
            .iter()
            .find(|entry| entry.status == "quarantined")
            .expect("a quarantined entry should exist")
            .id;

        // (C) Final remove is refused until the user opts in (the token is gated). Keep the env var
        // absent here to exercise the refusal.
        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");
        let gated = mutation_token_issue(&state, "final_remove".to_string()).unwrap_err();
        assert!(
            gated.to_lowercase().contains("turned off"),
            "final remove must be gated off once disabled: {gated}"
        );

        // (D) Enable the supervised QA opt-in; the final remove now succeeds.
        std::env::set_var("CODEHANGAR_ENABLE_FINAL_REMOVE", "1");
        let token = mutation_token_issue(&state, "final_remove".to_string())
            .unwrap()
            .token;
        let removed = mutation_final_remove_start(&state, stored_id, token).unwrap();
        assert_eq!(removed.entry_id, stored_id);
        assert!(removed.freed_bytes > 0);

        // (E) The held copy is gone, the entry is permanently deleted, but the verified
        // backup that authorised it still survives on disk (the recoverable safety net).
        let after = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(after
            .stored_entries
            .iter()
            .any(|entry| entry.id == stored_id && entry.status == "permanently_deleted"));
        assert!(
            Path::new(&backup.manifest_path).exists(),
            "the verified backup must survive the irreversible delete"
        );

        std::env::remove_var("CODEHANGAR_ENABLE_FINAL_REMOVE");
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn mutation_restore_conflict_can_restore_to_chosen_folder() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-api-restore-elsewhere");
        let project_dir = temp_root.join("project");
        let holding_dir = temp_root.join("holding");
        let restore_dir = temp_root.join("restore-target");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "local mutation fixture").unwrap();

        insert_mutation_fixture_project(&state, &project_dir, &source);
        let plan = operation_plan_build(&state, 90_000, "Future holding review".to_string(), None)
            .unwrap();
        // Gate 3: a verified backup is required before moving to the holding area.
        let backup_dir = temp_root.join("backup");
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap();
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let moved = mutation_move_start(
            &state,
            plan,
            holding_dir.to_string_lossy().to_string(),
            backup.backup_id,
            false,
            token,
        )
        .unwrap();
        assert_eq!(moved.moved, 1);

        let activity = mutation_activity_log(&state, Some(20)).unwrap();
        let stored = activity
            .stored_entries
            .iter()
            .find(|entry| entry.status == "quarantined")
            .expect("stored entry should be journaled");

        // The recursive move emptied and removed the project dir; recreate it with a
        // new occupant so the restore must report a conflict at the original path.
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(&source, "new occupant").unwrap();
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let conflict = mutation_restore_start(&state, stored.id, token).unwrap();
        assert_eq!(conflict.outcome, "conflict");
        assert_eq!(
            conflict.conflict_path.as_deref(),
            Some(source.to_str().unwrap())
        );

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let restored = mutation_restore_to_folder_start(
            &state,
            stored.id,
            restore_dir.to_string_lossy().to_string(),
            token,
        )
        .unwrap();
        assert_eq!(restored.outcome, "restored_elsewhere");
        let restored_path = restored.restored_path.expect("restored path");
        assert!(Path::new(&restored_path).exists());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "new occupant");

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn mutation_revalidation_rejects_changed_file_identity() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-api-identity");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "local mutation fixture").unwrap();
        let identity = hangar_fs::inspect_path_identity(&source);
        if identity.volume_id.is_none() || identity.inode_key.is_none() {
            let _ = std::fs::remove_dir_all(temp_root);
            return;
        }

        insert_mutation_fixture_project(&state, &project_dir, &source);
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "UPDATE node SET volume_id = 'wrong-volume', inode_key = 'wrong-inode'
                     WHERE id = 90001",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let plan = operation_plan_build(
            &state,
            90_000,
            "Future backup or holding review".to_string(),
            None,
        )
        .unwrap();
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let error = mutation_backup_start(
            &state,
            plan,
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap_err();
        assert!(error.contains("identity changed"));
        assert!(source.exists());

        let _ = std::fs::remove_dir_all(temp_root);
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{nonce}"))
    }

    #[cfg(feature = "mutation")]
    fn insert_mutation_fixture_project(state: &AppState, project_dir: &Path, source: &Path) {
        let now = "2026-01-01T00:00:00Z";
        let source_path = source.to_string_lossy();
        let project_path = project_dir.to_string_lossy();
        let identity = hangar_fs::inspect_path_identity(source);
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO node(id, kind, path, name, first_seen_at, last_seen_at, present)
                     VALUES(90000, 'project', ?1, 'mutation-fixture', ?2, ?2, 1)",
                    params![project_path.as_ref(), now],
                )?;
                conn.execute(
                    "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count,
                                      size_apparent, size_allocated, first_seen_at, last_seen_at, present)
                     VALUES(90001, 'file', ?1, 'artifact.txt', ?2, ?3,
                            1, 22, 22, ?4, ?4, 1)",
                    params![
                        source_path.as_ref(),
                        identity.volume_id.as_deref(),
                        identity.inode_key.as_deref(),
                        now
                    ],
                )?;
                conn.execute(
                    "INSERT INTO nav_item(id, project_id, node_id, path, display_path, display_name,
                                          item_kind, priority, sort_key, fully_scanned)
                     VALUES(90001, 90000, 90001, 'artifact.txt', 'artifact.txt', 'artifact.txt',
                            'file', 0, 'artifact.txt', 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn exposes_dashboard_summary() {
        let state = AppState::memory().unwrap();
        let dashboard = dashboard_summary(&state).unwrap();
        assert!(dashboard.total_projects > 0);
        assert!(dashboard.context_files > 0);
    }

    #[test]
    fn watcher_marks_an_empty_root_as_empty() {
        let state = AppState::memory().unwrap();
        let root_dir = unique_temp_dir("codehangar-watch-root");
        std::fs::create_dir_all(&root_dir).unwrap();
        let root = roots_add(&state, root_dir.to_string_lossy().to_string()).unwrap();

        let status = watcher_status(&state, None, None).unwrap();
        assert_eq!(status.poll_interval_ms, 30_000);
        let root_status = status
            .projects
            .iter()
            .find(|candidate| candidate.scan_root_id == root.id)
            .expect("new root should be watched");

        assert_eq!(root_status.state, "empty");
        assert_eq!(root_status.reason, "This project folder is empty.");

        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn watcher_keeps_a_non_empty_new_root_as_needing_scan() {
        let state = AppState::memory().unwrap();
        let root_dir = unique_temp_dir("codehangar-watch-root-with-file");
        std::fs::create_dir_all(&root_dir).unwrap();
        std::fs::write(root_dir.join("README.md"), "# Pending scan").unwrap();
        let root = roots_add(&state, root_dir.to_string_lossy().to_string()).unwrap();

        let status = watcher_status(&state, None, None).unwrap();
        let root_status = status
            .projects
            .iter()
            .find(|candidate| candidate.scan_root_id == root.id)
            .expect("new root should be watched");

        assert_eq!(root_status.state, "needs_scan");
        assert!(status.stale_projects >= 1);

        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn focused_watcher_reports_current_fixture_file_state() {
        let state = AppState::memory().unwrap();
        let project = projects_list(&state)
            .unwrap()
            .into_iter()
            .find(|project| project.context_count > 0)
            .expect("fixture project with context");
        let context = project_context_files(&state, project.id)
            .unwrap()
            .into_iter()
            .next()
            .expect("fixture context file");

        let status = watcher_status(&state, Some(project.id), Some(context.node_id)).unwrap();
        let focused = status.focused.expect("focused watcher status");
        let current = focused.current_node.expect("current node status");

        assert_eq!(focused.project_id, project.id);
        assert_eq!(current.node_id, context.node_id);
        assert_eq!(current.state, "missing");
    }

    #[test]
    fn exposes_preview_plan_and_risk_report() {
        let state = AppState::memory().unwrap();
        let plan =
            operation_plan_build(&state, 1, "Future cleanup review".to_string(), None).unwrap();
        assert!(plan.read_only_preview);
        assert!(plan.external_services_unaffected);
        assert_eq!(plan.schema, "operation_plan/1");

        let report = risk_report_build(&state, plan, None).unwrap();
        assert!(report.read_only_preview);
        assert!(report
            .caveats
            .iter()
            .any(|caveat| caveat.contains("Preview only")));
    }

    #[test]
    fn preview_plan_job_completes_with_report() {
        let state = AppState::memory().unwrap();
        let job_id =
            operation_plan_start(&state, 1, "Future cleanup review".to_string(), None).unwrap();

        let status = (0..20)
            .find_map(|_| {
                let status = operation_plan_status(&state, job_id.clone()).unwrap();
                if matches!(status.state.as_str(), "completed" | "failed" | "cancelled") {
                    Some(status)
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    None
                }
            })
            .expect("preview plan job did not finish");

        assert_eq!(status.state, "completed");
        assert!(status
            .plan
            .as_ref()
            .is_some_and(|plan| plan.read_only_preview));
        assert!(status.report.is_some());
    }

    #[test]
    fn refuses_root_disable_or_unregister_during_active_scan() {
        let state = AppState::memory().unwrap();
        let root = state
            .db()
            .unwrap()
            .roots_add("fixture://guarded-root")
            .unwrap();
        let job_id = state.jobs.create_running_for_roots(
            "Scanning guarded root.",
            vec![root.id],
            vec![root.path.clone()],
        );

        let disable_error = roots_set_enabled(&state, root.id, false).unwrap_err();
        assert!(disable_error.contains("active scan"));

        let unregister_error = roots_unregister(&state, root.id).unwrap_err();
        assert!(unregister_error.contains("active scan"));

        let status = scan_status(&state, job_id).unwrap();
        assert_eq!(status.root_ids, vec![root.id]);
        assert_eq!(status.root_paths, vec![root.path]);
    }

    #[test]
    fn codex_rollout_jsonl_is_detected_by_shape() {
        assert!(is_codex_rollout_jsonl(Path::new(
            r"C:\Users\user\.codex\sessions\2026\05\08\rollout-2026-05-08T00-00-00-019e04cd.jsonl"
        )));
        // archived_sessions also counts.
        assert!(is_codex_rollout_jsonl(Path::new(
            r"C:\Users\user\.codex\archived_sessions\2026\05\rollout-abc.jsonl"
        )));
        // POSIX-style path (a WSL `.codex`).
        assert!(is_codex_rollout_jsonl(Path::new(
            "/home/me/.codex/sessions/2026/05/rollout-x.jsonl"
        )));
        // Wrong extension, wrong prefix, or not under .codex/sessions -> not a rollout.
        assert!(!is_codex_rollout_jsonl(Path::new(
            r"C:\Users\user\.codex\sessions\2026\rollout-x.json"
        )));
        assert!(!is_codex_rollout_jsonl(Path::new(
            r"C:\Users\user\.codex\sessions\2026\transcript.jsonl"
        )));
        assert!(!is_codex_rollout_jsonl(Path::new(
            r"C:\Users\user\.claude\sessions\rollout-x.jsonl"
        )));
    }

    #[test]
    fn oversized_codex_rollout_window_reads_tail_newest_first() {
        // Synthesize a rollout larger than the rollout cap whose FIRST lines are
        // encrypted-blob noise and whose LAST line is the newest readable turn. The
        // head-read would only see the noise; the tail-read must surface the newest
        // line and report `truncated = true`.
        let dir = unique_temp_dir("codex-rollout-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-2026-05-08T00-00-00-synthetic.jsonl");

        let newest = r#"{"type":"response_item","payload":{"text":"NEWEST_TURN_MARKER the latest conversation"}}"#;
        let mut contents = String::new();
        // Pad well past CODEX_ROLLOUT_TAIL_MAX_BYTES with blob-only lines.
        let blob = "x".repeat(2048);
        let noise_line = format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"encrypted_content\":\"gAAAAA{blob}\"}}}}\n"
        );
        while contents.len() < (CODEX_ROLLOUT_TAIL_MAX_BYTES as usize) + (512 * 1024) {
            contents.push_str(&noise_line);
        }
        contents.push_str(newest);
        contents.push('\n');
        std::fs::write(&file, &contents).unwrap();

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        assert!(size_bytes > CODEX_ROLLOUT_TAIL_MAX_BYTES);

        let (buffer, truncated) =
            read_session_preview_window(&file, true, true, size_bytes).unwrap();
        let text = String::from_utf8_lossy(&buffer);

        assert!(truncated, "an oversized rollout tail-read is truncated");
        assert!(
            text.contains("NEWEST_TURN_MARKER"),
            "the newest turn must be in the tail window"
        );
        // The window is bounded by the cap (after dropping the partial first line).
        assert!((buffer.len() as u64) <= CODEX_ROLLOUT_TAIL_MAX_BYTES);
        // The leading partial line was dropped, so the buffer starts on a clean line.
        assert!(
            text.starts_with("{\"type\":\"response_item\""),
            "tail window starts on a whole line, got: {:?}",
            &text[..text.len().min(40)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn progressive_jsonl_window_expands_until_full_content_is_available() {
        let dir = unique_temp_dir("session-progressive-window");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude-session.jsonl");
        let oldest = r#"{"type":"user","message":{"role":"user","content":"OLDEST_TURN"}}"#;
        let newest =
            r#"{"type":"assistant","message":{"role":"assistant","content":"NEWEST_TURN"}}"#;
        let filler = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":\"{}\"}}}}\n",
            "x".repeat(2048)
        );
        let mut contents = format!("{oldest}\n");
        while contents.len() < 320 * 1024 {
            contents.push_str(&filler);
        }
        contents.push_str(newest);
        contents.push('\n');
        std::fs::write(&file, &contents).unwrap();
        let size_bytes = std::fs::metadata(&file).unwrap().len();

        let (initial, initial_truncated) =
            read_session_preview_window_with_limit(&file, true, size_bytes, 64 * 1024).unwrap();
        let (expanded, expanded_truncated) =
            read_session_preview_window_with_limit(&file, true, size_bytes, 128 * 1024).unwrap();
        let (full, full_truncated) =
            read_session_preview_window_with_limit(&file, true, size_bytes, size_bytes).unwrap();

        assert!(initial_truncated);
        assert!(expanded_truncated);
        assert!(expanded.len() > initial.len());
        assert!(String::from_utf8_lossy(&initial).contains("NEWEST_TURN"));
        assert!(!String::from_utf8_lossy(&expanded).contains("OLDEST_TURN"));
        assert!(!full_truncated);
        assert!(String::from_utf8_lossy(&full).contains("OLDEST_TURN"));
        assert!(String::from_utf8_lossy(&full).contains("NEWEST_TURN"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "large generated fixture; run with scripts/acceptance-v011.ps1 -Lane DataStress"]
    fn huge_generated_session_progressively_loads_and_opens_fully() {
        use std::ffi::OsString;
        use std::io::{BufWriter, Write};

        struct EnvVarGuard {
            name: &'static str,
            previous: Option<OsString>,
        }

        impl EnvVarGuard {
            fn set(name: &'static str, value: &Path) -> Self {
                let previous = std::env::var_os(name);
                std::env::set_var(name, value);
                Self { name, previous }
            }
        }

        impl Drop for EnvVarGuard {
            fn drop(&mut self) {
                if let Some(previous) = self.previous.as_ref() {
                    std::env::set_var(self.name, previous);
                } else {
                    std::env::remove_var(self.name);
                }
            }
        }

        const TURN_COUNT: usize = 12_000;
        let dir = unique_temp_dir("session-public-stress");
        let codex_home = dir.join("codex-home");
        let sessions = codex_home.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("large-generic-session.jsonl");
        let env_guard = EnvVarGuard::set("CODEX_HOME", &codex_home);

        let output = std::fs::File::create(&file).unwrap();
        let mut writer = BufWriter::new(output);
        let padding = "x".repeat(1_800);
        for index in 0..TURN_COUNT {
            let role = if index % 2 == 0 { "user" } else { "assistant" };
            let record = serde_json::json!({
                "type": role,
                "message": {
                    "role": role,
                    "content": format!("TURN_{index:05} {padding}")
                }
            });
            serde_json::to_writer(&mut writer, &record).unwrap();
            writer.write_all(b"\n").unwrap();
            if index > 0 && index % 2_000 == 0 {
                writer.write_all(b"{BROKEN_RECORD\n").unwrap();
            }
        }
        writer
            .write_all(b"{\"type\":\"assistant\",\"message\":")
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        assert!(
            size_bytes > 20 * 1024 * 1024,
            "fixture is only {size_bytes} bytes"
        );
        let path = file.to_string_lossy().to_string();
        let started = Instant::now();

        let initial = session_preview_window(path.clone(), false, None, false).unwrap();
        assert!(initial.truncated);
        assert!(initial.source_truncated);
        assert_eq!(initial.preview_limit_bytes, SESSION_PREVIEW_MAX_BYTES);
        assert!(initial.text.contains("TURN_11999"));
        assert!(!initial.text.contains("TURN_00000"));

        let expanded =
            session_preview_window(path.clone(), false, Some(1024 * 1024), false).unwrap();
        assert!(expanded.truncated);
        assert!(expanded.source_truncated);
        assert_eq!(expanded.preview_limit_bytes, 1024 * 1024);
        assert!(expanded.text.len() > initial.text.len());
        assert!(expanded.text.contains("TURN_11999"));
        assert!(!expanded.text.contains("TURN_00000"));

        let full = session_preview_window(path, false, None, true).unwrap();
        assert!(!full.truncated);
        assert!(full.source_truncated);
        assert_eq!(full.preview_limit_bytes, size_bytes);
        assert!(
            full.text.len() < 300 * 1024,
            "raw Source view became unbounded"
        );
        let rendered = full
            .rendered_text
            .as_deref()
            .expect("full JSONL request should stream a readable transcript");
        assert!(rendered.contains("TURN_00000"));
        assert!(rendered.contains("TURN_11999"));
        assert!(!rendered.contains("BROKEN_RECORD"));

        let elapsed = started.elapsed();
        assert!(
            elapsed.as_secs() < 60,
            "progressive and full session reads exceeded 60 s: {elapsed:?}"
        );
        println!(
            "session stress: {TURN_COUNT} turns, {size_bytes} source bytes, {} readable bytes in {:?}",
            rendered.len(),
            elapsed
        );

        drop(env_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_full_session_limit_is_the_only_unbounded_request() {
        let size = 20 * 1024 * 1024;
        assert_eq!(
            requested_session_preview_limit(size, SESSION_PREVIEW_MAX_BYTES, None, false),
            SESSION_PREVIEW_MAX_BYTES
        );
        assert_eq!(
            requested_session_preview_limit(
                size,
                SESSION_PREVIEW_MAX_BYTES,
                Some(2 * 1024 * 1024),
                false
            ),
            2 * 1024 * 1024
        );
        assert_eq!(
            requested_session_preview_limit(
                size,
                SESSION_PREVIEW_MAX_BYTES,
                Some(2 * 1024 * 1024),
                true
            ),
            size
        );
    }

    #[test]
    fn expanded_codex_window_keeps_every_readable_turn_in_the_requested_slice() {
        let mut lines = vec![
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"FULL_USER"}}"#
                .to_string(),
        ];
        for index in 0..140 {
            lines.push(format!(
                "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"UPDATE_{index}\"}}}}"
            ));
        }
        lines.push(
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"FALLBACK_COPY"}]}}"#.to_string(),
        );
        let rendered = expanded_codex_rendered_window(lines.join("\n").as_bytes())
            .expect("expanded conversation should render");

        assert!(rendered.contains("FULL_USER"));
        assert!(rendered.contains("UPDATE_0"));
        assert!(rendered.contains("UPDATE_139"));
        assert!(!rendered.contains("FALLBACK_COPY"));
    }

    #[test]
    fn full_generic_jsonl_stream_keeps_conversation_and_drops_heavy_internal_payloads() {
        let dir = unique_temp_dir("session-full-stream");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude-session.jsonl");
        let tool_blob = "x".repeat(512 * 1024);
        let lines = [
            serde_json::json!({"type":"summary","summary":"internal summary"}).to_string(),
            serde_json::json!({
                "type":"assistant",
                "message": {"role":"assistant","content":[
                    {"type":"thinking","thinking":"private reasoning"},
                    {"type":"text","text":"Readable answer"},
                    {"type":"tool_use","name":"PowerShell","input":{"blob":tool_blob}}
                ]}
            })
            .to_string(),
            serde_json::json!({
                "type":"user",
                "message":{"role":"user","content":[
                    {"type":"tool_result","content":"huge tool output"}
                ]}
            })
            .to_string(),
            serde_json::json!({
                "type":"user",
                "message":{"role":"user","content":"Final human request"}
            })
            .to_string(),
        ];
        std::fs::write(&file, lines.join("\n")).unwrap();

        let rendered = read_full_rendered_jsonl(&file, false)
            .unwrap()
            .expect("readable conversation should be streamed");

        assert!(rendered.contains("Readable answer"));
        assert!(rendered.contains("↳ used PowerShell"));
        assert!(rendered.contains("Final human request"));
        assert!(!rendered.contains("private reasoning"));
        assert!(!rendered.contains("huge tool output"));
        assert!(!rendered.contains(&"x".repeat(1024)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_rendered_window_skips_large_tool_output_and_recovers_turns() {
        let dir = unique_temp_dir("codex-readable-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-readable.jsonl");
        let user = r#"{"type":"event_msg","payload":{"type":"user_message","message":"RENDERED_USER_MARKER"}}"#;
        let assistant = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"RENDERED_ASSISTANT_MARKER"}}"#;
        let blob = "A".repeat(32 * 1024);
        let tool = format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"custom_tool_call_output\",\"output\":\"{blob}\"}}}}\n"
        );
        let mut contents = format!("{user}\n{assistant}\n");
        while contents.len() < 2 * 1024 * 1024 {
            contents.push_str(&tool);
        }
        std::fs::write(&file, &contents).unwrap();

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        let (raw_window, _) = read_session_preview_window(&file, true, true, size_bytes).unwrap();
        assert!(!String::from_utf8_lossy(&raw_window).contains("RENDERED_USER_MARKER"));

        let rendered = read_codex_rendered_window(&file, size_bytes)
            .unwrap()
            .expect("readable turns should be recovered");
        assert!(rendered.contains("RENDERED_USER_MARKER"));
        assert!(rendered.contains("RENDERED_ASSISTANT_MARKER"));
        assert!(!rendered.contains("custom_tool_call_output"));
        assert!(rendered.len() <= CODEX_ROLLOUT_RENDER_MAX_BYTES);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_rendered_window_recovers_distant_user_context_before_recent_updates() {
        use std::io::{Seek, Write};

        let dir = unique_temp_dir("codex-distant-user-context");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-distant-context.jsonl");
        let user = r#"{"type":"event_msg","payload":{"type":"user_message","message":"DISTANT_USER_CONTEXT"}}"#;
        let mut handle = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&file)
            .unwrap();
        writeln!(handle, "{user}").unwrap();

        // Keep the human request outside the normal 32 MiB rendered tail without
        // physically filling the test file: the zero range is sparse on supported
        // filesystems and represents high-volume screenshot/tool output.
        let tail_start = CODEX_ROLLOUT_RENDER_SCAN_MAX_BYTES + (2 * 1024 * 1024);
        handle.set_len(tail_start).unwrap();
        handle.seek(std::io::SeekFrom::Start(tail_start)).unwrap();
        writeln!(handle).unwrap();
        // A fallback response_item user record must not count as context once the
        // selected window also contains event_msg records: the frontend will render
        // the event stream and intentionally discard this alternate copy.
        writeln!(
            handle,
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"FALLBACK_USER\"}}]}}}}"
        )
        .unwrap();
        for index in 0..(CODEX_ROLLOUT_RENDER_MAX_LINES + 12) {
            writeln!(
                handle,
                "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"RECENT_UPDATE_{index}\"}}}}"
            )
            .unwrap();
        }
        handle.flush().unwrap();
        drop(handle);

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        let rendered = read_codex_rendered_window(&file, size_bytes)
            .unwrap()
            .expect("distant human context and recent updates should render");

        assert!(rendered.contains("DISTANT_USER_CONTEXT"));
        assert!(rendered.contains("session_gap"));
        assert!(rendered.contains("RECENT_UPDATE_107"));
        assert!(rendered.len() <= CODEX_ROLLOUT_RENDER_MAX_BYTES);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_rendered_window_starts_at_first_user_in_selected_tail() {
        let dir = unique_temp_dir("codex-trim-contextless-updates");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-trim-contextless-updates.jsonl");
        let contents = [
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"ORPHAN_UPDATE_1"}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"ORPHAN_UPDATE_2"}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"CURRENT_REQUEST"}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"CURRENT_REPLY"}}"#,
        ]
        .join("\n");
        std::fs::write(&file, contents).unwrap();

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        let rendered = read_codex_rendered_window(&file, size_bytes)
            .unwrap()
            .expect("the selected user turn and reply should render");

        assert!(!rendered.contains("ORPHAN_UPDATE"));
        assert!(rendered.starts_with(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"CURRENT_REQUEST"}}"#
        ));
        assert!(rendered.contains("CURRENT_REPLY"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_rendered_window_recovers_event_user_when_recent_tail_is_tool_only() {
        use std::io::{Seek, Write};

        let dir = unique_temp_dir("codex-tool-only-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-tool-only-tail.jsonl");
        let mut handle = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&file)
            .unwrap();
        writeln!(handle, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"ONLY_HUMAN_CONTEXT\"}}}}").unwrap();
        let tail_start = CODEX_ROLLOUT_RENDER_SCAN_MAX_BYTES + (1024 * 1024);
        handle.set_len(tail_start).unwrap();
        handle.seek(std::io::SeekFrom::Start(tail_start)).unwrap();
        writeln!(handle).unwrap();
        writeln!(handle, "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"custom_tool_call_output\",\"output\":\"tool noise\"}}}}").unwrap();
        handle.flush().unwrap();
        drop(handle);

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        let rendered = read_codex_rendered_window(&file, size_bytes)
            .unwrap()
            .expect("event user should be recovered without recent readable turns");
        assert!(rendered.contains("ONLY_HUMAN_CONTEXT"));
        assert!(!rendered.contains("custom_tool_call_output"));
        assert!(!rendered.contains("session_gap"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_rollout_window_reads_head_untruncated() {
        // A rollout below the cap keeps the original head-read and is not truncated.
        let dir = unique_temp_dir("codex-rollout-head");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rollout-small.jsonl");
        let body = "{\"type\":\"response_item\",\"payload\":{\"text\":\"only turn\"}}\n";
        std::fs::write(&file, body).unwrap();
        let size_bytes = std::fs::metadata(&file).unwrap().len();

        let (buffer, truncated) =
            read_session_preview_window(&file, true, true, size_bytes).unwrap();
        assert!(!truncated, "a small rollout is not truncated");
        assert_eq!(String::from_utf8_lossy(&buffer), body);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_claude_jsonl_window_reads_tail_newest_first() {
        // A multi-MB Claude-style transcript (append-ordered, NOT a Codex rollout):
        // the oldest lines fill the head, the newest exchange is the LAST line. The
        // old head-read showed only the oldest fraction; the generalized tail-read
        // must surface the newest line, stay within the standard cap, and start on
        // a whole line.
        let dir = unique_temp_dir("claude-jsonl-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("18b6d6cf-55da-4775-bdf5-0309806a44bf.jsonl");

        let newest =
            r#"{"type":"user","message":{"content":"NEWEST_CLAUDE_TURN the latest exchange"}}"#;
        let mut contents = String::new();
        let filler = "y".repeat(2048);
        let old_line =
            format!("{{\"type\":\"assistant\",\"message\":{{\"content\":\"{filler}\"}}}}\n");
        while contents.len() < 3 * 1024 * 1024 {
            contents.push_str(&old_line);
        }
        contents.push_str(newest);
        contents.push('\n');
        std::fs::write(&file, &contents).unwrap();

        let size_bytes = std::fs::metadata(&file).unwrap().len();
        assert!(size_bytes > SESSION_PREVIEW_MAX_BYTES);

        let (buffer, truncated) =
            read_session_preview_window(&file, false, true, size_bytes).unwrap();
        let text = String::from_utf8_lossy(&buffer);

        assert!(truncated, "an oversized transcript tail-read is truncated");
        assert!(
            text.contains("NEWEST_CLAUDE_TURN"),
            "the newest exchange must be in the tail window"
        );
        // Non-rollouts keep the STANDARD cap (after dropping the partial first line).
        assert!((buffer.len() as u64) <= SESSION_PREVIEW_MAX_BYTES);
        assert!(
            text.starts_with("{\"type\":\"assistant\""),
            "tail window starts on a whole line, got: {:?}",
            &text[..text.len().min(40)]
        );

        // A non-jsonl file of the same size keeps the head-read (no seek surprise
        // for formats whose newest content is NOT at the end).
        let other = dir.join("some-session.json");
        std::fs::write(&other, &contents).unwrap();
        let (head, head_truncated) =
            read_session_preview_window(&other, false, false, size_bytes).unwrap();
        assert!(head_truncated);
        assert!(String::from_utf8_lossy(&head).starts_with("{\"type\":\"assistant\""));
        assert!(!String::from_utf8_lossy(&head).contains("NEWEST_CLAUDE_TURN"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_session_head_is_sniffed_and_never_rendered_as_text() {
        // Protobuf-ish bytes, as an Antigravity `.pb` conversation head looks:
        // field tags, varints, embedded lengths and plenty of NULs.
        let mut proto = vec![0x0a, 0x12, 0x08, 0x96, 0x01, 0x00, 0x00, 0x12, 0x07];
        proto.extend_from_slice(b"convers");
        proto.extend_from_slice(&[0u8; 64]);
        proto.extend_from_slice(&[0x1a, 0x05]);
        proto.extend_from_slice(b"hello");
        assert!(looks_binary_session_head(&proto));

        // SQLite headers carry NULs too.
        let mut sqlite = b"SQLite format 3\0".to_vec();
        sqlite.extend_from_slice(&[0u8; 100]);
        assert!(looks_binary_session_head(&sqlite));

        // Real transcript heads are never flagged — including tabs/newlines/CRLF.
        assert!(!looks_binary_session_head(
            b"{\"type\":\"user\",\"cwd\":\"C:\\\\proj\"}\r\n\t{\"type\":\"assistant\"}\n"
        ));
        assert!(!looks_binary_session_head(b""));
        // NUL padding from a torn append lives past the sniffed head and does not
        // reclassify an otherwise readable transcript.
        let mut torn = vec![b'a'; 8192];
        torn.extend_from_slice(&[0u8; 512]);
        assert!(!looks_binary_session_head(&torn));
    }

    #[test]
    #[ignore = "depends on the local user's real Hermes state database"]
    fn real_hermes_session_preview_opens_through_the_public_api() {
        let report = hangar_discovery::discover_known_projects(
            &[],
            hangar_discovery::DiscoveryOptions {
                limit: 0,
                include_loose_sessions: true,
                include_agents: true,
                include_technical_candidates: false,
            },
        );
        // Skip gracefully (rather than panic on `expect`) when this Windows machine has
        // no Hermes state database — the test is a real-data check, not a fixture, so
        // "no Hermes state present" is a skip, not a failure.
        let Some(session) = report.sessions.into_iter().find(|session| {
            session.source_kind.contains("hermes_state")
                && session.path.contains("#hermes-session=")
        }) else {
            eprintln!("skipping: no real Hermes SQLite session on this machine");
            return;
        };
        let base = session
            .path
            .rsplit_once('#')
            .map(|(base, _)| PathBuf::from(base))
            .expect("Hermes base path");
        let canonical = base.canonicalize().expect("canonical Hermes base path");
        assert!(hangar_discovery::is_hermes_state_db(&base));
        assert!(hangar_discovery::is_hermes_state_db(&canonical));
        let preview = session_preview(session.path, false).expect("bounded Hermes preview");

        assert_eq!(preview.session_kind, "Hermes/NemoClaw");
        assert!(!preview.text.trim().is_empty());
        assert!(!preview.revealed);
    }

    // FIX 3: a Cursor in-IDE chat, previewed through the SAME public entry point the UI
    // uses, must come back as a clean readable transcript (role-labelled turns), NOT the
    // binary "unreadable store" note, with redaction wired and `reveal` honored. Runs
    // only against the real `state.vscdb` because `session_preview`'s allow-list is keyed
    // on the real session-store roots; reports COUNTS only, never conversation content.
    #[test]
    #[ignore = "depends on the local user's real Cursor state.vscdb"]
    fn real_cursor_ide_chat_preview_opens_through_the_public_api() {
        let report = hangar_discovery::discover_known_projects(
            &[],
            hangar_discovery::DiscoveryOptions {
                limit: 0,
                include_loose_sessions: true,
                include_agents: true,
                include_technical_candidates: false,
            },
        );
        let cursor_sessions: Vec<_> = report
            .sessions
            .into_iter()
            .filter(|session| {
                session.source_kind == "cursor_ide_chats"
                    && session.path.contains("#cursor-ide-chat=")
            })
            .collect();
        if cursor_sessions.is_empty() {
            eprintln!("skipping: no real Cursor in-IDE conversation on this machine");
            return;
        }

        // EVERY listed Cursor session must preview cleanly: a rendered transcript, or the
        // calm empty-draft note — NEVER the alarming "couldn't read this store" fallback
        // (that is reserved for a genuinely locked/corrupt DB). Confirm at least one real
        // conversation renders role-labelled turns.
        let mut rendered = 0usize;
        let mut empty = 0usize;
        for session in &cursor_sessions {
            let preview =
                session_preview(session.path.clone(), false).expect("bounded Cursor preview");
            assert_eq!(preview.session_kind, "Cursor");
            assert!(!preview.revealed);
            assert!(
                !preview.text.contains("couldn't read this session store"),
                "a listed Cursor chat must never fall to the unreadable-store note"
            );
            if preview.text.contains("## User") || preview.text.contains("## Assistant") {
                rendered += 1;
            } else if preview.text.contains("no messages yet") {
                empty += 1;
            }
        }
        println!(
            "[real] cursor previews: {} total, {} rendered transcripts, {} empty drafts",
            cursor_sessions.len(),
            rendered,
            empty
        );
        assert!(
            rendered >= 1,
            "at least one real Cursor conversation should render turns"
        );
    }

    // FIX 3 (redaction): the Cursor branch runs its assembled transcript through the very
    // same `redact_secrets` the Hermes/OpenClaw/Antigravity previews use. Prove that gate
    // masks a secret embedded in a Cursor-shaped transcript and reports the count — the
    // deterministic half of the redaction guarantee (the real-machine test above proves it
    // is actually wired into `session_preview`).
    #[test]
    fn cursor_transcript_shape_is_secret_redacted() {
        let transcript = "## User\n\nplease deploy\n\n## Assistant\n\n\
             Using token ghp_abcdefghijklmnopqrstuvwxyz0123456789 now.";
        let (redacted, count) = redact_secrets(transcript);
        assert!(count >= 1, "the embedded token must be masked");
        assert!(
            !redacted.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "a secret must never survive redaction: {redacted}"
        );
        // The surrounding role-labelled prose is preserved.
        assert!(redacted.contains("## User"));
        assert!(redacted.contains("please deploy"));
    }

    fn project_summary_fixture(path: &str) -> ProjectSummary {
        ProjectSummary {
            id: 1,
            name: "fixture".to_string(),
            path: path.to_string(),
            source: "registry".to_string(),
            context_count: 0,
            pinned: false,
            protected_level: None,
            scan_state: "scanned".to_string(),
            scan_root_id: None,
            antigravity_name: None,
            is_current: false,
            app: None,
            apps: Vec::new(),
        }
    }

    #[test]
    fn enrich_current_state_leaves_unknown_paths_inactive() {
        // A path no registry/activity signal claims must stay is_current = false and
        // app = None — and enrichment must never panic on real-machine data.
        let mut projects = vec![project_summary_fixture(
            r"C:\definitely\not\a\real\registered\project\zzz",
        )];
        enrich_current_state(&mut projects);
        assert!(!projects[0].is_current);
        assert_eq!(projects[0].app, None);
    }

    /// Real-data verification that `enrich_current_state` flips a project the local AI
    /// apps currently track to `is_current` (which un-archives it on the frontend).
    /// The prior version asserted an anonymized placeholder path that exists on no
    /// machine, so it could never pass. It now targets a project supplied via
    /// `CODEHANGAR_TEST_ACTIVE_PROJECT_PATH` (a path the tester knows the AI-app
    /// registries reference) and SKIPS when that env var is unset — so the check is
    /// portable and never asserts a machine-specific assumption. Ignored by default
    /// (real-machine data).
    #[test]
    #[ignore = "depends on the local user's real AI-app registry data"]
    fn real_enrich_current_state_marks_active_project() {
        let Ok(path) = std::env::var("CODEHANGAR_TEST_ACTIVE_PROJECT_PATH") else {
            eprintln!(
                "skipping: set CODEHANGAR_TEST_ACTIVE_PROJECT_PATH to a project the \
                 local AI apps currently track (e.g. C:\\AI\\Codex\\CodeHangar)"
            );
            return;
        };
        let mut projects = vec![project_summary_fixture(path.trim())];
        enrich_current_state(&mut projects);
        assert!(
            projects[0].is_current,
            "a project the AI-app registries reference must be marked current by the API pass"
        );
    }
}
