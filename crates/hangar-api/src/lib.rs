use chrono::{DateTime, Utc};
use hangar_core::{
    display_path_for_path, normalize_path, AdapterSummary, Comment, ContextFile, DashboardSummary,
    DocumentSearchResult, DuplicateCandidates, DuplicateConfirmation, ExportResult,
    FileChangeEvent, FilePreview, FocusedWatcherStatus, FolderExplanation, FolderInvestigation,
    GitRepoSummary, GraphMap, InvestigationHandle, LostProjectCandidates, MutationActivityLog,
    MutationBackupSummary, MutationFinalRemoveSummary, MutationLockInspection, MutationMoveSummary,
    MutationProtectedPreview, MutationRestoreSummary, MutationTokenResult, NavChildrenPage,
    NavItem, NodeRelationships, OpenTargetInspection, OpenTargetPreparation, OpenTargetScanStart,
    OperationPlan, OrphanCandidates, OrphanStatus, PinnedItem, PlanPreviewStatus, PreviewMode,
    PreviewPolicy, ProcessResourceUsage, ProjectDetail, ProjectDiscoveryReport,
    ProjectReviewCheckpoint, ProjectSummary, QuickOpenResult, RecentItem, RecoverableSummary,
    RecoveryPending, RecoveryResolveResult, ReviewLedgerEntry, RiskReport,
    SafeManageOperationPlanRequest, ScanRoot, ScanStatus, SecurityStatus, SessionChangeSet,
    SessionPreview, StartupStatus, SystemResourceProfile, WatcherNodeStatus, WatcherProjectStatus,
    WatcherStatus,
};
#[cfg(feature = "agent_automation")]
use hangar_core::{
    AiFollowUpResult, AiGlossaryEntry, AiGlossaryState, AiRewriteProposal, AiSuggestionApplyResult,
    AiWalkthroughPreview, AutomationActivityEntry, AutomationAgentKind, AutomationAgentSummary,
    AutomationCredential, AutomationReadGrant, AutomationStatus, AutomationTransport,
    CodeAnnotation, ConnectedAppHost,
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
use hangar_jobs::{JobStore, RunningJobAdmission};
#[cfg(feature = "mutation")]
use rusqlite::{params, OptionalExtension};
#[cfg(feature = "agent_automation")]
mod ai_assist;
#[cfg(feature = "mutation")]
mod app_removal;
#[cfg(feature = "agent_automation")]
mod connector_advisory;
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
mod safe_manage;
mod session_changes;
#[cfg(feature = "mutation")]
mod value_edit;
#[cfg(feature = "agent_automation")]
pub use ai_assist::AiExplainPreview;
#[cfg(feature = "mutation")]
pub use app_removal::{
    list_app_removals, record_app_removal, remove_antigravity_registration,
    remove_claude_registration, remove_codex_registration, remove_cursor_registration,
    remove_hermes_registration, remove_project_app_registrations, restore_app_removal,
    restore_app_removal_by_id, AppRemovalOutcome, AppRemovalRecord, PersistedAppRemoval,
};
#[cfg(feature = "agent_automation")]
pub use connector_advisory::{
    ai_safe_manage_advisory, ai_safe_manage_advisory_disclosure, ai_safe_manage_advisory_receipts,
    ai_safe_manage_context_candidates,
};
// Re-export the preview/result wire types so the desktop boundary can name
// them without depending directly on the mutation engine crate.
use dup_jobs::DupJobStore;
#[cfg(feature = "agent_automation")]
pub use hangar_ai::AiUsageStatus;
#[cfg(feature = "mutation")]
pub use hangar_mutation::{FinalRemoveBatchResult, FinalRemovePreview, FinalRemoveScope};
use performance::{scan_limits, PerformanceMode, PerformanceScope};
use plan_jobs::PlanJobStore;
pub use project_summary::project_context_summary;
pub use safe_manage::{
    analysis_cancel as safe_manage_analysis_cancel, analysis_start as safe_manage_analysis_start,
    analysis_status as safe_manage_analysis_status, decision_record as safe_manage_decision_record,
    decisions_record_atomic as safe_manage_decisions_record_atomic,
    first_run_preference as safe_manage_first_run_preference,
    first_run_preference_set as safe_manage_first_run_preference_set,
    operation_plan_start as safe_manage_operation_plan_start, overview as safe_manage_overview,
    regenerable_scan_start as safe_manage_regenerable_scan_start,
    regenerable_targets as safe_manage_regenerable_targets,
};
#[cfg(any(feature = "agent_automation", feature = "mutation"))]
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(feature = "mutation")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "mutation")]
use std::sync::Arc;
#[cfg(feature = "agent_automation")]
use std::sync::OnceLock;
use std::sync::{Arc as SharedArc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROJECT_APP_STATE_CACHE_TTL: Duration = Duration::from_secs(60);
const PROJECT_DISK_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const PROJECT_DISK_CACHE_MAX_JSON_BYTES: usize = 1024 * 1024;

// Re-exported so the desktop app (which depends on hangar-api, not directly on
// hangar-appconfig) can name the connected-app host status in its Tauri commands.
#[cfg(feature = "agent_automation")]
pub use hangar_appconfig::HostStatus;

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
            let recovery_sql = format!(
                "SELECT o.id, o.kind, o.status, o.target_node_id, o.target_fingerprint,
                         o.created_at, o.started_at, o.error,
                         (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id),
                         (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'done'),
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'pending'),
                        (SELECT COUNT(*) FROM operation_item oi WHERE oi.operation_id = o.id AND oi.status = 'failed')
                  FROM operation o
                  WHERE o.status IN ('executing', 'backup_running', 'verifying')
                     OR EXISTS (
                         SELECT 1 FROM permanent_delete_batch b
                         WHERE b.operation_id = o.id AND ({predicate})
                     )
                     OR EXISTS (
                         SELECT 1 FROM quarantine_entry qe
                         WHERE qe.operation_id = o.id AND qe.status = 'deleting'
                     )
                  ORDER BY o.id",
                predicate = PENDING_FINAL_REMOVE_BATCH_PREDICATE,
            );
            let mut stmt = conn.prepare(&recovery_sql)?;
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
    let _inventory_guard = state
        .inventory_mutation_gate
        .write()
        .map_err(|_| "Inventory/mutation coordination lock is poisoned.".to_string())?;
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
const LEGACY_FINAL_REMOVE_RETIRED: &str =
    "The legacy single-entry final-remove command is retired because it cannot carry an immutable preview-bound grant. Permanent removal remains available through mutation_final_remove_preview, mutation_final_remove_confirm and mutation_final_remove_batch_start.";

#[cfg(feature = "mutation")]
pub fn mutation_token_issue(
    state: &AppState,
    action: String,
) -> Result<MutationTokenResult, String> {
    let parsed = parse_confirm_action(&action)?;
    if matches!(parsed, hangar_mutation::ConfirmAction::PermanentDelete) {
        return Err(LEGACY_FINAL_REMOVE_RETIRED.to_string());
    }
    Ok(MutationTokenResult {
        action,
        token: state
            .mutation_tokens
            .issue(parsed)
            .map_err(|error| error.to_string())?,
    })
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_token_issue(
    _state: &AppState,
    _action: String,
) -> Result<MutationTokenResult, String> {
    Err("Mutation confirmation tokens require a mutation-enabled build.".to_string())
}

/// The canonical pending-state predicate for a final-removal batch. Keep this
/// aligned with the recovery engine's candidate selection: a terminal-looking
/// header cannot hide a deleting held entry, an unlinked promotion proof, or a
/// malformed item count.
#[cfg(feature = "mutation")]
const PENDING_FINAL_REMOVE_BATCH_PREDICATE: &str = r#"
    NOT (
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
        WHERE bi.batch_id = b.id
          AND bi.archive_id IS NULL
          AND bi.archive_proof_blake3 IS NOT NULL
          AND bi.phase != 'archive_recovery_closed'
    ) OR b.requested_count != (
        SELECT COUNT(*) FROM permanent_delete_batch_item bi
        WHERE bi.batch_id = b.id
    )
"#;

#[cfg(feature = "mutation")]
#[derive(Debug, Clone)]
struct PendingFinalRemoveBatch {
    numeric_id: i64,
    public_id: Option<String>,
    batch_status: String,
    operation_status: String,
    unresolved_items: i64,
}

/// Load every final-removal batch which still needs recovery. The recovery
/// dashboard, recovery prompt and forward-mutation guard must all use this
/// selection so none can call the same durable state idle while another blocks.
#[cfg(feature = "mutation")]
fn pending_final_remove_batches(
    conn: &rusqlite::Connection,
) -> Result<Vec<PendingFinalRemoveBatch>, DbError> {
    let sql = format!(
        "SELECT b.id, b.public_id, b.status, o.status,
                (SELECT COUNT(*)
                 FROM permanent_delete_batch_item bi
                 JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
                 WHERE bi.batch_id = b.id
                   AND (bi.status NOT IN ('deleted', 'blocked', 'failed')
                        OR qe.status = 'deleting'
                        OR (bi.archive_id IS NULL
                            AND bi.archive_proof_blake3 IS NOT NULL
                            AND bi.phase != 'archive_recovery_closed')))
         FROM permanent_delete_batch b
         JOIN operation o ON o.id = b.operation_id
         WHERE ({predicate})
         ORDER BY b.id",
        predicate = PENDING_FINAL_REMOVE_BATCH_PREDICATE,
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok(PendingFinalRemoveBatch {
                numeric_id: row.get(0)?,
                public_id: row.get(1)?,
                batch_status: row.get(2)?,
                operation_status: row.get(3)?,
                unresolved_items: row.get(4)?,
            })
        })
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| DbError::FileRead(error.to_string()))
}

/// Refuse any new forward mutation while a prior operation was left interrupted. The
/// journal-first design assumes recovery runs to completion before the next disk action;
/// without this guard a second mutation could stack on an unreconciled one.
/// `failed` is deliberately not included: executors use it only after reconciling their
/// physical outcome (for example, a partial quarantine keeps every moved copy as a visible
/// entry, and a post-move restore warning marks the entry restored). Ambiguous outcomes stay
/// `executing`/`verifying`, so they continue to block here and remain visible in Recovery.
#[cfg(feature = "mutation")]
fn ensure_no_pending_recovery(conn: &mut rusqlite::Connection) -> Result<(), DbError> {
    let generic_or_orphan_pending: i64 = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM operation
                 WHERE status IN ('executing', 'backup_running', 'verifying'))
              + (SELECT COUNT(*) FROM permanent_delete_batch_item
                 WHERE status IN ('deleting', 'interrupted'))
              + (SELECT COUNT(*) FROM quarantine_entry WHERE status = 'deleting')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    if generic_or_orphan_pending > 0 || !pending_final_remove_batches(conn)?.is_empty() {
        return Err(DbError::FileRead(
            "A previous mutation was interrupted and must be recovered first. Open the Recovery area and resolve it before any new backup, move or delete."
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "mutation")]
fn mutation_exclusive_guard(
    state: &AppState,
) -> Result<std::sync::RwLockWriteGuard<'_, ()>, String> {
    state
        .inventory_mutation_gate
        .write()
        .map_err(|_| "Inventory/mutation coordination lock is poisoned.".to_string())
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
    let _inventory_guard = mutation_exclusive_guard(state)?;
    hangar_mutation::validate_local_mutation_path(Path::new(&destination_root))
        .map_err(|error| error.to_string())?;
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
            let plan_items = concrete_items_for_plan(conn, &plan, include_protected)?;
            let current_plan = plan_items.current_plan;
            let concrete = plan_items.items;
            let backup_items = concrete
                .iter()
                .map(|item| hangar_mutation::BackupItem {
                    source: item.source.clone(),
                    relative: item.relative.clone(),
                    expected_source_stamp: item.expected_source_stamp.clone(),
                    expected_source_hash: item.expected_source_hash.clone(),
                })
                .collect::<Vec<_>>();
            // The backup's in-source guard must protect the WHOLE folder that the move will
            // empty, not just the deepest common ancestor of the concrete items. For a
            // project/directory target the move recursively empties `plan.target.path`
            // (cleanup_root), so a backup written anywhere inside it would be moved/deleted with
            // everything else — losing the user's only backup. Guard against that exact root.
            let source_root =
                if matches!(current_plan.target.kind.as_str(), "project" | "directory") {
                    std::path::PathBuf::from(&current_plan.target.path)
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
                    plan_json: serde_json::to_string(&current_plan)
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
    let _inventory_guard = mutation_exclusive_guard(state)?;
    hangar_mutation::validate_local_mutation_path(Path::new(&holding_root))
        .map_err(|error| error.to_string())?;
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
            let current_plan = plan_items.current_plan;
            let concrete = plan_items.items;
            // The verified backup manifest is the only source of move-time hash and
            // source stamp. No path-based pre-hash is performed here: the executor
            // opens the reviewed object once and verifies both values through that
            // same handle before journaling or moving it.
            let items = concrete
                .iter()
                .map(|item| {
                    let source_text = item.source.to_string_lossy().to_string();
                    let copy = backup.copy_for(&source_text).ok_or_else(|| {
                        DbError::FileRead(format!(
                            "The chosen backup does not cover {source_text}. Create a verified backup of every file before moving."
                        ))
                    })?;
                    let source_stamp = copy.source_stamp().ok_or_else(|| {
                        DbError::FileRead(format!(
                            "The chosen backup has no identity-bound source stamp for {source_text}. Re-create the backup before moving."
                        ))
                    })?;
                    if source_stamp != &item.expected_source_stamp {
                        return Err(DbError::FileRead(format!(
                            "The chosen backup was created from a different reviewed identity for {source_text}. Re-create the plan and backup before moving."
                        )));
                    }
                    Ok(hangar_mutation::QuarantineItem {
                        source: item.source.clone(),
                        relative: item.relative.clone(),
                        expected_source_stamp: source_stamp.clone(),
                        backup_hash: copy.blake3.clone(),
                    })
                })
                .collect::<Result<Vec<_>, DbError>>()?;
            let source_paths = concrete
                .iter()
                .map(|item| item.source.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            // The move gate is a retained-handle proof, not a momentary path
            // hash. Every backup payload stays bound until all corresponding
            // source moves and their journal updates have completed.
            let _backup_payload_guard = backup
                .bind_payloads(&source_paths)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let cleanup_root = matches!(
                current_plan.target.kind.as_str(),
                "project" | "directory"
            )
            .then(|| std::path::PathBuf::from(&current_plan.target.path));
            let result = hangar_mutation::quarantine(
                conn,
                hangar_mutation::QuarantineRequest {
                    quarantine_root: Path::new(&holding_root),
                    items,
                    plan_json: serde_json::to_string(&current_plan)
                        .map_err(|err| DbError::FileRead(err.to_string()))?,
                    target_node_id: Some(current_plan.target.node_id),
                    target_fingerprint: Some(current_plan.target_fingerprint.clone()),
                    backup_id: verified_backup_id,
                    // For a project/folder target, remove the now-empty source
                    // directories after the move so the whole folder leaves the disk.
                    cleanup_root: cleanup_root.clone(),
                    include_protected,
                    reparse_links: Vec::new(),
                },
            )
            .map_err(|err| DbError::FileRead(err.to_string()))?;
            if result.failed != 0 || result.skipped != 0 {
                return Err(DbError::FileRead(format!(
                    "Move is incomplete: {} failed and {} skipped item(s). The project remains registered and recovery entries stay visible.",
                    result.failed, result.skipped
                )));
            }
            if let Some(root) = cleanup_root.as_deref() {
                prove_cleanup_root_absent_or_empty(root)?;
            }
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

/// Read-only preview of an opt-in "empty the folder completely": the
/// sensitive/protected files that would be copied to the backup then moved.
/// Reparse entries block this preview; the API never promises to unlink them.
#[cfg(feature = "mutation")]
pub fn mutation_preview_protected(
    state: &AppState,
    plan: OperationPlan,
) -> Result<MutationProtectedPreview, String> {
    let _inventory_guard = mutation_exclusive_guard(state)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|err| DbError::FileRead(err.to_string()))?;
            let plan_items = concrete_items_for_plan(conn, &plan, true)?;
            Ok(MutationProtectedPreview {
                protected: plan_items.protected_paths,
                reparse: Vec::new(),
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
    let _inventory_guard = mutation_exclusive_guard(state)?;
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
    let _inventory_guard = mutation_exclusive_guard(state)?;
    hangar_mutation::validate_local_mutation_path(Path::new(&destination_root))
        .map_err(|error| error.to_string())?;
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
    _state: &AppState,
    _entry_id: i64,
    _token: String,
) -> Result<MutationFinalRemoveSummary, String> {
    Err(LEGACY_FINAL_REMOVE_RETIRED.to_string())
}

/// Batch preview contract used by the primary project-cleanup flow. The feature
/// remains visible in Recovery, but starts disabled and cannot even mint a
/// preview until the local owner has explicitly enabled permanent removal.
#[cfg(feature = "mutation")]
pub fn mutation_final_remove_preview(
    state: &AppState,
    scope: hangar_mutation::FinalRemoveScope,
) -> Result<hangar_mutation::FinalRemovePreview, String> {
    let _inventory_guard = mutation_exclusive_guard(state)?;
    require_final_remove_enabled(state)?;
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            hangar_mutation::build_final_remove_preview(conn, scope.clone())
                .map_err(|error| DbError::FileRead(error.to_string()))
        })
        .map_err(to_message)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_final_remove_preview(_state: &AppState, _scope: ()) -> Result<(), String> {
    Err("Final-cleanup previews require a mutation-enabled build.".to_string())
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveConfirmationToken {
    pub token: String,
    pub preview_id: String,
    pub preview_digest: String,
    pub expires_at: String,
}

#[cfg(feature = "mutation")]
#[derive(Clone)]
struct FinalRemoveExecutionControl {
    owner_stop: Arc<AtomicBool>,
    progress_observer_failed: Arc<AtomicBool>,
    capability_disabled: Arc<AtomicBool>,
}

#[cfg(feature = "mutation")]
impl hangar_mutation::FinalRemoveBatchControl for FinalRemoveExecutionControl {
    fn stop_requested(&self) -> bool {
        self.owner_stop.load(Ordering::Acquire)
            || self.progress_observer_failed.load(Ordering::Acquire)
            || self.capability_disabled.load(Ordering::Acquire)
    }

    fn interruption_reason(&self) -> Option<hangar_mutation::FinalRemoveInterruptionReason> {
        // Internal truthfulness failure wins if both signals race. It must
        // never be persisted as though the owner explicitly cancelled.
        if self.progress_observer_failed.load(Ordering::Acquire) {
            Some(hangar_mutation::FinalRemoveInterruptionReason::ProgressObserverFailed)
        } else if self.owner_stop.load(Ordering::Acquire)
            || self.capability_disabled.load(Ordering::Acquire)
        {
            Some(hangar_mutation::FinalRemoveInterruptionReason::OwnerStop)
        } else {
            None
        }
    }
}

#[cfg(feature = "mutation")]
pub fn mutation_final_remove_confirm(
    state: &AppState,
    preview_id: String,
    preview_digest: String,
    selected_topology_group_ids: Vec<String>,
) -> Result<FinalRemoveConfirmationToken, String> {
    let _inventory_guard = mutation_exclusive_guard(state)?;
    require_final_remove_enabled(state)?;
    let binding = state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            hangar_mutation::final_remove_confirmation_binding(
                conn,
                &preview_id,
                &preview_digest,
                selected_topology_group_ids.clone(),
            )
            .map_err(|error| DbError::FileRead(error.to_string()))
        })
        .map_err(to_message)?;
    let token = state
        .mutation_tokens
        .issue_scoped(hangar_mutation::ConfirmAction::PermanentDelete, binding)
        .map_err(|error| error.to_string())?;
    Ok(FinalRemoveConfirmationToken {
        token,
        preview_id,
        preview_digest,
        expires_at: (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    })
}

#[cfg(feature = "mutation")]
fn installed_final_remove_helper_path() -> Result<PathBuf, String> {
    let parent = std::env::current_exe()
        .map_err(|error| format!("Cannot resolve the installed Code Hangar executable: {error}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            "The installed Code Hangar executable has no parent directory.".to_string()
        })?;
    let helper = parent.join("code-hangar-elevated.exe");
    // Validate locality and target-path syntax before any exists/open/read
    // probe. The transport then binds this exact image and its same-directory
    // offline-signed release identity manifest before displaying UAC.
    hangar_mutation::validate_local_mutation_path(&helper).map_err(|error| {
        format!("The installed elevated-helper path is not a local safe path: {error}")
    })?;
    Ok(helper)
}

#[cfg(feature = "mutation")]
type FinalRemoveWorkerTask = Box<dyn FnOnce() + Send + 'static>;

/// Launch a final-removal worker without leaving its admitted in-memory job
/// permanently active if the OS refuses the thread or the worker panics.
///
/// `spawn` is injected so the refusal path can be proved deterministically in
/// unit tests without exhausting process or system resources.
#[cfg(feature = "mutation")]
fn spawn_final_remove_worker_with(
    jobs: &FinalRemoveJobStore,
    summary: &FinalRemoveBatchStartSummary,
    worker: FinalRemoveWorkerTask,
    spawn: impl FnOnce(FinalRemoveWorkerTask) -> std::io::Result<()>,
) -> Result<(), String> {
    let panic_jobs = jobs.clone();
    let panic_job_id = summary.job_id.clone();
    let guarded_worker: FinalRemoveWorkerTask = Box::new(move || {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)).is_err() {
            panic_jobs.fail(
                &panic_job_id,
                "Final-cleanup worker stopped after an unexpected internal panic. Review Recovery before starting another cleanup."
                    .to_string(),
            );
        }
    });
    if let Err(error) = spawn(guarded_worker) {
        let message = format!(
            "Final-cleanup worker could not start; no helper or deletion was launched: {error}"
        );
        jobs.fail(&summary.job_id, message.clone());
        return Err(message);
    }
    Ok(())
}

#[cfg(feature = "mutation")]
fn spawn_final_remove_worker(
    jobs: &FinalRemoveJobStore,
    summary: &FinalRemoveBatchStartSummary,
    worker: FinalRemoveWorkerTask,
) -> Result<(), String> {
    spawn_final_remove_worker_with(jobs, summary, worker, |worker| {
        thread::Builder::new()
            .name("code-hangar-final-remove".to_string())
            .spawn(worker)
            .map(|_| ())
    })
}

#[cfg(feature = "mutation")]
pub fn mutation_final_remove_batch_start(
    state: &AppState,
    request: FinalRemoveBatchStartRequest,
) -> Result<FinalRemoveBatchStartSummary, String> {
    // Serialize the start decision with the owner-controlled capability flag.
    // The worker proves the flag again under the same lock immediately before
    // helper resolution, token consumption, or deletion.
    let _inventory_guard = mutation_exclusive_guard(state)?;
    require_final_remove_enabled(state)?;
    let binding = state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            ensure_no_pending_recovery(conn)?;
            hangar_mutation::final_remove_confirmation_binding(
                conn,
                &request.preview_id,
                &request.preview_digest,
                request.selected_topology_group_ids.clone(),
            )
            .map_err(|error| DbError::FileRead(error.to_string()))
        })
        .map_err(to_message)?;
    let summary = state
        .final_remove_jobs
        .create(u64::from(binding.target_count))?;
    let worker_summary = summary.clone();
    let worker_state = state.clone();
    let worker: FinalRemoveWorkerTask = Box::new(move || {
        let stop_signal = match worker_state
            .final_remove_jobs
            .begin_worker(&worker_summary.job_id)
        {
            Ok(Some(signal)) => signal,
            Ok(None) => return,
            Err(error) => {
                worker_state
                    .final_remove_jobs
                    .fail(&worker_summary.job_id, error);
                return;
            }
        };
        #[cfg(test)]
        let _worker_test_guard = match worker_state.final_remove_worker_test_gate.read() {
            Ok(guard) => guard,
            Err(_) => {
                worker_state.final_remove_jobs.fail(
                    &worker_summary.job_id,
                    "Final-cleanup test coordination lock is poisoned.".to_string(),
                );
                return;
            }
        };
        let _inventory_guard = match worker_state.inventory_mutation_gate.write() {
            Ok(guard) => guard,
            Err(_) => {
                worker_state.final_remove_jobs.fail(
                    &worker_summary.job_id,
                    "Inventory/mutation coordination lock is poisoned.".to_string(),
                );
                return;
            }
        };
        if let Err(error) = require_final_remove_enabled(&worker_state) {
            worker_state
                .final_remove_jobs
                .fail(&worker_summary.job_id, error);
            return;
        }
        let helper_path = match installed_final_remove_helper_path() {
            Ok(path) => path,
            Err(error) => {
                worker_state
                    .final_remove_jobs
                    .fail(&worker_summary.job_id, error);
                return;
            }
        };
        let observer_failed = Arc::new(AtomicBool::new(false));
        let control = FinalRemoveExecutionControl {
            owner_stop: Arc::clone(&stop_signal),
            progress_observer_failed: Arc::clone(&observer_failed),
            capability_disabled: Arc::clone(&worker_state.final_remove_disable_latch),
        };
        let observer_jobs = worker_state.final_remove_jobs.clone();
        let observer_job_id = worker_summary.job_id.clone();
        let observer_failed_latch = Arc::clone(&observer_failed);
        let mut observer = move |progress: hangar_mutation::FinalRemoveBatchProgress| {
            if observer_jobs
                .update_progress(&observer_job_id, progress)
                .is_err()
            {
                // Losing the truthful job-progress mirror must not let another
                // irreversible topology group begin unnoticed.
                observer_failed_latch.store(true, Ordering::Release);
            }
        };
        let result = worker_state.db().and_then(|db| {
            db.with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|error| DbError::FileRead(error.to_string()))?;
                ensure_no_pending_recovery(conn)?;
                hangar_mutation::execute_final_remove_batch_controlled(
                    conn,
                    &worker_state.mutation_tokens,
                    &request.confirmation_token,
                    &request.preview_id,
                    &request.preview_digest,
                    request.selected_topology_group_ids.clone(),
                    &helper_path,
                    &worker_summary.batch_id,
                    &control,
                    &mut observer,
                )
                .map_err(|error| DbError::FileRead(error.to_string()))
            })
            .map_err(to_message)
        });
        match result {
            Ok(result) => {
                if let Err(error) = worker_state
                    .final_remove_jobs
                    .complete(&worker_summary.job_id, result)
                {
                    worker_state
                        .final_remove_jobs
                        .fail(&worker_summary.job_id, error);
                }
            }
            Err(error) => worker_state
                .final_remove_jobs
                .fail(&worker_summary.job_id, error),
        }
    });
    spawn_final_remove_worker(&state.final_remove_jobs, &summary, worker)?;
    Ok(summary)
}

#[cfg(feature = "mutation")]
pub fn mutation_final_remove_batch_status(
    state: &AppState,
    job_id: String,
) -> Result<FinalRemoveBatchStatus, String> {
    state.final_remove_jobs.status(&job_id)
}

#[cfg(feature = "mutation")]
pub fn mutation_final_remove_batch_stop(state: &AppState, job_id: String) -> Result<(), String> {
    state.final_remove_jobs.request_stop(&job_id)
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationRecoveryDashboard {
    pub available: bool,
    pub final_remove: FinalRemoveRecoveryState,
    pub message: String,
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveRecoveryState {
    pub state: String,
    pub batch_id: Option<String>,
    pub job_id: Option<String>,
    pub phase: Option<String>,
    pub message: String,
}

/// Read only with respect to filesystem state. Any non-terminal or ambiguous
/// final-removal journal row blocks a new batch rather than assuming idle.
#[cfg(feature = "mutation")]
pub fn mutation_recovery_dashboard(state: &AppState) -> Result<MutationRecoveryDashboard, String> {
    let active_job = state.final_remove_jobs.active();
    let (persisted_interruption, orphan_deleting) = state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let interruption = pending_final_remove_batches(conn)?.into_iter().next();
            let orphan_deleting: i64 = conn.query_row(
                "SELECT COUNT(*) FROM quarantine_entry qe
                 WHERE qe.status = 'deleting'
                   AND NOT EXISTS (
                     SELECT 1 FROM permanent_delete_batch_item bi
                     WHERE bi.quarantine_entry_id = qe.id
                   )",
                [],
                |row| row.get(0),
            )?;
            Ok((interruption, orphan_deleting))
        })
        .map_err(to_message)?;
    let final_remove = if orphan_deleting > 0 {
        FinalRemoveRecoveryState {
            state: "unknown".to_string(),
            batch_id: None,
            job_id: None,
            phase: Some("interrupted".to_string()),
            message: "A deleting holding entry has no exact batch-item relation. Reconcile the journal before another cleanup."
                .to_string(),
        }
    } else if let Some(PendingFinalRemoveBatch {
        numeric_id,
        public_id,
        batch_status,
        operation_status,
        unresolved_items,
    }) = persisted_interruption
    {
        let batch_id = public_id.unwrap_or_else(|| numeric_id.to_string());
        if let Some((job_id, progress)) = active_job
            .as_ref()
            .filter(|(_, progress)| progress.batch_id == batch_id)
        {
            FinalRemoveRecoveryState {
                state: "active".to_string(),
                batch_id: Some(batch_id),
                job_id: Some(job_id.clone()),
                phase: Some(progress.phase.clone()),
                message: "A same-process final-cleanup worker is active.".to_string(),
            }
        } else {
            FinalRemoveRecoveryState {
                state: "interrupted".to_string(),
                batch_id: Some(batch_id),
                job_id: None,
                phase: Some(batch_phase_wire(&batch_status)),
                message: format!(
                    "A persisted final-cleanup batch is not coherently terminal (batch={batch_status}, operation={operation_status}, unresolvedItems={unresolved_items}). No live same-process worker owns it."
                ),
            }
        }
    } else if let Some((job_id, progress)) = active_job {
        FinalRemoveRecoveryState {
            state: "active".to_string(),
            batch_id: Some(progress.batch_id.clone()),
            job_id: Some(job_id),
            phase: Some(progress.phase),
            message: "A same-process final-cleanup worker is preparing its durable batch journal."
                .to_string(),
        }
    } else {
        FinalRemoveRecoveryState {
            state: "idle".to_string(),
            batch_id: None,
            job_id: None,
            phase: None,
            message: "No final-cleanup batch is active or interrupted.".to_string(),
        }
    };
    Ok(MutationRecoveryDashboard {
        available: true,
        message: final_remove.message.clone(),
        final_remove,
    })
}

#[cfg(feature = "mutation")]
fn batch_phase_wire(status: &str) -> String {
    match status {
        "waiting_for_uac" => "waitingForUac",
        "verifying_archives" => "verifyingArchives",
        "roundtrip" => "roundtrip",
        "parent_disposition" => "parentDisposition",
        "deleting" => "deleting",
        "cleaning_dirs" => "cleaningDirs",
        _ => "interrupted",
    }
    .to_string()
}

#[cfg(all(test, feature = "mutation"))]
mod final_remove_api_tests {
    use super::*;

    fn seed_batch(
        state: &AppState,
        id: i64,
        public_id: &str,
        batch_status: &str,
        operation_status: &str,
    ) {
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|error| DbError::FileRead(error.to_string()))?;
                conn.execute(
                    "INSERT INTO operation(
                        id, kind, status, plan_json, target_fingerprint, created_at
                     ) VALUES(?1, 'permanent_delete_batch', ?2, '{}', ?3,
                              '2026-08-23T00:00:00Z')",
                    rusqlite::params![id, operation_status, format!("v2:{}", "ab".repeat(32))],
                )?;
                conn.execute(
                    "INSERT INTO permanent_delete_batch(
                        id, public_id, operation_id, preview_id, preview_digest,
                        selected_groups_json, requested_count, status, created_at,
                        finished_at
                     ) VALUES(?1, ?2, ?1, ?3, ?4, '[]', 0, ?5,
                              '2026-08-23T00:00:00Z',
                              CASE WHEN ?5 IN ('completed', 'partial', 'cancelled', 'failed')
                                   THEN '2026-08-23T00:01:00Z' ELSE NULL END)",
                    rusqlite::params![
                        id,
                        public_id,
                        format!("preview-{id}"),
                        format!("v2:{}", "cd".repeat(32)),
                        batch_status,
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn assert_final_remove_recovery_blocks(
        state: &AppState,
        expected_batch_id: &str,
        expected_operation_id: i64,
    ) {
        let dashboard = mutation_recovery_dashboard(state).unwrap();
        assert_eq!(dashboard.final_remove.state, "interrupted");
        assert_eq!(
            dashboard.final_remove.batch_id.as_deref(),
            Some(expected_batch_id)
        );
        let pending = recovery_pending(state).unwrap();
        assert!(pending.pending);
        assert!(
            pending
                .operations
                .iter()
                .any(|operation| operation.id == expected_operation_id),
            "the generic recovery surface must expose the exact blocked operation"
        );
        let guard = state
            .db()
            .unwrap()
            .with_recovery_writer(ensure_no_pending_recovery);
        assert!(
            guard.is_err(),
            "a pending batch must block every new mutation"
        );
    }

    fn seed_terminal_blocked_item_with_deleting_entry(state: &AppState, id: i64, public_id: &str) {
        seed_batch(state, id, public_id, "failed", "failed");
        let entry_id = id * 100;
        let item_id = entry_id;
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO quarantine_entry(
                        id, operation_id, original_path, quarantine_path, status, manifest_json
                     ) VALUES(?1, ?2, 'C:\\project\\held.bin', 'C:\\holding\\held.bin',
                              'deleting', '{}')",
                    rusqlite::params![entry_id, id],
                )?;
                conn.execute(
                    "INSERT INTO operation_item(id, operation_id, action, from_path, status)
                     VALUES(?1, ?2, 'final_remove_bound', 'C:\\holding\\held.bin', 'skipped')",
                    rusqlite::params![item_id, id],
                )?;
                conn.execute(
                    "INSERT INTO permanent_delete_batch_item(
                        id, batch_id, operation_item_id, quarantine_entry_id,
                        removal_group_id, topology_group_id, held_path,
                        expected_volume_id, expected_file_id, expected_bytes,
                        expected_content_blake3, logical_bytes, phase, status,
                        created_at, updated_at
                     ) VALUES(?1, ?2, ?3, ?4, 'project:deleting', 'topology:deleting',
                              'C:\\holding\\held.bin', 'volume-test', 'file-test', 1, ?5, 1,
                              'blocked', 'blocked', '2026-08-24T00:00:00Z',
                              '2026-08-24T00:00:00Z')",
                    rusqlite::params![item_id, id, item_id, entry_id, "12".repeat(32),],
                )?;
                conn.execute(
                    "UPDATE permanent_delete_batch SET requested_count = 1 WHERE id = ?1",
                    [id],
                )?;
                Ok(())
            })
            .unwrap();
    }

    struct TerminalPromotionFixture {
        _root: tempfile::TempDir,
        held_path: PathBuf,
        archive_path: PathBuf,
    }

    /// Reproduce the narrow crash window after helper promotion proof was made
    /// durable but an in-process error falsely labelled the headers terminal.
    fn seed_terminal_promotion_proof(state: &AppState) -> TerminalPromotionFixture {
        const QUARANTINE_OPERATION_ID: i64 = 9_001;
        const BACKUP_ID: i64 = 9_001;
        const ENTRY_ID: i64 = 9_001;
        const DELETE_OPERATION_ID: i64 = 9_002;
        const ELEVATION_ID: i64 = 9_001;
        const OPERATION_ITEM_ID: i64 = 9_001;
        const BATCH_ID: i64 = 9_001;
        const BATCH_ITEM_ID: i64 = 9_001;
        const CAPABILITY_INDEX: u32 = 7;

        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("project/render.bin");
        let held_path = root.path().join("holding/project/render.bin");
        let archive_root = root.path().join("archive-root");
        let nonce = "8a".repeat(32);
        let partial_path =
            hangar_mutation::archive_path_for_capability(&archive_root, &nonce, CAPABILITY_INDEX);
        let archive_path = archive_root
            .join(hangar_mutation::OBJECT_ARCHIVE_DIRECTORY_NAME)
            .join(format!("entry-{ENTRY_ID:016x}.chobj"));
        std::fs::create_dir_all(held_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&held_path, b"held object must never be auto-deleted").unwrap();
        std::fs::write(
            &archive_path,
            b"complete synthetic object_archive/2 payload",
        )
        .unwrap();
        let (held_stamp, held_hash) = hangar_mutation::inspect_local_mutation_file(&held_path)
            .expect("held proof fixture must bind");
        let (archive_stamp, archive_hash) =
            hangar_mutation::inspect_local_mutation_file(&archive_path)
                .expect("promoted archive fixture must bind");
        let nonce_digest = blake3::hash(nonce.as_bytes()).to_hex().to_string();
        let raw_backup_hash = "56".repeat(32);
        let semantic_hash = "34".repeat(32);

        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|error| DbError::FileRead(error.to_string()))?;
                conn.execute(
                    "INSERT INTO backup(id, level, destination, manifest_path, verified, created_at)
                     VALUES(?1, 'full', ?2, ?3, 1, '2026-08-24T00:00:00Z')",
                    rusqlite::params![
                        BACKUP_ID,
                        archive_root.to_string_lossy(),
                        root.path().join("manifest.json").to_string_lossy(),
                    ],
                )?;
                conn.execute(
                    "INSERT INTO operation(id, kind, status, plan_json, created_at)
                     VALUES(?1, 'quarantine', 'completed', '{}', '2026-08-24T00:00:00Z')",
                    [QUARANTINE_OPERATION_ID],
                )?;
                conn.execute(
                    "INSERT INTO quarantine_entry(
                        id, operation_id, original_path, quarantine_path, size, backup_id,
                        status, manifest_json, removal_group_id
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'quarantined', '{}', 'project:proof')",
                    rusqlite::params![
                        ENTRY_ID,
                        QUARANTINE_OPERATION_ID,
                        project_path.to_string_lossy(),
                        held_path.to_string_lossy(),
                        held_stamp.bytes as i64,
                        BACKUP_ID,
                    ],
                )?;
                conn.execute(
                    "INSERT INTO operation(id, kind, status, plan_json, created_at)
                     VALUES(?1, 'permanent_delete_batch', 'failed', '{}',
                             '2026-08-24T00:00:00Z')",
                    [DELETE_OPERATION_ID],
                )?;
                conn.execute(
                    "INSERT INTO elevation_capability(
                        id, operation_id, request_digest, transport_nonce, nonce_digest,
                        status, issued_at
                     ) VALUES(?1, ?2, ?3, ?4, ?5, 'failed', '2026-08-24T00:00:00Z')",
                    rusqlite::params![
                        ELEVATION_ID,
                        DELETE_OPERATION_ID,
                        "71".repeat(32),
                        nonce,
                        nonce_digest,
                    ],
                )?;
                conn.execute(
                    "INSERT INTO operation_item(
                        id, operation_id, action, from_path, bytes, expected_volume_id,
                        expected_file_id, expected_blake3, expected_modified_unix_seconds, status
                     ) VALUES(?1, ?2, 'final_remove_bound', ?3, ?4, ?5, ?6, ?7, ?8, 'pending')",
                    rusqlite::params![
                        OPERATION_ITEM_ID,
                        DELETE_OPERATION_ID,
                        held_path.to_string_lossy(),
                        held_stamp.bytes as i64,
                        held_stamp.volume_id,
                        held_stamp.file_id,
                        held_hash,
                        held_stamp.modified_unix_seconds,
                    ],
                )?;
                conn.execute(
                    "INSERT INTO permanent_delete_batch(
                        id, public_id, operation_id, preview_id, preview_digest,
                        selected_groups_json, requested_count, status, created_at, finished_at
                     ) VALUES(?1, 'terminal-promotion-proof', ?2, 'preview-proof', ?3,
                              '[\"topology:proof\"]', 1, 'failed',
                              '2026-08-24T00:00:00Z', '2026-08-24T00:01:00Z')",
                    rusqlite::params![
                        BATCH_ID,
                        DELETE_OPERATION_ID,
                        format!("v2:{}", "ab".repeat(32)),
                    ],
                )?;
                conn.execute(
                    "INSERT INTO permanent_delete_batch_item(
                        id, batch_id, operation_item_id, quarantine_entry_id,
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
                        phase, status, reason_code, created_at, updated_at
                     ) VALUES(?1, ?2, ?3, ?4, 'project:proof', 'topology:proof', ?5,
                              ?6, ?7, ?8, ?9, ?10, ?8, ?11, ?12,
                              ?13, ?14, ?15, ?16, ?17, ?18,
                              ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?25,
                              1, 1, 1, ?26, 'blocked', 'blocked', 'helperFailed',
                              '2026-08-24T00:00:00Z', '2026-08-24T00:00:00Z')",
                    rusqlite::params![
                        BATCH_ITEM_ID,
                        BATCH_ID,
                        OPERATION_ITEM_ID,
                        ENTRY_ID,
                        held_path.to_string_lossy(),
                        held_stamp.volume_id,
                        held_stamp.file_id,
                        held_stamp.bytes as i64,
                        held_stamp.modified_unix_seconds,
                        held_hash,
                        ELEVATION_ID,
                        CAPABILITY_INDEX as i64,
                        partial_path.to_string_lossy(),
                        archive_stamp.volume_id,
                        archive_stamp.file_id,
                        archive_stamp.bytes as i64,
                        archive_stamp.modified_unix_seconds,
                        archive_path.to_string_lossy(),
                        archive_stamp.volume_id,
                        archive_stamp.file_id,
                        archive_stamp.bytes as i64,
                        archive_stamp.modified_unix_seconds,
                        archive_hash,
                        raw_backup_hash,
                        semantic_hash,
                        hangar_mutation::OBJECT_ARCHIVE_PROOF_SCHEMA,
                    ],
                )?;
                Ok(())
            })
            .unwrap();

        TerminalPromotionFixture {
            _root: root,
            held_path,
            archive_path,
        }
    }

    #[test]
    fn dashboard_finds_older_interruption_even_when_newer_batch_is_terminal() {
        let state = AppState::memory().unwrap();
        seed_batch(
            &state,
            10,
            "older-interrupted",
            "parent_disposition",
            "parent_disposition",
        );
        seed_batch(&state, 20, "newer-complete", "completed", "completed");

        let dashboard = mutation_recovery_dashboard(&state).unwrap();
        assert_eq!(dashboard.final_remove.state, "interrupted");
        assert_eq!(
            dashboard.final_remove.batch_id.as_deref(),
            Some("older-interrupted")
        );
        assert_eq!(dashboard.final_remove.job_id, None);
        assert_eq!(
            dashboard.final_remove.phase.as_deref(),
            Some("parentDisposition")
        );
    }

    #[test]
    fn persisted_nonterminal_batch_is_interrupted_after_process_restart() {
        let state = AppState::memory().unwrap();
        seed_batch(
            &state,
            10,
            "durable-without-worker",
            "verifying_archives",
            "verifying_archives",
        );

        let dashboard = mutation_recovery_dashboard(&state).unwrap();
        assert_eq!(dashboard.final_remove.state, "interrupted");
        assert_eq!(dashboard.final_remove.job_id, None);
        assert_eq!(
            dashboard.final_remove.phase.as_deref(),
            Some("verifyingArchives")
        );
        let pending = recovery_pending(&state).unwrap();
        assert!(pending.pending);
        assert!(pending
            .operations
            .iter()
            .any(|operation| operation.id == 10));
    }

    #[test]
    fn terminal_headers_with_ready_item_are_not_reported_idle() {
        let state = AppState::memory().unwrap();
        seed_batch(&state, 30, "false-terminal", "completed", "completed");
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO quarantine_entry(
                        id, operation_id, original_path, quarantine_path, status, manifest_json
                     ) VALUES(300, 30, 'C:\\project\\a.bin', 'C:\\holding\\a.bin',
                              'quarantined', '{}')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO operation_item(
                        id, operation_id, action, from_path, status
                     ) VALUES(300, 30, 'final_remove_bound', 'C:\\holding\\a.bin', 'pending')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO permanent_delete_batch_item(
                        id, batch_id, operation_item_id, quarantine_entry_id,
                        removal_group_id, topology_group_id, held_path,
                        expected_volume_id, expected_file_id, expected_bytes,
                        expected_content_blake3, logical_bytes, phase, status,
                        created_at, updated_at
                     ) VALUES(300, 30, 300, 300, 'project:30', 'topology:30',
                              'C:\\holding\\a.bin', 'volume-c', 'file-a', 1, ?1, 1,
                              'archive_ready', 'ready', '2026-08-23T00:00:00Z',
                              '2026-08-23T00:00:00Z')",
                    ["12".repeat(32)],
                )?;
                conn.execute(
                    "UPDATE permanent_delete_batch SET requested_count = 1 WHERE id = 30",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let dashboard = mutation_recovery_dashboard(&state).unwrap();
        assert_eq!(dashboard.final_remove.state, "interrupted");
        assert_eq!(
            dashboard.final_remove.batch_id.as_deref(),
            Some("false-terminal")
        );
        let pending = recovery_pending(&state).unwrap();
        assert!(pending.pending);
    }

    #[test]
    fn terminal_header_pair_mismatch_blocks_every_recovery_surface() {
        let state = AppState::memory().unwrap();
        seed_batch(&state, 40, "terminal-pair-mismatch", "completed", "failed");

        assert_final_remove_recovery_blocks(&state, "terminal-pair-mismatch", 40);
    }

    #[test]
    fn terminal_blocked_item_with_deleting_entry_blocks_every_recovery_surface() {
        let state = AppState::memory().unwrap();
        seed_terminal_blocked_item_with_deleting_entry(&state, 41, "deleting-held-entry");

        assert_final_remove_recovery_blocks(&state, "deleting-held-entry", 41);
    }

    #[test]
    fn terminal_requested_count_mismatch_blocks_every_recovery_surface() {
        let state = AppState::memory().unwrap();
        seed_batch(&state, 42, "terminal-count-mismatch", "failed", "failed");
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "UPDATE permanent_delete_batch SET requested_count = 1 WHERE id = 42",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        assert_final_remove_recovery_blocks(&state, "terminal-count-mismatch", 42);
    }

    #[test]
    fn terminal_promotion_proof_remains_visible_until_recovery_reconciles_it() {
        let state = AppState::memory().unwrap();
        let fixture = seed_terminal_promotion_proof(&state);

        assert_final_remove_recovery_blocks(&state, "terminal-promotion-proof", 9_002);

        let resolved = recovery_resolve(&state, "rollback".to_string()).unwrap();
        assert_eq!(resolved.action, "rollback");
        assert_eq!(resolved.recovered_operations, 1);
        assert_eq!(resolved.rolled_back_items, 0);
        assert!(fixture.archive_path.exists());
        assert_eq!(
            std::fs::read(&fixture.held_path).unwrap(),
            b"held object must never be auto-deleted"
        );
        let archive: (i64, String) = state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.query_row(
                    "SELECT oa.id, oa.status
                     FROM object_archive oa
                     JOIN permanent_delete_batch_item bi ON bi.archive_id = oa.id
                     WHERE bi.id = 9001",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(DbError::from)
            })
            .unwrap();
        assert!(archive.0 > 0);
        assert_eq!(archive.1, "ready");
        assert_eq!(
            mutation_recovery_dashboard(&state)
                .unwrap()
                .final_remove
                .state,
            "idle"
        );
        assert!(!recovery_pending(&state).unwrap().pending);
        assert!(state
            .db()
            .unwrap()
            .with_recovery_writer(ensure_no_pending_recovery)
            .is_ok());
    }

    #[test]
    fn deleting_entry_without_batch_is_visible_on_recovery_surfaces() {
        let state = AppState::memory().unwrap();
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::ensure_journal_schema(conn)
                    .map_err(|error| DbError::FileRead(error.to_string()))?;
                conn.execute(
                    "INSERT INTO operation(id, kind, status, plan_json, created_at)
                     VALUES(43, 'permanent_delete', 'failed', '{}', '2026-08-24T00:00:00Z')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO quarantine_entry(
                        id, operation_id, original_path, quarantine_path, status, manifest_json
                     ) VALUES(43, 43, 'C:\\project\\orphan.bin', 'C:\\holding\\orphan.bin',
                              'deleting', '{}')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let dashboard = mutation_recovery_dashboard(&state).unwrap();
        assert_eq!(dashboard.final_remove.state, "unknown");
        assert!(recovery_pending(&state)
            .unwrap()
            .operations
            .iter()
            .any(|operation| operation.id == 43));
        assert!(state
            .db()
            .unwrap()
            .with_recovery_writer(ensure_no_pending_recovery)
            .is_err());
    }

    #[test]
    fn dashboard_uses_actual_same_process_job_id_and_exact_camel_case_wire() {
        let state = AppState::memory().unwrap();
        let summary = state.final_remove_jobs.create(2).unwrap();

        let dashboard = mutation_recovery_dashboard(&state).unwrap();
        assert_eq!(dashboard.final_remove.state, "active");
        assert_eq!(
            dashboard.final_remove.batch_id.as_deref(),
            Some(summary.batch_id.as_str())
        );
        assert_eq!(
            dashboard.final_remove.job_id.as_deref(),
            Some(summary.job_id.as_str())
        );
        assert_eq!(
            dashboard.final_remove.phase.as_deref(),
            Some("waitingForUac")
        );
        let wire = serde_json::to_value(&dashboard).unwrap();
        let final_remove = wire.get("finalRemove").unwrap();
        assert!(final_remove.get("batchId").is_some());
        assert!(final_remove.get("jobId").is_some());
        assert_eq!(final_remove["phase"], "waitingForUac");
        assert!(wire.get("final_remove").is_none());
    }

    #[test]
    fn final_remove_control_never_mislabels_progress_failure_as_owner_stop() {
        let owner_stop = Arc::new(AtomicBool::new(false));
        let progress_observer_failed = Arc::new(AtomicBool::new(false));
        let capability_disabled = Arc::new(AtomicBool::new(false));
        let control = FinalRemoveExecutionControl {
            owner_stop: Arc::clone(&owner_stop),
            progress_observer_failed: Arc::clone(&progress_observer_failed),
            capability_disabled: Arc::clone(&capability_disabled),
        };
        assert_eq!(
            hangar_mutation::FinalRemoveBatchControl::interruption_reason(&control),
            None
        );

        capability_disabled.store(true, Ordering::Release);
        assert_eq!(
            hangar_mutation::FinalRemoveBatchControl::interruption_reason(&control),
            Some(hangar_mutation::FinalRemoveInterruptionReason::OwnerStop)
        );
        assert!(hangar_mutation::FinalRemoveBatchControl::stop_requested(
            &control
        ));
        capability_disabled.store(false, Ordering::Release);

        owner_stop.store(true, Ordering::Release);
        assert_eq!(
            hangar_mutation::FinalRemoveBatchControl::interruption_reason(&control),
            Some(hangar_mutation::FinalRemoveInterruptionReason::OwnerStop)
        );

        progress_observer_failed.store(true, Ordering::Release);
        assert_eq!(
            hangar_mutation::FinalRemoveBatchControl::interruption_reason(&control),
            Some(hangar_mutation::FinalRemoveInterruptionReason::ProgressObserverFailed)
        );
        assert!(hangar_mutation::FinalRemoveBatchControl::stop_requested(
            &control
        ));
    }

    #[test]
    fn stop_before_worker_returns_a_structured_cancelled_result() {
        let jobs = FinalRemoveJobStore::default();
        let summary = jobs.create(3).unwrap();

        jobs.request_stop(&summary.job_id).unwrap();
        let status = jobs.status(&summary.job_id).unwrap();
        assert_eq!(status.progress.phase, "finished");
        assert_eq!(status.progress.completed, 3);
        let result = status.result.expect("pre-start Stop must be terminal");
        assert_eq!(result.batch_id, summary.batch_id);
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.requested_objects, 3);
        assert_eq!(result.deleted_objects, 0);
        assert_eq!(result.kept_objects, 3);
        assert_eq!(result.failed_objects, 0);
        assert!(result.archive_retained);
        assert!(jobs.begin_worker(&summary.job_id).unwrap().is_none());
        assert!(jobs.active().is_none());
    }

    #[test]
    fn stop_after_worker_latches_and_progress_cannot_hide_it() {
        let jobs = FinalRemoveJobStore::default();
        let summary = jobs.create(4).unwrap();
        let signal = jobs
            .begin_worker(&summary.job_id)
            .unwrap()
            .expect("worker should start");
        jobs.update_progress(
            &summary.job_id,
            hangar_mutation::FinalRemoveBatchProgress {
                batch_id: summary.batch_id.clone(),
                phase: hangar_mutation::FinalRemoveBatchPhase::VerifyingArchives,
                total: 4,
                completed: 0,
                current_path: Some(r"C:\holding\one.bin".to_string()),
            },
        )
        .unwrap();

        jobs.request_stop(&summary.job_id).unwrap();
        assert!(signal.load(Ordering::Acquire));
        assert_eq!(
            jobs.status(&summary.job_id).unwrap().progress.phase,
            "stoppingAfterCurrentTopologyGroup"
        );

        jobs.update_progress(
            &summary.job_id,
            hangar_mutation::FinalRemoveBatchProgress {
                batch_id: summary.batch_id.clone(),
                phase: hangar_mutation::FinalRemoveBatchPhase::Deleting,
                total: 4,
                completed: 2,
                current_path: Some(r"C:\holding\two.bin".to_string()),
            },
        )
        .unwrap();
        let stopping = jobs.status(&summary.job_id).unwrap();
        assert_eq!(stopping.progress.phase, "stoppingAfterCurrentTopologyGroup");
        assert_eq!(stopping.progress.completed, 2);
        assert!(stopping.result.is_none());

        jobs.complete(
            &summary.job_id,
            hangar_mutation::FinalRemoveBatchResult {
                batch_id: summary.batch_id.clone(),
                status: "cancelled".to_string(),
                requested_objects: 4,
                deleted_objects: 2,
                kept_objects: 2,
                failed_objects: 0,
                projects: Vec::new(),
                volumes: Vec::new(),
                items: Vec::new(),
                archive_retained: true,
            },
        )
        .unwrap();
        let terminal = jobs.status(&summary.job_id).unwrap();
        assert_eq!(terminal.progress.phase, "finished");
        assert_eq!(terminal.result.unwrap().status, "cancelled");
    }

    #[test]
    fn final_remove_spawn_failure_is_terminal_and_never_runs_the_worker() {
        let jobs = FinalRemoveJobStore::default();
        let summary = jobs.create(2).unwrap();
        let worker_ran = Arc::new(AtomicBool::new(false));
        let worker_ran_in_task = Arc::clone(&worker_ran);

        let error = spawn_final_remove_worker_with(
            &jobs,
            &summary,
            Box::new(move || worker_ran_in_task.store(true, Ordering::Release)),
            |_worker| Err(std::io::Error::other("synthetic thread refusal")),
        )
        .unwrap_err();

        assert!(error.contains("could not start"), "{error}");
        assert!(!worker_ran.load(Ordering::Acquire));
        assert!(jobs.active().is_none());
        let terminal = jobs.status(&summary.job_id).unwrap_err();
        assert!(terminal.contains("could not start"), "{terminal}");
        assert!(
            jobs.create(1).is_ok(),
            "a failed spawn must not block the next final-cleanup admission"
        );
    }

    #[test]
    fn final_remove_worker_panic_is_observed_and_terminal() {
        let jobs = FinalRemoveJobStore::default();
        let summary = jobs.create(1).unwrap();
        let worker_jobs = jobs.clone();
        let worker_job_id = summary.job_id.clone();

        spawn_final_remove_worker(
            &jobs,
            &summary,
            Box::new(move || {
                assert!(worker_jobs.begin_worker(&worker_job_id).unwrap().is_some());
                panic!("synthetic final-cleanup worker panic");
            }),
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let terminal = loop {
            match jobs.status(&summary.job_id) {
                Err(error) => break error,
                Ok(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the panicked worker remained active"
                    );
                    std::thread::yield_now();
                }
            }
        };
        assert!(terminal.contains("unexpected internal panic"), "{terminal}");
        assert!(jobs.active().is_none());
        assert!(
            jobs.create(1).is_ok(),
            "a panicked worker must not leave the in-memory job store active"
        );
    }
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
    hangar_mutation::validate_local_mutation_path(Path::new(&path))
        .map_err(|error| error.to_string())?;
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
    expected_source_stamp: hangar_mutation::FileStamp,
    expected_source_hash: String,
}

// SAFE-08 exercises the real public mutation entrypoints. This thread-local test trap sits on the
// exact production boundary that starts filesystem revalidation of a reviewed source. If a cloud-classified node
// ever crosses the database gate, the test gets an observable I/O attempt instead of accepting a
// weaker assertion that the source merely still exists afterwards.
#[cfg(all(test, feature = "mutation"))]
std::thread_local! {
    static PLAN_SOURCE_INSPECTION_TRAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PLAN_SOURCE_INSPECTION_ATTEMPTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(all(test, feature = "mutation"))]
struct PlanSourceInspectionTrap;

#[cfg(all(test, feature = "mutation"))]
impl PlanSourceInspectionTrap {
    fn arm() -> Self {
        PLAN_SOURCE_INSPECTION_ATTEMPTS.with(|attempts| attempts.set(0));
        PLAN_SOURCE_INSPECTION_TRAP.with(|trap| {
            assert!(
                !trap.replace(true),
                "the source-inspection trap is already armed"
            )
        });
        Self
    }

    fn attempts(&self) -> u64 {
        PLAN_SOURCE_INSPECTION_ATTEMPTS.with(std::cell::Cell::get)
    }
}

#[cfg(all(test, feature = "mutation"))]
impl Drop for PlanSourceInspectionTrap {
    fn drop(&mut self) {
        PLAN_SOURCE_INSPECTION_TRAP.with(|trap| trap.set(false));
    }
}

#[cfg(feature = "mutation")]
fn validate_plan_candidate_source(source: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        let trapped = PLAN_SOURCE_INSPECTION_TRAP.with(std::cell::Cell::get);
        if trapped {
            PLAN_SOURCE_INSPECTION_ATTEMPTS
                .with(|attempts| attempts.set(attempts.get().saturating_add(1)));
            return Err(std::io::Error::other(
                "SAFE-08 detected a source I/O boundary after cloud classification",
            ));
        }
    }
    hangar_mutation::validate_local_mutation_path(source)
}

/// The result of re-validating a plan. Reparse entries never reach this
/// structure: link deletion is disabled until it has a faithful reversible
/// journal, so their presence blocks the whole request.
#[cfg(feature = "mutation")]
#[derive(Debug, Clone)]
struct PlanItems {
    current_plan: OperationPlan,
    items: Vec<ConcreteMutationItem>,
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
const FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT: &str = "ENABLE PERMANENT REMOVAL";

/// Owner-controlled permanent-removal capability. A DB read error fails closed.
/// There is deliberately no environment-variable bypass in the distributed app:
/// activation must be a durable, explicit in-app owner decision.
#[cfg(feature = "mutation")]
fn final_remove_persisted_enabled(state: &AppState) -> bool {
    state
        .db()
        .ok()
        .and_then(|db| db.final_remove_enabled_value().ok())
        .unwrap_or(false)
}

#[cfg(feature = "mutation")]
fn final_remove_runtime_enabled(state: &AppState) -> bool {
    !state.final_remove_disable_latch.load(Ordering::Acquire)
        && final_remove_persisted_enabled(state)
}

/// Read the owner-controlled permanent-removal capability flag.
#[cfg(feature = "mutation")]
pub fn mutation_final_remove_enabled(state: &AppState) -> bool {
    // UI truth is the durable, linearized setting. The internal disable latch
    // may already be refusing new work while an admitted worker finishes its
    // current topology group; do not report OFF until that boundary has exited
    // and the durable setting has actually been written.
    final_remove_persisted_enabled(state)
}

#[cfg(not(feature = "mutation"))]
pub fn mutation_final_remove_enabled(_state: &AppState) -> bool {
    false
}

#[cfg(feature = "mutation")]
fn require_final_remove_enabled(state: &AppState) -> Result<(), String> {
    if final_remove_runtime_enabled(state) {
        Ok(())
    } else {
        Err(
            "Permanent removal is off. Enable it explicitly in Recovery & cleanup before creating a final-removal preview."
                .to_string(),
        )
    }
}

/// Persist the owner-controlled capability. Enabling it requires a typed phrase.
/// Disabling is serialized with preview, confirmation and execution; after this
/// call returns no worker can still pass the capability check. Per-operation
/// immutable preview and fresh confirmation gates remain mandatory while enabled.
#[cfg(feature = "mutation")]
pub fn set_final_remove_enabled(
    state: &AppState,
    enabled: bool,
    acknowledgement: Option<String>,
) -> Result<(), String> {
    if enabled && acknowledgement.as_deref() != Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT) {
        return Err(format!(
            "Type {FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT} exactly to enable permanent removal."
        ));
    }
    if !enabled {
        // Publish the fail-closed intent before waiting for the shared mutation
        // boundary. A worker admitted concurrently must observe this latch at
        // its authoritative recheck, even if it wins the lock hand-off.
        state
            .final_remove_disable_latch
            .store(true, Ordering::Release);
        // A running engine observes this between topology groups. The write
        // boundary below then waits for the worker to leave its irreversible
        // critical section before durable OFF is reported to the caller.
        if let Some((job_id, _)) = state.final_remove_jobs.active() {
            let _ = state.final_remove_jobs.request_stop(&job_id);
        }
    }
    // This is the same process-wide write boundary held by the final-removal
    // worker. Once disabling returns, no worker can still be between its
    // authoritative flag check and an irreversible filesystem operation.
    let _inventory_guard = mutation_exclusive_guard(state)?;
    let persisted = state
        .db()?
        .set_final_remove_enabled(enabled)
        .map_err(to_message);
    if persisted.is_ok() && enabled {
        // Clear only after durable ON exists and while the execution boundary
        // is still exclusive. A failed enable therefore remains fail-closed.
        state
            .final_remove_disable_latch
            .store(false, Ordering::Release);
    }
    persisted
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
fn is_cloud_reparse_kind(kind: Option<&str>) -> bool {
    matches!(kind, Some("cloud_local" | "cloud_placeholder"))
}

#[cfg(feature = "mutation")]
fn concrete_items_for_plan(
    conn: &rusqlite::Connection,
    plan: &OperationPlan,
    include_protected: bool,
) -> Result<PlanItems, DbError> {
    hangar_db::ensure_relationship_inputs_stable(conn)?;
    let current = hangar_plan::build_operation_plan(conn, plan.target.node_id, &plan.action_label)
        .map_err(plan_error_to_db_error)?;
    if plan.schema != current.schema
        || !plan.read_only_preview
        || !current.read_only_preview
        || plan.target != current.target
    {
        return Err(DbError::FileRead(
            "Operation Plan envelope was altered. Rebuild the preview before mutation.".to_string(),
        ));
    }
    if current.target_fingerprint != plan.target_fingerprint {
        return Err(DbError::FileRead(
            "Operation Plan is stale. Rebuild the preview before entering mutation mode."
                .to_string(),
        ));
    }
    if !current.relationship_evidence_complete {
        return Err(DbError::FileRead(
            "Operation Plan relationship evidence is incomplete. Finish the inventory and relationship rebuild before mutation."
                .to_string(),
        ));
    }
    if current.partial_footprint {
        return Err(DbError::FileRead(
            "Operation Plan contains partial or opaque inventory evidence. Resume the scan before mutation."
                .to_string(),
        ));
    }

    let accounting = hangar_accounting::recoverable_for_target(conn, plan.target.node_id)
        .map_err(DbError::from)?;
    if !accounting.relationship_evidence_complete || accounting.summary.partial_footprint {
        return Err(DbError::FileRead(
            "Recoverable accounting is incomplete; mutation is blocked until every relevant subtree and relationship family is proven."
                .to_string(),
        ));
    }
    let mut issues = Vec::new();
    let mut items = Vec::new();
    let mut protected_paths = Vec::new();
    for candidate in accounting.candidates {
        // Cloud Files are never a protected-content opt-in. A materialized
        // `cloud_local` entry can have every byte on disk and still propagate a
        // local unlink to the provider; a `cloud_placeholder` can hydrate merely
        // by being opened. Keep either state as a plan-wide hard blocker.
        if is_cloud_reparse_kind(candidate.reparse_kind.as_deref()) {
            issues.push(issue(
                candidate.node_id,
                &candidate.path,
                "file is cloud-backed and cannot participate in a mutation",
            ));
            continue;
        }
        if candidate.is_reparse {
            issues.push(issue(
                candidate.node_id,
                &candidate.path,
                "reparse/symlink/junction removal is disabled until reversible link journaling exists",
            ));
            continue;
        }
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
                        volume_id, inode_key, reparse_kind, mtime
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
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            path,
            bytes,
            is_reparse,
            present,
            stored_volume_id,
            stored_inode_key,
            stored_reparse_kind,
            stored_mtime,
        )) = node
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
        if is_cloud_reparse_kind(stored_reparse_kind.as_deref()) {
            issues.push(issue(
                candidate.node_id,
                &path,
                "file is cloud-backed and cannot participate in a mutation",
            ));
            continue;
        }
        // Reparse points are never followed or unlinked. The mutation journal cannot
        // yet faithfully recreate every symlink/junction type and target, so their
        // presence blocks the entire request even with protected-content opt-in.
        if is_reparse {
            issues.push(issue(
                candidate.node_id,
                &path,
                "reparse/symlink/junction removal is disabled until reversible link journaling exists",
            ));
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
        if let Err(error) = validate_plan_candidate_source(&source) {
            issues.push(issue(
                candidate.node_id,
                &path,
                &format!("mutation path is not a proven local path: {error}"),
            ));
            continue;
        }
        let (Some(stored_volume), Some(stored_inode)) =
            (stored_volume_id.as_deref(), stored_inode_key.as_deref())
        else {
            issues.push(issue(
                candidate.node_id,
                &path,
                "reviewed inventory has no stable volume/file identity",
            ));
            continue;
        };
        let Some(stored_mtime) = stored_mtime
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok())
        else {
            issues.push(issue(
                candidate.node_id,
                &path,
                "reviewed inventory has no parseable modification-time proof",
            ));
            continue;
        };
        // This single no-follow/no-recall primitive binds every ancestor and
        // the leaf and derives identity, size, mtime and content hash from the
        // same open handle. Avoid path-following identity and separate lock
        // probes in the mutation authorization path.
        let (runtime_stamp, runtime_hash) =
            match hangar_mutation::inspect_local_mutation_file(&source) {
                Ok(proof) => proof,
                Err(error) => {
                    issues.push(issue(
                        candidate.node_id,
                        &path,
                        &format!("runtime handle proof failed: {error}"),
                    ));
                    continue;
                }
            };
        if stored_volume != runtime_stamp.volume_id
            || stored_inode != runtime_stamp.file_id
            || bytes != runtime_stamp.bytes
            || runtime_stamp.modified_unix_seconds != Some(stored_mtime)
        {
            issues.push(issue(
                candidate.node_id,
                &path,
                "file identity, size, or modification time changed since the preview was built",
            ));
            continue;
        }
        if is_sensitive {
            protected_paths.push(path.clone());
        }
        items.push(ConcreteMutationItem {
            source,
            relative: safe_relative(&candidate.path, &path),
            expected_source_stamp: runtime_stamp,
            expected_source_hash: runtime_hash,
        });
    }

    if !issues.is_empty() {
        return Err(DbError::FileRead(format_validation_issues(&issues)));
    }
    if items.is_empty() {
        return Err(DbError::FileRead(
            "Operation Plan has no concrete recoverable file items after revalidation.".to_string(),
        ));
    }
    Ok(PlanItems {
        current_plan: current,
        items,
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
fn prove_cleanup_root_absent_or_empty(root: &Path) -> Result<(), DbError> {
    hangar_mutation::validate_local_mutation_path(root)
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(DbError::FileRead(format!(
                "Cannot prove the cleanup root state for {}: {error}",
                root.display()
            )))
        }
    };
    let before = hangar_fs::inspect_path_identity(root);
    if before.inaccessible
        || before.is_reparse
        || is_cloud_reparse_kind(before.reparse_kind.as_deref())
        || !metadata.is_dir()
    {
        return Err(DbError::FileRead(format!(
            "Cleanup root {} is not a proven local plain directory; the project remains registered.",
            root.display()
        )));
    }
    let mut entries = fs::read_dir(root).map_err(|error| {
        DbError::FileRead(format!(
            "Cannot enumerate cleanup residuals under {}: {error}",
            root.display()
        ))
    })?;
    if let Some(entry) = entries.next() {
        let detail = entry
            .map(|entry| entry.path().display().to_string())
            .unwrap_or_else(|error| format!("unreadable entry: {error}"));
        return Err(DbError::FileRead(format!(
            "Cleanup root {} still contains residual content ({detail}); the project remains registered.",
            root.display()
        )));
    }
    let after = hangar_fs::inspect_path_identity(root);
    if after.inaccessible
        || after.is_reparse
        || before.volume_id.is_none()
        || before.inode_key.is_none()
        || before.volume_id != after.volume_id
        || before.inode_key != after.inode_key
    {
        return Err(DbError::FileRead(format!(
            "Cleanup root {} changed while residual absence was being proved; the project remains registered.",
            root.display()
        )));
    }
    Ok(())
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
    project_snapshot: SharedArc<Mutex<ProjectSnapshotCache>>,
    project_app_state_cache: SharedArc<Mutex<ProjectAppStateCache>>,
    jobs: JobStore,
    plan_jobs: PlanJobStore,
    dup_jobs: DupJobStore,
    project_discovery_source: ProjectDiscoverySource,
    safe_manage_jobs: safe_manage::SafeManageJobStore,
    safe_manage_discovery_source: safe_manage::SafeManageDiscoverySource,
    /// One process-wide proof boundary: inventory scans hold a read lock from
    /// estimate through finalization; plans and every disk mutation/recovery
    /// hold the write lock. This prevents a plan becoming ready between scan
    /// batches and then authorizing stale membership.
    inventory_mutation_gate: SharedArc<RwLock<()>>,
    #[cfg(feature = "mutation")]
    mutation_tokens: Arc<hangar_mutation::ConfirmTokenStore>,
    #[cfg(feature = "mutation")]
    final_remove_jobs: FinalRemoveJobStore,
    #[cfg(feature = "mutation")]
    final_remove_disable_latch: Arc<AtomicBool>,
    #[cfg(all(test, feature = "mutation"))]
    final_remove_worker_test_gate: SharedArc<RwLock<()>>,
    #[cfg(feature = "agent_automation")]
    automation_endpoint: SharedArc<Mutex<Option<String>>>,
    #[cfg(feature = "agent_automation")]
    ai_followups: SharedArc<Mutex<AiFollowUpStore>>,
    #[cfg(feature = "agent_automation")]
    ai_rewrite_proposals: SharedArc<Mutex<AiRewriteProposalStore>>,
    #[cfg(feature = "agent_automation")]
    ai_prepared_sends: SharedArc<Mutex<AiPreparedSendStore>>,
    /// Linearizes every provider send with key/binding/provider mutations. A send holds this
    /// from one-shot consume through transport completion; a mutation holds it through durable
    /// binding and Credential Manager changes.
    #[cfg(feature = "agent_automation")]
    ai_credential_operations: SharedArc<Mutex<()>>,
    #[cfg(feature = "agent_automation")]
    ai_safe_manage_contexts: SharedArc<Mutex<connector_advisory::SafeManageContextSelectionStore>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectDiscoveryScope {
    Global,
    Folder,
}

/// Selects the implementation behind the two public project-discovery entry
/// points. Production states always resolve one real system WSL snapshot; unit
/// tests may inject only controlled local WSL homes while the production
/// discovery implementation itself still runs.
#[derive(Clone, Default)]
enum ProjectDiscoverySource {
    #[default]
    System,
    #[cfg(test)]
    Fixture {
        load_wsl_homes:
            SharedArc<dyn Fn(ProjectDiscoveryScope) -> Vec<(String, PathBuf)> + Send + Sync>,
    },
}

impl ProjectDiscoverySource {
    fn snapshot(
        &self,
        wsl_enabled: bool,
        _scope: ProjectDiscoveryScope,
    ) -> hangar_discovery::WslDiscoverySnapshot {
        match self {
            Self::System => hangar_discovery::WslDiscoverySnapshot::system(wsl_enabled),
            #[cfg(test)]
            Self::Fixture { load_wsl_homes } => {
                if !wsl_enabled {
                    return hangar_discovery::WslDiscoverySnapshot::controlled(Vec::new());
                }
                hangar_discovery::WslDiscoverySnapshot::controlled(load_wsl_homes(_scope))
            }
        }
    }

    #[cfg(test)]
    fn fixture(
        load_wsl_homes: impl Fn(ProjectDiscoveryScope) -> Vec<(String, PathBuf)> + Send + Sync + 'static,
    ) -> Self {
        Self::Fixture {
            load_wsl_homes: SharedArc::new(load_wsl_homes),
        }
    }
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchStartRequest {
    pub preview_id: String,
    pub preview_digest: String,
    pub selected_topology_group_ids: Vec<String>,
    pub confirmation_token: String,
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchStartSummary {
    pub job_id: String,
    pub batch_id: String,
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchProgress {
    pub batch_id: String,
    pub phase: String,
    pub total: u64,
    pub completed: u64,
    pub current_path: Option<String>,
}

#[cfg(feature = "mutation")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchStatus {
    pub progress: FinalRemoveBatchProgress,
    pub result: Option<hangar_mutation::FinalRemoveBatchResult>,
}

#[cfg(feature = "mutation")]
#[derive(Clone, Default)]
struct FinalRemoveJobStore {
    inner: Arc<Mutex<HashMap<String, FinalRemoveJobRecord>>>,
    next_id: Arc<AtomicU64>,
}

#[cfg(feature = "mutation")]
#[derive(Clone)]
struct FinalRemoveJobRecord {
    progress: FinalRemoveBatchProgress,
    result: Option<hangar_mutation::FinalRemoveBatchResult>,
    error: Option<String>,
    worker_started: bool,
    stop_requested: Arc<AtomicBool>,
}

#[cfg(feature = "mutation")]
impl FinalRemoveJobStore {
    fn create(&self, total: u64) -> Result<FinalRemoveBatchStartSummary, String> {
        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "System clock is unavailable for the local job id.".to_string())?
            .as_nanos();
        let job_id = format!(
            "final-remove-job-{:x}-{:x}-{:x}",
            std::process::id(),
            created,
            sequence
        );
        let batch_id = format!("final-remove-batch-{created:x}-{sequence:x}");
        let record = FinalRemoveJobRecord {
            progress: FinalRemoveBatchProgress {
                batch_id: batch_id.clone(),
                phase: "waitingForUac".to_string(),
                total,
                completed: 0,
                current_path: None,
            },
            result: None,
            error: None,
            worker_started: false,
            stop_requested: Arc::new(AtomicBool::new(false)),
        };
        let mut jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        if jobs
            .values()
            .any(|job| job.result.is_none() && job.error.is_none())
        {
            return Err("A final-cleanup batch is already active.".to_string());
        }
        if jobs.len() >= 128 {
            let mut terminal = jobs
                .iter()
                .filter(|(_, job)| job.result.is_some() || job.error.is_some())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            terminal.sort_unstable();
            for id in terminal.into_iter().take(jobs.len().saturating_sub(127)) {
                jobs.remove(&id);
            }
        }
        jobs.insert(job_id.clone(), record);
        Ok(FinalRemoveBatchStartSummary { job_id, batch_id })
    }

    fn begin_worker(&self, job_id: &str) -> Result<Option<Arc<AtomicBool>>, String> {
        let mut jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Final-cleanup job was not found.".to_string())?;
        if job.result.is_some() || job.error.is_some() {
            return Ok(None);
        }
        job.worker_started = true;
        // The engine owns the phase transition. Until its first authenticated
        // progress event, Windows approval is still the truthful visible state.
        Ok(Some(Arc::clone(&job.stop_requested)))
    }

    fn update_progress(
        &self,
        job_id: &str,
        progress: hangar_mutation::FinalRemoveBatchProgress,
    ) -> Result<(), String> {
        let mut jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Final-cleanup job was not found.".to_string())?;
        if job.result.is_some() || job.error.is_some() {
            return Ok(());
        }
        if progress.batch_id != job.progress.batch_id
            || progress.total != job.progress.total
            || progress.completed > progress.total
            || progress.completed < job.progress.completed
        {
            return Err(
                "Final-cleanup engine progress changed immutable identity or regressed its counts."
                    .to_string(),
            );
        }
        job.progress.completed = progress.completed;
        job.progress.current_path = progress.current_path;
        // A terminal phase is published atomically with its structured result by
        // `complete`; otherwise the frontend could briefly observe `finished`
        // without the result that proves object counts and archive retention.
        if !matches!(
            progress.phase,
            hangar_mutation::FinalRemoveBatchPhase::Finished
                | hangar_mutation::FinalRemoveBatchPhase::Interrupted
        ) {
            job.progress.phase = if job.stop_requested.load(Ordering::Acquire) {
                "stoppingAfterCurrentTopologyGroup".to_string()
            } else {
                progress.phase.as_str().to_string()
            };
        }
        Ok(())
    }

    fn complete(
        &self,
        job_id: &str,
        result: hangar_mutation::FinalRemoveBatchResult,
    ) -> Result<(), String> {
        let mut jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Final-cleanup job was not found.".to_string())?;
        if job.result.is_some() {
            return Ok(());
        }
        if result.batch_id != job.progress.batch_id
            || result.requested_objects != job.progress.total
            || result
                .deleted_objects
                .checked_add(result.kept_objects)
                .and_then(|count| count.checked_add(result.failed_objects))
                != Some(result.requested_objects)
            || !result.archive_retained
        {
            return Err(
                "Final-cleanup engine returned a result with inconsistent identity, counts or archive retention."
                    .to_string(),
            );
        }
        job.progress.completed = result
            .deleted_objects
            .saturating_add(result.kept_objects)
            .saturating_add(result.failed_objects)
            .min(job.progress.total);
        job.progress.phase = if result.status == "interrupted" {
            "interrupted"
        } else {
            "finished"
        }
        .to_string();
        job.result = Some(result);
        Ok(())
    }

    fn fail(&self, job_id: &str, message: String) {
        if let Ok(mut jobs) = self.inner.lock() {
            if let Some(job) = jobs.get_mut(job_id) {
                if job.result.is_some() {
                    return;
                }
                job.progress.phase = "interrupted".to_string();
                job.error = Some(message);
            }
        }
    }

    fn status(&self, job_id: &str) -> Result<FinalRemoveBatchStatus, String> {
        let jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        let job = jobs
            .get(job_id)
            .ok_or_else(|| "Final-cleanup job was not found.".to_string())?;
        if let Some(error) = &job.error {
            return Err(error.clone());
        }
        Ok(FinalRemoveBatchStatus {
            progress: job.progress.clone(),
            result: job.result.clone(),
        })
    }

    fn request_stop(&self, job_id: &str) -> Result<(), String> {
        let mut jobs = self
            .inner
            .lock()
            .map_err(|_| "Final-cleanup job state is unavailable.".to_string())?;
        let job = jobs
            .get_mut(job_id)
            .ok_or_else(|| "Final-cleanup job was not found.".to_string())?;
        if job.result.is_some() || job.error.is_some() {
            return Ok(());
        }
        job.stop_requested.store(true, Ordering::Release);
        if job.worker_started {
            job.progress.phase = "stoppingAfterCurrentTopologyGroup".to_string();
            return Ok(());
        }
        let requested = job.progress.total;
        job.progress.completed = requested;
        job.progress.current_path = None;
        job.progress.phase = "finished".to_string();
        job.result = Some(hangar_mutation::FinalRemoveBatchResult {
            batch_id: job.progress.batch_id.clone(),
            status: "cancelled".to_string(),
            requested_objects: requested,
            deleted_objects: 0,
            kept_objects: requested,
            failed_objects: 0,
            projects: Vec::new(),
            volumes: Vec::new(),
            items: Vec::new(),
            archive_retained: true,
        });
        Ok(())
    }

    fn active(&self) -> Option<(String, FinalRemoveBatchProgress)> {
        self.inner.lock().ok().and_then(|jobs| {
            jobs.iter().find_map(|(id, job)| {
                (job.result.is_none() && job.error.is_none())
                    .then(|| (id.clone(), job.progress.clone()))
            })
        })
    }

    #[cfg(test)]
    fn worker_started(&self, job_id: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|jobs| jobs.get(job_id).map(|job| job.worker_started))
            .unwrap_or(false)
    }
}

type ProjectAppStates = std::collections::HashMap<String, hangar_discovery::ProjectAppState>;

#[derive(Default)]
struct ProjectAppStateCache {
    loaded_at: Option<Instant>,
    states: ProjectAppStates,
}

#[derive(Default)]
struct ProjectSnapshotCache {
    generation: u64,
    projects: Option<Vec<ProjectSummary>>,
}

impl ProjectAppStateCache {
    fn get_or_load(
        &mut self,
        now: Instant,
        ttl: Duration,
        load: impl FnOnce() -> ProjectAppStates,
    ) -> ProjectAppStates {
        let fresh = self
            .loaded_at
            .and_then(|loaded_at| now.checked_duration_since(loaded_at))
            .is_some_and(|age| age < ttl);
        if fresh {
            return self.states.clone();
        }

        let states = load();
        self.loaded_at = Some(now);
        self.states.clone_from(&states);
        states
    }

    fn invalidate(&mut self) {
        self.loaded_at = None;
        self.states.clear();
    }
}

fn project_disk_cache_json(projects: &[ProjectSummary]) -> Option<Vec<u8>> {
    let snapshot: Vec<ProjectSummary> = projects.iter().take(200).cloned().collect();
    let json = serde_json::to_vec(&snapshot).ok()?;
    if json.len() <= PROJECT_DISK_CACHE_MAX_JSON_BYTES {
        Some(json)
    } else {
        // Keep the full process snapshot, but fail closed on the bounded cold-start
        // artifact instead of retaining an older set of project ids on disk.
        Some(b"[]".to_vec())
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiPreparedKind {
    Read,
    Walkthrough,
    FollowUp,
    ChangeNarration,
    ChangeLearning,
    ChangeReview,
    Rewrite,
    ProjectSummary,
    SafeManageAdvisory,
    ProviderTest,
    ProviderModels,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug)]
struct PendingAiSend {
    kind: AiPreparedKind,
    request: hangar_ai::PreparedRequest,
    credential_binding: Option<hangar_core::AiProviderCredentialBinding>,
    created_at: Instant,
    /// Receipt and selected context references exist only for the Connector
    /// Safe Manage advisory. They remain process-local capabilities/locators.
    receipt_id: Option<String>,
    selected_context_ids: Vec<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, Default)]
struct AiPreparedSendStore {
    requests: HashMap<String, PendingAiSend>,
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

/// A DB-independent first response to a Windows file association. It contains
/// only the explicitly requested local file and its immediate Viewer boundary;
/// catalog registration and project attribution happen after this preview has
/// painted.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectFilePreview {
    pub input_path: String,
    pub viewer_root: String,
    pub preview: FilePreview,
}

impl AppState {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, String> {
        let db_path_ref = db_path.as_ref();
        let state = Self {
            db: SharedArc::new(DbSlot::starting("Opening encrypted local inventory.")),
            db_path: db_path_ref.to_path_buf(),
            project_cache_path: db_path_ref.with_extension("projects.dpapi"),
            discovery_cache_path: db_path_ref.with_extension("discovery.dpapi"),
            project_snapshot: SharedArc::new(Mutex::new(ProjectSnapshotCache::default())),
            project_app_state_cache: SharedArc::new(Mutex::new(ProjectAppStateCache::default())),
            jobs: JobStore::default(),
            plan_jobs: PlanJobStore::default(),
            dup_jobs: DupJobStore::default(),
            project_discovery_source: ProjectDiscoverySource::default(),
            safe_manage_jobs: safe_manage::SafeManageJobStore::default(),
            safe_manage_discovery_source: safe_manage::SafeManageDiscoverySource::default(),
            inventory_mutation_gate: SharedArc::new(RwLock::new(())),
            #[cfg(feature = "mutation")]
            mutation_tokens: Arc::new(hangar_mutation::ConfirmTokenStore::default()),
            #[cfg(feature = "mutation")]
            final_remove_jobs: FinalRemoveJobStore::default(),
            #[cfg(feature = "mutation")]
            final_remove_disable_latch: Arc::new(AtomicBool::new(false)),
            #[cfg(all(test, feature = "mutation"))]
            final_remove_worker_test_gate: SharedArc::new(RwLock::new(())),
            #[cfg(feature = "agent_automation")]
            automation_endpoint: SharedArc::new(Mutex::new(None)),
            #[cfg(feature = "agent_automation")]
            ai_followups: SharedArc::new(Mutex::new(AiFollowUpStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_rewrite_proposals: SharedArc::new(Mutex::new(AiRewriteProposalStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_prepared_sends: SharedArc::new(Mutex::new(AiPreparedSendStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_credential_operations: SharedArc::new(Mutex::new(())),
            #[cfg(feature = "agent_automation")]
            ai_safe_manage_contexts: SharedArc::new(Mutex::new(
                connector_advisory::SafeManageContextSelectionStore::default(),
            )),
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

    /// Open a persistent test inventory with an immutable Safe Manage discovery
    /// report. This constructor exists only in debug builds so native integration
    /// tests cannot accidentally enumerate the host's real coding-tool stores;
    /// release builds have only the dedicated discovery source used by `open`.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn open_with_safe_manage_discovery_fixture_for_test(
        db_path: impl AsRef<Path>,
        report: ProjectDiscoveryReport,
    ) -> Result<Self, String> {
        let mut state = Self::open(db_path)?;
        state.safe_manage_discovery_source =
            safe_manage::SafeManageDiscoverySource::fixture(report);
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
            project_snapshot: SharedArc::new(Mutex::new(ProjectSnapshotCache::default())),
            project_app_state_cache: SharedArc::new(Mutex::new(ProjectAppStateCache::default())),
            jobs: JobStore::default(),
            plan_jobs: PlanJobStore::default(),
            dup_jobs: DupJobStore::default(),
            project_discovery_source: ProjectDiscoverySource::default(),
            safe_manage_jobs: safe_manage::SafeManageJobStore::default(),
            safe_manage_discovery_source: safe_manage::SafeManageDiscoverySource::default(),
            inventory_mutation_gate: SharedArc::new(RwLock::new(())),
            #[cfg(feature = "mutation")]
            mutation_tokens: Arc::new(hangar_mutation::ConfirmTokenStore::default()),
            #[cfg(feature = "mutation")]
            final_remove_jobs: FinalRemoveJobStore::default(),
            #[cfg(feature = "mutation")]
            final_remove_disable_latch: Arc::new(AtomicBool::new(false)),
            #[cfg(all(test, feature = "mutation"))]
            final_remove_worker_test_gate: SharedArc::new(RwLock::new(())),
            #[cfg(feature = "agent_automation")]
            automation_endpoint: SharedArc::new(Mutex::new(None)),
            #[cfg(feature = "agent_automation")]
            ai_followups: SharedArc::new(Mutex::new(AiFollowUpStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_rewrite_proposals: SharedArc::new(Mutex::new(AiRewriteProposalStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_prepared_sends: SharedArc::new(Mutex::new(AiPreparedSendStore::default())),
            #[cfg(feature = "agent_automation")]
            ai_credential_operations: SharedArc::new(Mutex::new(())),
            #[cfg(feature = "agent_automation")]
            ai_safe_manage_contexts: SharedArc::new(Mutex::new(
                connector_advisory::SafeManageContextSelectionStore::default(),
            )),
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
        let mut snapshot = self
            .project_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(projects) = snapshot.projects.as_ref() {
            return projects.clone();
        }

        let projects = self.read_project_cache_from_disk();
        snapshot.projects = Some(projects.clone());
        projects
    }

    fn read_project_cache_from_disk(&self) -> Vec<ProjectSummary> {
        if self.project_cache_path.as_os_str().is_empty() {
            return Vec::new();
        }
        let file = match fs::File::open(&self.project_cache_path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };
        let mut protected = Vec::new();
        if file
            .take(PROJECT_DISK_CACHE_MAX_BYTES + 1)
            .read_to_end(&mut protected)
            .is_err()
            || protected.len() as u64 > PROJECT_DISK_CACHE_MAX_BYTES
        {
            return Vec::new();
        }
        let json = match hangar_security::unprotect_local_bytes(&protected) {
            Ok(bytes) => bytes,
            Err(_) => return Vec::new(),
        };
        serde_json::from_slice::<Vec<ProjectSummary>>(&json).unwrap_or_default()
    }

    fn write_project_cache_if_generation(&self, projects: &[ProjectSummary], generation: u64) {
        let mut snapshot = self
            .project_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if snapshot.generation != generation {
            return;
        }
        snapshot.projects = Some(projects.to_vec());
        self.write_project_cache_to_disk(projects);
    }

    fn write_project_cache_to_disk(&self, projects: &[ProjectSummary]) {
        if self.project_cache_path.as_os_str().is_empty() {
            return;
        }
        let Some(json) = project_disk_cache_json(projects) else {
            return;
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

    fn project_cache_generation(&self) -> u64 {
        self.project_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn project_app_states(&self) -> ProjectAppStates {
        self.project_app_state_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_load(
                Instant::now(),
                PROJECT_APP_STATE_CACHE_TTL,
                hangar_discovery::project_app_states,
            )
    }

    fn invalidate_project_app_state_cache(&self) {
        self.project_app_state_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
    }

    fn invalidate_project_caches(&self) {
        self.invalidate_project_app_state_cache();
        // The on-disk snapshot is a cold-start optimization, not an authority. Replace
        // it with an encrypted empty snapshot after catalog mutation so a crash before
        // the next project-list refresh cannot resurrect deleted ids on restart.
        let mut snapshot = self
            .project_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generation = snapshot.generation.saturating_add(1);
        snapshot.projects = Some(Vec::new());
        self.write_project_cache_to_disk(&[]);
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
struct AutomationProjectNodeParams {
    project_id: i64,
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

/// Params for a final-removal review recommendation: only a prior holding-area
/// entry id. This identifies what Recovery should review; it is not an executor.
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

#[cfg(feature = "agent_automation")]
fn require_final_remove_recommendation_enabled(db: &Db) -> Result<(), String> {
    if db.final_remove_enabled_value().map_err(to_message)? {
        Ok(())
    } else {
        Err(
            "Permanent removal is off. Connected apps cannot recommend it, and the local preview/confirmation flow remains unavailable until the owner enables it in Recovery & cleanup."
                .to_string(),
        )
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
        granted_nodes.contains(&edge.source_node_id)
            && granted_nodes.contains(&edge.target_node_id)
            && edge
                .source_project_id
                .is_none_or(|project_id| granted_project_ids.contains(&project_id))
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
    // Keep active display names distinct for clear UI/audit attribution. This is
    // not an authority boundary: comment ownership and transport authorization
    // are keyed to each agent's immutable identity_id.
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
    if agent.agent_kind != AutomationAgentKind::LocalTool
        || agent.allowed_transport != AutomationTransport::NamedPipe
        || agent.connected_host.is_some()
    {
        return Err(
            "Temporary file-body grants are available only to trusted named-pipe local tools, never connected apps."
                .to_string(),
        );
    }
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
                return Err(format!(
                    "The connected-app recommendation for held entry {entry_id} cannot approve or start permanent removal. Open Recovery & cleanup and review the immutable project/batch preview locally instead."
                ));
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

/// Default one-click access is deliberately body-free and non-mutating. History
/// search and mutation *requests* are separate opt-ins on the next Connect/Reconnect;
/// neither option adds an MCP file-body or direct-execution tool.
#[cfg(feature = "agent_automation")]
const CONNECTED_APP_SCOPES: &[&str] = &[
    "comments_read",
    "comments_write",
    "read_structure",
    "read_graph",
];

#[cfg(feature = "agent_automation")]
fn connected_app_scopes(
    include_history_search: bool,
    include_mutation_requests: bool,
) -> Vec<String> {
    let mut scopes = CONNECTED_APP_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect::<Vec<_>>();
    if include_history_search {
        scopes.push("history_search".to_string());
    }
    if include_mutation_requests {
        scopes.push("execute_plan".to_string());
    }
    scopes
}

#[cfg(feature = "agent_automation")]
fn connected_app_home() -> Result<PathBuf, String> {
    hangar_appconfig::user_home()
        .ok_or_else(|| "Could not resolve your Windows home directory.".to_string())
}

#[cfg(feature = "agent_automation")]
fn resolve_connected_app_host(host_id: &str) -> Result<hangar_appconfig::Host, String> {
    hangar_appconfig::Host::from_id(host_id).ok_or_else(|| format!("Unknown AI app: {host_id}."))
}

#[cfg(feature = "agent_automation")]
fn core_connected_app_host(host: hangar_appconfig::Host) -> ConnectedAppHost {
    match host {
        hangar_appconfig::Host::Claude => ConnectedAppHost::Claude,
        hangar_appconfig::Host::Cursor => ConnectedAppHost::Cursor,
        hangar_appconfig::Host::Codex => ConnectedAppHost::Codex,
    }
}

#[cfg(feature = "agent_automation")]
static CONNECTED_APP_HOST_LOCKS: OnceLock<[Mutex<()>; 3]> = OnceLock::new();

#[cfg(feature = "agent_automation")]
fn connected_app_host_lock(host: hangar_appconfig::Host) -> &'static Mutex<()> {
    let locks =
        CONNECTED_APP_HOST_LOCKS.get_or_init(|| [Mutex::new(()), Mutex::new(()), Mutex::new(())]);
    match host {
        hangar_appconfig::Host::Claude => &locks[0],
        hangar_appconfig::Host::Cursor => &locks[1],
        hangar_appconfig::Host::Codex => &locks[2],
    }
}

#[cfg(feature = "agent_automation")]
fn db_file_fingerprint(
    value: &hangar_appconfig::FileFingerprint,
) -> hangar_db::ConnectedAppFileFingerprint {
    hangar_db::ConnectedAppFileFingerprint {
        exists: value.exists,
        hash: value.hash.clone(),
    }
}

#[cfg(feature = "agent_automation")]
fn db_fs_contract(
    value: &hangar_appconfig::ChangeFingerprints,
) -> hangar_db::ConnectedAppFsContract {
    hangar_db::ConnectedAppFsContract {
        config_before: db_file_fingerprint(&value.config_before),
        config_after: db_file_fingerprint(&value.config_after),
        backup_before: db_file_fingerprint(&value.backup_before),
        backup_after: db_file_fingerprint(&value.backup_after),
        backup_changed: value.backup_changed,
        state_before: db_file_fingerprint(&value.state_before),
        state_after: db_file_fingerprint(&value.state_after),
        state_changed: value.state_changed,
    }
}

#[cfg(feature = "agent_automation")]
fn appconfig_file_fingerprint(
    value: &hangar_db::ConnectedAppFileFingerprint,
) -> hangar_appconfig::FileFingerprint {
    hangar_appconfig::FileFingerprint {
        exists: value.exists,
        hash: value.hash.clone(),
    }
}

#[cfg(feature = "agent_automation")]
fn appconfig_fs_contract(
    value: &hangar_db::ConnectedAppFsContract,
) -> hangar_appconfig::ChangeFingerprints {
    hangar_appconfig::ChangeFingerprints {
        config_before: appconfig_file_fingerprint(&value.config_before),
        config_after: appconfig_file_fingerprint(&value.config_after),
        backup_before: appconfig_file_fingerprint(&value.backup_before),
        backup_after: appconfig_file_fingerprint(&value.backup_after),
        backup_changed: value.backup_changed,
        state_before: appconfig_file_fingerprint(&value.state_before),
        state_after: appconfig_file_fingerprint(&value.state_after),
        state_changed: value.state_changed,
    }
}

/// Idempotently close a crash window before any status/register/remove decision.
/// Exact-before (or an externally absent non-after config) aborts; exact-after
/// completes; anything else is ambiguous and is left untouched. Cross-store
/// atomicity is not available, so this explicit journal + compensation is the
/// durable contract.
#[cfg(feature = "agent_automation")]
fn reconcile_connected_app_host(
    db: &Db,
    host: hangar_appconfig::Host,
    home: &Path,
) -> Result<(), String> {
    let Some(change) = db.connected_app_change(host.id()).map_err(to_message)? else {
        if hangar_appconfig::pending_sidecars_present(host, home)? {
            return Err(
                "A connected-app sidecar has no matching encrypted journal; it was left untouched."
                    .to_string(),
            );
        }
        return Ok(());
    };
    let current = hangar_appconfig::config_fingerprint(host, home)?;
    let fs = appconfig_fs_contract(&change.fs);
    let is_before = current == fs.config_before;
    let is_after = current == fs.config_after;
    // Absence is an explicit abort image unless it is itself the journal's exact
    // before/after image. Never reconstruct a removed config from hashes: retain
    // the absent owner-controlled state, roll back only exact sidecars and keep or
    // restore the prior DB credential.
    let is_absent_abort = !current.exists && !is_before && !is_after;
    if (!is_before && !is_after && !is_absent_abort) || (is_before && is_after) {
        return Err(
            "The connected-app config is ambiguous or changed; recovery stopped without overwriting it."
                .to_string(),
        );
    }
    match (change.state.as_str(), is_before, is_after) {
        ("prepared", true, false) => {
            hangar_appconfig::recover_change(host, home, &fs, false)?;
            if !db
                .connected_app_change_abort_prepared(host.id(), &change.operation_id)
                .map_err(to_message)?
            {
                return Err("The prepared connected-app journal could not be aborted.".to_string());
            }
        }
        ("prepared", false, true) => {
            db.connected_app_change_commit(host.id(), &change.operation_id)
                .map_err(to_message)?;
            hangar_appconfig::recover_change(host, home, &fs, true)?;
            db.connected_app_change_finalize(host.id(), &change.operation_id)
                .map_err(to_message)?;
        }
        ("prepared", false, false) if is_absent_abort => {
            hangar_appconfig::recover_sidecars(host, home, &fs, false)?;
            if !db
                .connected_app_change_abort_prepared(host.id(), &change.operation_id)
                .map_err(to_message)?
            {
                return Err("The prepared connected-app journal could not be aborted.".to_string());
            }
        }
        ("committed", false, true) => {
            hangar_appconfig::recover_change(host, home, &fs, true)?;
            db.connected_app_change_finalize(host.id(), &change.operation_id)
                .map_err(to_message)?;
        }
        ("committed", true, false) => {
            hangar_appconfig::recover_change(host, home, &fs, false)?;
            db.connected_app_change_rollback_committed(host.id(), &change.operation_id)
                .map_err(to_message)?;
        }
        ("committed", false, false) if is_absent_abort => {
            hangar_appconfig::recover_sidecars(host, home, &fs, false)?;
            db.connected_app_change_rollback_committed(host.id(), &change.operation_id)
                .map_err(to_message)?;
        }
        _ => {
            return Err(
                "The connected-app journal has an unsupported state and was left untouched."
                    .to_string(),
            )
        }
    }
    Ok(())
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

#[cfg(feature = "agent_automation")]
fn connected_app_effective_status(
    db: &Db,
    host: hangar_appconfig::Host,
    home: &Path,
    recovery_required: bool,
) -> Result<hangar_appconfig::HostStatus, String> {
    let inspection = hangar_appconfig::inspect(host, home);
    let mut status = inspection.status;
    status.recovery_required = recovery_required;
    let durable = db
        .connected_app_agent(core_connected_app_host(host))
        .map_err(to_message)?;
    if let Some(agent) = durable.as_ref() {
        status.durable_agent_id = Some(agent.id);
        status.durable_identity_id = Some(agent.identity_id.clone());
        status.durable_credential_enabled = agent.enabled;
    }
    if !recovery_required && status.readable && status.registered {
        if let Some(token_hash) = inspection.configured_token_hash.as_deref() {
            if let Some(agent) = db
                .connected_app_effective_agent(host.id(), token_hash)
                .map_err(to_message)?
            {
                if durable
                    .as_ref()
                    .is_some_and(|durable| durable.identity_id == agent.identity_id)
                {
                    status.effective_scopes = agent.scopes;
                    status.effective_project_ids = agent.project_ids;
                    status.credential_active = true;
                }
            }
        }
    }
    if status.durable_credential_enabled && !status.credential_active {
        status.credential_orphaned = true;
        status.orphan_reason = Some(if recovery_required {
            "A durable credential remains while a prior config transaction needs recovery. Revoke it DB-only if this app is no longer trusted."
                .to_string()
        } else if !status.config_exists {
            "The external config is missing, but its durable credential still exists. It can be revoked without recreating the config."
                .to_string()
        } else if !status.readable {
            "The external config is malformed or unreadable, but its durable credential can still be revoked DB-only."
                .to_string()
        } else if !status.registered {
            "The Code Hangar entry is absent from the external config, but a durable credential remains."
                .to_string()
        } else {
            "The external entry no longer matches the immutable durable credential. Reconnect or revoke it DB-only."
                .to_string()
        });
    }
    Ok(status)
}

/// Status of every supported AI app's config and its actually effective DB
/// grants. A host with ambiguous recovery is reported as such without blocking
/// the other hosts or overwriting any file.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_status(state: &AppState) -> Result<Vec<hangar_appconfig::HostStatus>, String> {
    let home = connected_app_home()?;
    let db = state.db()?;
    let mut statuses = Vec::new();
    for host in hangar_appconfig::Host::ALL {
        let guard = connected_app_host_lock(host)
            .lock()
            .map_err(|_| "The connected-app host lock is unavailable.".to_string())?;
        let _os_guard =
            hangar_appconfig::acquire_host_operation_lock(host, &home, "status-reconcile")?;
        let recovery_required = reconcile_connected_app_host(&db, host, &home).is_err();
        statuses.push(connected_app_effective_status(
            &db,
            host,
            &home,
            recovery_required,
        )?);
        drop(guard);
    }
    Ok(statuses)
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
    include_history_search: bool,
    include_mutation_requests: bool,
) -> Result<hangar_appconfig::HostStatus, String> {
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    mcp_appconfig_register_at(
        state,
        host,
        &home,
        None,
        project_ids,
        include_history_search,
        include_mutation_requests,
    )
}

#[cfg(feature = "agent_automation")]
fn mcp_appconfig_register_at(
    state: &AppState,
    host: hangar_appconfig::Host,
    home: &Path,
    server_path_override: Option<&Path>,
    project_ids: Vec<i64>,
    include_history_search: bool,
    include_mutation_requests: bool,
) -> Result<hangar_appconfig::HostStatus, String> {
    let db = state.db()?;
    let agent_name = host.label();
    let _guard = connected_app_host_lock(host)
        .lock()
        .map_err(|_| "The connected-app host lock is unavailable.".to_string())?;
    let operation_id = hangar_agent::random_token(18)?;
    let _os_guard = hangar_appconfig::acquire_host_operation_lock(host, home, &operation_id)?;
    reconcile_connected_app_host(&db, host, home)?;
    if project_ids.is_empty() {
        return Err("Choose at least one project before connecting an AI app.".to_string());
    }

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
    let server_path = match server_path_override {
        Some(path) => path.to_path_buf(),
        None => connected_app_server_path()?,
    };

    let durable_binding = db
        .connected_app_binding_ensure(core_connected_app_host(host))
        .map_err(to_message)?;
    let registration_binding = hangar_appconfig::RegistrationBinding::from_hex(
        &durable_binding.agent_identity_id,
        &durable_binding.state_auth_key_hex,
    )?;

    let token = hangar_agent::random_token(32)?;
    let token_hash = automation_token_hash(&token);
    let scopes = connected_app_scopes(include_history_search, include_mutation_requests);

    let mut env = vec![
        ("CODEHANGAR_MCP_TOKEN".to_string(), token),
        ("CODEHANGAR_MCP_HOST".to_string(), host.id().to_string()),
        (
            "CODEHANGAR_MCP_AGENT_ID".to_string(),
            durable_binding.agent_identity_id.clone(),
        ),
    ];
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

    let prepared =
        hangar_appconfig::prepare_register_authenticated(host, home, &spec, &registration_binding)?;
    let change = db
        .connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id,
            kind: "register".to_string(),
            agent_name: agent_name.to_string(),
            new_token_hash: Some(token_hash),
            new_scopes: scopes,
            new_project_ids: project_ids,
            fs: db_fs_contract(prepared.fingerprints()),
        })
        .map_err(to_message)?;
    if let Err(error) = prepared.apply() {
        if prepared.can_abort_after_failed_apply().unwrap_or(false) {
            let _ = db.connected_app_change_abort_prepared(host.id(), &change.operation_id);
            return Err(error);
        }
        let recovery = reconcile_connected_app_host(&db, host, home);
        return Err(match recovery {
            Ok(()) => error,
            Err(_) => format!(
                "{error} The prior credential remains authoritative until connected-app recovery completes."
            ),
        });
    }
    if let Err(error) = db.connected_app_change_commit(host.id(), &change.operation_id) {
        if prepared.rollback().is_ok() {
            let _ = db.connected_app_change_abort_prepared(host.id(), &change.operation_id);
        }
        return Err(format!(
            "The replacement credential could not be committed: {}",
            to_message(error)
        ));
    }
    prepared.finalize()?;
    db.connected_app_change_finalize(host.id(), &change.operation_id)
        .map_err(to_message)?;
    connected_app_effective_status(&db, host, home, false)
}

/// Remove Code Hangar's connector from one AI app's config and revoke its token.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_remove(
    state: &AppState,
    host_id: String,
) -> Result<hangar_appconfig::HostStatus, String> {
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    mcp_appconfig_remove_at(state, host, &home)
}

#[cfg(feature = "agent_automation")]
fn mcp_appconfig_remove_at(
    state: &AppState,
    host: hangar_appconfig::Host,
    home: &Path,
) -> Result<hangar_appconfig::HostStatus, String> {
    let db = state.db()?;
    let agent_name = host.label();
    let _guard = connected_app_host_lock(host)
        .lock()
        .map_err(|_| "The connected-app host lock is unavailable.".to_string())?;
    let operation_id = hangar_agent::random_token(18)?;
    let _os_guard = hangar_appconfig::acquire_host_operation_lock(host, home, &operation_id)?;
    reconcile_connected_app_host(&db, host, home)?;
    let durable_binding = db
        .connected_app_binding(core_connected_app_host(host))
        .map_err(to_message)?
        .ok_or_else(|| {
            "This external entry has no authenticated Code Hangar binding and was left untouched. Reconnect it to adopt a fresh binding, or revoke any orphaned credential DB-only."
                .to_string()
        })?;
    let registration_binding = hangar_appconfig::RegistrationBinding::from_hex(
        &durable_binding.agent_identity_id,
        &durable_binding.state_auth_key_hex,
    )?;
    let Some(prepared) =
        hangar_appconfig::prepare_unregister_authenticated(host, home, &registration_binding)?
    else {
        return connected_app_effective_status(&db, host, home, false);
    };
    let change = db
        .connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id,
            kind: "remove".to_string(),
            agent_name: agent_name.to_string(),
            new_token_hash: None,
            new_scopes: Vec::new(),
            new_project_ids: Vec::new(),
            fs: db_fs_contract(prepared.fingerprints()),
        })
        .map_err(to_message)?;
    if let Err(error) = prepared.apply() {
        if prepared.can_abort_after_failed_apply().unwrap_or(false) {
            let _ = db.connected_app_change_abort_prepared(host.id(), &change.operation_id);
            return Err(error);
        }
        let recovery = reconcile_connected_app_host(&db, host, home);
        return Err(match recovery {
            Ok(()) => error,
            Err(_) => format!(
                "{error} The current credential remains authoritative until connected-app recovery completes."
            ),
        });
    }
    if let Err(error) = db.connected_app_change_commit(host.id(), &change.operation_id) {
        if prepared.rollback().is_ok() {
            let _ = db.connected_app_change_abort_prepared(host.id(), &change.operation_id);
        }
        return Err(format!(
            "The disconnect could not be committed: {}",
            to_message(error)
        ));
    }
    prepared.finalize()?;
    db.connected_app_change_finalize(host.id(), &change.operation_id)
        .map_err(to_message)?;
    connected_app_effective_status(&db, host, home, false)
}

/// Revoke only the SQLCipher credential for a connected host. This deliberately
/// does not parse, create or edit the external app config, so an orphan remains
/// revocable when that config is missing, malformed or no longer has our entry.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_revoke_orphan(
    state: &AppState,
    host_id: String,
) -> Result<hangar_appconfig::HostStatus, String> {
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    mcp_appconfig_revoke_orphan_at(state, host, &home)
}

#[cfg(feature = "agent_automation")]
fn mcp_appconfig_revoke_orphan_at(
    state: &AppState,
    host: hangar_appconfig::Host,
    home: &Path,
) -> Result<hangar_appconfig::HostStatus, String> {
    let db = state.db()?;
    let _guard = connected_app_host_lock(host)
        .lock()
        .map_err(|_| "The connected-app host lock is unavailable.".to_string())?;
    let _os_guard = hangar_appconfig::acquire_host_operation_lock(host, home, "db-only-revoke")?;
    if db
        .connected_app_change(host.id())
        .map_err(to_message)?
        .is_some()
    {
        return Err(
            "A connected-app credential transaction is still journaled. Resolve that recovery before DB-only revocation."
                .to_string(),
        );
    }
    db.connected_app_revoke_db_only(core_connected_app_host(host))
        .map_err(to_message)?;
    connected_app_effective_status(&db, host, home, false)
}

/// Forget an already-revoked connected-host registry row without touching its
/// external config. The authenticated host/path binding is retained so a later
/// owner-directed disconnect can still prove what it owns.
#[cfg(feature = "agent_automation")]
pub fn mcp_appconfig_forget_orphan(
    state: &AppState,
    host_id: String,
) -> Result<hangar_appconfig::HostStatus, String> {
    let host = resolve_connected_app_host(&host_id)?;
    let home = connected_app_home()?;
    mcp_appconfig_forget_orphan_at(state, host, &home)
}

#[cfg(feature = "agent_automation")]
fn mcp_appconfig_forget_orphan_at(
    state: &AppState,
    host: hangar_appconfig::Host,
    home: &Path,
) -> Result<hangar_appconfig::HostStatus, String> {
    let db = state.db()?;
    let _guard = connected_app_host_lock(host)
        .lock()
        .map_err(|_| "The connected-app host lock is unavailable.".to_string())?;
    let _os_guard = hangar_appconfig::acquire_host_operation_lock(host, home, "db-only-forget")?;
    if db
        .connected_app_change(host.id())
        .map_err(to_message)?
        .is_some()
    {
        return Err(
            "A connected-app credential transaction is still journaled. Resolve that recovery before forgetting its registry row."
                .to_string(),
        );
    }
    db.connected_app_forget_db_only(core_connected_app_host(host))
        .map_err(to_message)?;
    connected_app_effective_status(&db, host, home, false)
}

#[cfg(feature = "agent_automation")]
fn handle_automation_request(
    state: &AppState,
    request: hangar_agent::AgentRequest,
) -> hangar_agent::AgentResponse {
    handle_automation_request_for_transport(state, request, AutomationTransport::NamedPipe)
}

#[cfg(feature = "agent_automation")]
fn handle_automation_request_for_transport(
    state: &AppState,
    request: hangar_agent::AgentRequest,
    transport: AutomationTransport,
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
    let agent = match db
        .automation_authenticate_for_transport(&automation_token_hash(token), Some(transport))
    {
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

/// Named-pipe ingress only. A connected-app token is typed `mcp_stdio` in the
/// durable registry and is therefore refused here even if leaked. MCP binds its
/// identity once at process startup and uses [`dispatch_mcp_bound_request`]
/// without carrying a token in an AgentRequest/body.
#[cfg(feature = "agent_automation")]
pub fn dispatch_agent_request(
    state: &AppState,
    request: hangar_agent::AgentRequest,
) -> hangar_agent::AgentResponse {
    handle_automation_request(state, request)
}

/// Opaque, token-free MCP session identity. It is created once from the
/// per-host startup secret, then every call re-resolves this immutable identity
/// against the live DB row so revocation takes effect immediately.
#[cfg(feature = "agent_automation")]
#[derive(Clone, PartialEq, Eq)]
pub struct McpTransportBinding {
    agent_id: i64,
    identity_id: String,
    host: ConnectedAppHost,
    // Private verifier for the credential incarnation that authenticated this
    // process. It is never serialized, logged or placed in an AgentRequest.
    credential_hash: String,
}

#[cfg(feature = "agent_automation")]
impl McpTransportBinding {
    pub fn agent_id(&self) -> i64 {
        self.agent_id
    }

    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }

    pub fn host(&self) -> ConnectedAppHost {
        self.host
    }
}

/// Consume the plaintext startup token and bind this stdio process to the exact
/// immutable connected-host identity written into its host entry. The token is
/// never placed in an AgentRequest or forwarded to the named-pipe ingress.
#[cfg(feature = "agent_automation")]
pub fn bind_mcp_transport(
    state: &AppState,
    token: &str,
    host_id: &str,
    expected_identity_id: &str,
) -> Result<McpTransportBinding, String> {
    let host = resolve_connected_app_host(host_id)?;
    let host = core_connected_app_host(host);
    let identity = expected_identity_id.trim().to_ascii_lowercase();
    if identity.len() != 32 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The MCP host entry has an invalid immutable identity.".to_string());
    }
    if token.trim().is_empty() {
        return Err("The MCP startup credential is missing.".to_string());
    }
    let db = state.db()?;
    let credential_hash = automation_token_hash(token);
    let agent = db
        .automation_authenticate_for_transport(
            &credential_hash,
            Some(AutomationTransport::McpStdio),
        )
        .map_err(to_message)?
        .ok_or_else(|| {
            "The MCP startup credential is invalid, revoked, or bound to another transport."
                .to_string()
        })?;
    if agent.agent_kind != AutomationAgentKind::ConnectedApp
        || agent.allowed_transport != AutomationTransport::McpStdio
        || agent.connected_host != Some(host)
        || agent.identity_id != identity
    {
        let _ = db.automation_log(
            Some(agent.id),
            "mcp_transport_bind",
            "denied",
            "The startup credential did not match the immutable host/transport identity.",
        );
        return Err(
            "The MCP startup credential does not match this connected host identity. Reconnect the app in Code Hangar."
                .to_string(),
        );
    }
    db.automation_log(
        Some(agent.id),
        "mcp_transport_bind",
        "allowed",
        "Bound one MCP stdio process to its immutable connected-host identity.",
    )
    .map_err(to_message)?;
    Ok(McpTransportBinding {
        agent_id: agent.id,
        identity_id: agent.identity_id,
        host,
        credential_hash,
    })
}

#[cfg(feature = "agent_automation")]
fn resolve_mcp_binding_agent(
    db: &Db,
    binding: &McpTransportBinding,
) -> Result<AutomationAgentSummary, String> {
    db.connected_app_agent_for_binding(
        binding.agent_id,
        &binding.identity_id,
        binding.host,
        &binding.credential_hash,
    )
    .map_err(to_message)?
    .ok_or_else(|| {
        "This connected-app credential was revoked or rotated; reconnect it in Code Hangar."
            .to_string()
    })
}

#[cfg(feature = "agent_automation")]
pub fn mcp_catalog_context_bound(
    state: &AppState,
    binding: &McpTransportBinding,
) -> Result<McpCatalogContext, String> {
    let db = state.db()?;
    let agent = resolve_mcp_binding_agent(&db, binding)?;
    Ok(McpCatalogContext {
        scopes: Some(agent.scopes),
        total_control_enabled: db.mcp_full_control_enabled_value().map_err(to_message)?,
        final_remove_enabled: db.final_remove_enabled_value().map_err(to_message)?,
    })
}

/// Token-free MCP dispatch. The central allowlist excludes status, file bodies,
/// plan construction/direct execution and read-grant minting even if a caller
/// manufactures an AgentMethod that the advertised catalog never contains.
#[cfg(feature = "agent_automation")]
pub fn dispatch_mcp_bound_request(
    state: &AppState,
    binding: &McpTransportBinding,
    request_id: String,
    method: hangar_agent::AgentMethod,
    params: serde_json::Value,
) -> hangar_agent::AgentResponse {
    if !connected_app_method_allowed(&method) {
        return hangar_agent::AgentResponse::failure(
            request_id,
            "This method is not allowed on connected-app stdio.",
        );
    }
    let db = match state.db() {
        Ok(db) => db,
        Err(error) => return hangar_agent::AgentResponse::failure(request_id, error),
    };
    let agent = match resolve_mcp_binding_agent(&db, binding) {
        Ok(agent) => agent,
        Err(error) => return hangar_agent::AgentResponse::failure(request_id, error),
    };
    let method_label = automation_method_label(&method);
    let result = run_automation_method(state, &db, &agent, method, params);
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

/// Connector test fixture that creates only a durable, typed MCP credential in
/// an in-memory/temporary inventory. It is excluded from every production
/// build and exists so transport tests never weaken the real registration API.
#[cfg(all(feature = "agent_automation", any(test, feature = "test_support")))]
pub struct McpTestCredential {
    pub token: String,
    pub host_id: String,
    pub identity_id: String,
    pub state_auth_key_hex: String,
    pub binding: McpTransportBinding,
}

#[cfg(all(feature = "agent_automation", any(test, feature = "test_support")))]
pub fn mcp_test_register_transport(
    state: &AppState,
    host_id: &str,
    scopes: Vec<String>,
    project_ids: Vec<i64>,
) -> Result<McpTestCredential, String> {
    let host = resolve_connected_app_host(host_id)?;
    let core_host = core_connected_app_host(host);
    let scopes = normalize_automation_scopes(scopes)?;
    if project_ids.is_empty() {
        return Err("The MCP test credential needs an explicit project scope.".to_string());
    }
    let db = state.db()?;
    let known = db.projects_list_lite().map_err(to_message)?;
    if project_ids
        .iter()
        .any(|id| !known.iter().any(|project| project.id == *id))
    {
        return Err("The MCP test credential contains an unknown project.".to_string());
    }
    let durable = db
        .connected_app_binding_ensure(core_host)
        .map_err(to_message)?;
    let token = hangar_agent::random_token(32)?;
    let token_hash = automation_token_hash(&token);
    let operation_id = hangar_agent::random_token(18)?;
    let hash = |label: &str| hangar_db::ConnectedAppFileFingerprint {
        exists: true,
        hash: Some(blake3::hash(label.as_bytes()).to_hex().to_string()),
    };
    let absent = hangar_db::ConnectedAppFileFingerprint {
        exists: false,
        hash: None,
    };
    db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
        host: host.id().to_string(),
        operation_id: operation_id.clone(),
        kind: "register".to_string(),
        agent_name: host.label().to_string(),
        new_token_hash: Some(token_hash),
        new_scopes: scopes,
        new_project_ids: project_ids,
        fs: hangar_db::ConnectedAppFsContract {
            config_before: hash(&format!("{operation_id}:before")),
            config_after: hash(&format!("{operation_id}:after")),
            backup_before: absent.clone(),
            backup_after: absent.clone(),
            backup_changed: false,
            state_before: absent.clone(),
            state_after: absent,
            state_changed: false,
        },
    })
    .map_err(to_message)?;
    db.connected_app_change_commit(host.id(), &operation_id)
        .map_err(to_message)?;
    db.connected_app_change_finalize(host.id(), &operation_id)
        .map_err(to_message)?;
    let binding = bind_mcp_transport(state, &token, host.id(), &durable.agent_identity_id)?;
    Ok(McpTestCredential {
        token,
        host_id: host.id().to_string(),
        identity_id: durable.agent_identity_id,
        state_auth_key_hex: durable.state_auth_key_hex,
        binding,
    })
}

#[cfg(feature = "agent_automation")]
fn run_automation_method(
    state: &AppState,
    db: &Db,
    agent: &AutomationAgentSummary,
    method: hangar_agent::AgentMethod,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if is_connected_app_agent(agent) && !connected_app_method_allowed(&method) {
        return Err(
            "Connected-app stdio is body-free and request-only: file bodies, plan building/direct execution, and temporary read grants are not dispatchable over MCP."
                .to_string(),
        );
    }
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
                None,
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
            if is_connected_app_agent(agent) {
                return Err(
                    "Connected-app execute_plan access is request-only; direct plan execution is not available."
                        .to_string(),
                );
            }
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
            // Display fields are derived from the live DB agent row, while
            // ownership is persisted and authorized by immutable identity.
            let created = db
                .comment_add_for_agent(params.node_id, &params.body, &agent.identity_id)
                .map_err(to_message)?;
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
            let updated = db
                .comment_edit_for_agent(params.comment_id, &params.body, &agent.identity_id)
                .map_err(to_message)?;
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
            let params: AutomationProjectNodeParams = parse_automation_params(params)?;
            // Both identities are mandatory: the explicit project grant is checked
            // first and the DB call then requires this exact node membership. A node
            // shared with another project cannot borrow either project's authority.
            ensure_automation_project(agent, params.project_id)?;
            let mut relationships = node_relationships(state, params.project_id, params.node_id)?;
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
            let params: AutomationProjectNodeParams = parse_automation_params(params)?;
            ensure_automation_project(agent, params.project_id)?;
            let status = node_orphan_status(state, params.project_id, params.node_id)?;
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
            require_final_remove_recommendation_enabled(db)?;
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
            session_limit: 0,
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
fn is_connected_app_agent(agent: &AutomationAgentSummary) -> bool {
    agent.agent_kind == AutomationAgentKind::ConnectedApp
        && agent.allowed_transport == AutomationTransport::McpStdio
        && agent.connected_host.is_some()
}

#[cfg(feature = "agent_automation")]
fn connected_app_method_allowed(method: &hangar_agent::AgentMethod) -> bool {
    use hangar_agent::AgentMethod;
    matches!(
        method,
        AgentMethod::AgentProjectContext
            | AgentMethod::DeepHistorySearch
            | AgentMethod::CommentsList
            | AgentMethod::CommentsAdd
            | AgentMethod::CommentsEdit
            | AgentMethod::RequestCommentChange
            | AgentMethod::ListCatalog
            | AgentMethod::ListContextFiles
            | AgentMethod::ListProjectNav
            | AgentMethod::ExplainFolder
            | AgentMethod::GetProjectGraph
            | AgentMethod::NodeRelationships
            | AgentMethod::ListOrphanAssets
            | AgentMethod::NodeOrphanStatus
            | AgentMethod::ListDuplicateCandidates
            | AgentMethod::ConfirmDuplicateGroup
            | AgentMethod::ProjectGitStatus
            | AgentMethod::ListAdapters
            | AgentMethod::ListMyRequests
            | AgentMethod::RequestBackupProtected
            | AgentMethod::RequestMoveToHolding
            | AgentMethod::RequestPermanentDelete
    )
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
    state.invalidate_project_app_state_cache();
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
    let cache_generation = state.project_cache_generation();
    let mut projects = state.db()?.projects_list().map_err(to_message)?;
    enrich_antigravity_names(&mut projects);
    enrich_current_state(state, &mut projects);
    state.write_project_cache_if_generation(&projects, cache_generation);
    Ok(projects)
}

pub fn projects_list_lite(state: &AppState) -> Result<Vec<ProjectSummary>, String> {
    sync_wsl_scan_flag(state);
    let cache_generation = state.project_cache_generation();
    let mut projects = state.db()?.projects_list_lite().map_err(to_message)?;
    enrich_antigravity_names(&mut projects);
    enrich_current_state(state, &mut projects);
    state.write_project_cache_if_generation(&projects, cache_generation);
    Ok(projects)
}

/// Attach each project's owning `app` badge and its `is_current` flag from the
/// per-app registries/activity signals. A project is `is_current` when its root
/// appears in a recent app-activity signal — the Antigravity summaries proto (which
/// can mark a registry project current even when its `.pb` conversations never link a parsed
/// `.db`) or a Claude
/// `lastSessionModified`. The process-local cache parses those sources at most once
/// per 60-second window, with explicit invalidation after catalog/WSL-setting changes.
/// Best-effort: a project whose path no registry claims simply keeps
/// `app = None` / `is_current = false`.
fn enrich_current_state(state: &AppState, projects: &mut [ProjectSummary]) {
    let states = state.project_app_states();
    apply_project_app_states(projects, &states);
}

fn apply_project_app_states(
    projects: &mut [ProjectSummary],
    states: &std::collections::HashMap<String, hangar_discovery::ProjectAppState>,
) {
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
/// hides it — e.g. a project named "SampleProject" rooted at `D:\Samples` gains a
/// "named: SampleProject" label. Read from the Antigravity registry; best-effort and skipped
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

/// Read the resident tracker's metadata-only timeline and attach the latest cached
/// local application correlation for each project. This endpoint is polled by the
/// Overview while it is visible, so it must never walk AI-app registries or session
/// histories. Correlation is best-effort: the event remains useful when the cache is
/// not populated yet or no supported app claims its project.
pub fn recent_file_changes(
    state: &AppState,
    project_id: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<FileChangeEvent>, String> {
    let mut events = state
        .db()?
        .recent_file_changes(project_id, limit.unwrap_or(40))
        .map_err(to_message)?;
    // An empty/unreadable cache deliberately leaves `apps` empty. The DB project-list
    // contract cannot supply application attribution, and aggregating the navigation
    // catalog on every cold-cache poll would add cost without adding any labels.
    let projects = state.read_project_cache();
    apply_cached_project_apps(&mut events, &projects);
    Ok(events)
}

fn apply_cached_project_apps(events: &mut [FileChangeEvent], projects: &[ProjectSummary]) {
    let apps_by_project = projects
        .iter()
        .map(|project| {
            let mut apps = project.apps.clone();
            if apps.is_empty() {
                if let Some(app) = &project.app {
                    apps.push(app.clone());
                }
            }
            (project.id, (project.path.clone(), apps))
        })
        .collect::<std::collections::HashMap<_, _>>();

    for event in events {
        event.apps.clear();
        if let Some((project_path, apps)) = apps_by_project.get(&event.project_id) {
            if same_local_path(&event.project_path, project_path) {
                event.apps.clone_from(apps);
            }
        }
    }
}

/// Start one balanced, read-only refresh for roots whose cheap watcher signals
/// show drift. A periodic forced pass closes the remaining gap for newly-created
/// files deep in a tree, while the frequent pass detects root changes and edits
/// to indexed Markdown/context files without a full walk.
pub fn background_refresh_once(
    state: &AppState,
    force_all: bool,
) -> Result<Option<String>, String> {
    let db = state.db()?;
    let roots = db.roots_list().map_err(to_message)?;
    let projects = db.projects_list_lite().map_err(to_message)?;
    let mut candidates = Vec::new();

    for root in roots {
        if !root.enabled || state.jobs.has_running_job_for_root(root.id) {
            continue;
        }
        let project = projects
            .iter()
            .find(|project| same_local_path(&project.path, &root.path));
        let root_status = watcher_project_status(&root, project);
        if matches!(root_status.state.as_str(), "missing" | "disabled" | "empty") {
            continue;
        }
        let focused_dirty = if force_all {
            false
        } else if let Some(project) = project {
            focused_watcher_status(&db, project.id, None)
                .map(|status| status.state == "dirty")
                .unwrap_or(false)
        } else {
            false
        };
        if force_all
            || focused_dirty
            || matches!(root_status.state.as_str(), "stale" | "needs_scan")
        {
            candidates.push(root.id);
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }
    scan_start(state, Some(candidates), Some("balanced".to_string())).map(Some)
}

/// Resident-service variant of [`background_refresh_once`]. It intentionally
/// admits at most one root, never overlaps another scan and always uses the
/// one-worker throttled Background mode. Frequent passes are probes only: they
/// inspect registered-root metadata but never query the large navigation catalog
/// or turn a live file into a whole-root scan. Focused Markdown/context fingerprints
/// remain available to the open UI. Only the widely-spaced reconciliation
/// pass (or an explicit tray refresh, which sets `periodic_complete`) may start
/// inventory work. This separation is essential for projects whose build output
/// or AI session files change continuously.
pub fn background_refresh_resident(
    state: &AppState,
    periodic_complete: bool,
) -> Result<Option<String>, String> {
    if state.jobs.has_any_running_job() {
        return Ok(None);
    }
    let db = state.db()?;
    let roots = db.roots_list().map_err(to_message)?;
    let mut eligible = Vec::new();

    for root in roots {
        if !root.enabled {
            continue;
        }
        let root_status = watcher_project_status(&root, None);
        if matches!(root_status.state.as_str(), "missing" | "disabled" | "empty") {
            continue;
        }
        eligible.push(root);
    }

    if !periodic_complete {
        // A 30-second resident tick is deliberately root-metadata-only. Querying
        // even a bounded context window made a very large encrypted nav index read
        // tens of MiB per tick. Focused file checks run when a project is open.
        return Ok(None);
    }

    let mut candidates = eligible;

    candidates.sort_by(|left, right| {
        left.last_scanned_at
            .as_deref()
            .unwrap_or("")
            .cmp(right.last_scanned_at.as_deref().unwrap_or(""))
            .then_with(|| {
                left.path
                    .to_ascii_lowercase()
                    .cmp(&right.path.to_ascii_lowercase())
            })
    });
    let Some(root) = candidates.first() else {
        return Ok(None);
    };
    scan_start(state, Some(vec![root.id]), Some("background".to_string())).map(Some)
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
    let normalize = |path: &str| {
        normalize_path(&display_path_for_path(path))
            .trim_end_matches('/')
            .to_string()
    };
    normalize(left).eq_ignore_ascii_case(&normalize(right))
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
    let normalized = normalize_root_path(path)?;
    let db = state.db()?;
    let root = db.roots_add_adhoc(&normalized).map_err(to_message)?;
    state.invalidate_project_caches();
    let job_id = scan_start(state, Some(vec![root.id]), performance_mode)?;
    Ok(InvestigationHandle {
        root_id: root.id,
        job_id,
        path: display_path_for_path(&normalized),
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
    db.roots_unregister(root_id).map_err(to_message)?;
    state.invalidate_project_caches();
    Ok(())
}

pub fn node_full_path(state: &AppState, node_id: i64) -> Result<String, String> {
    state
        .db()?
        .node_path(node_id)
        .map_err(to_message)?
        .map(|path| display_path_for_path(&path))
        .ok_or_else(|| "Path is no longer available in the local inventory.".to_string())
}

// In-app file editing uses this exact inventory-resolution + protection gate (registered
// project, not sensitive/protected, not a reparse point) before it may write a file.
#[cfg(feature = "mutation")]
fn resolve_editable_inventory_target(
    state: &AppState,
    node_id: i64,
) -> Result<(String, Vec<String>), String> {
    let Some(target) = state
        .db()?
        .editable_file_target(node_id)
        .map_err(to_message)?
    else {
        return Err(
            "Not edited: this file is not a present item in a registered local project."
                .to_string(),
        );
    };
    if target.is_sensitive || target.protected_level.is_some() {
        return Err(
            "Not edited: this file is sensitive or belongs to a Protected Zone.".to_string(),
        );
    }
    if target.is_reparse || target.reparse_kind.is_some() {
        return Err(
            "Not edited: symlinks, junctions, reparse points and cloud placeholders are not eligible for in-app editing."
                .to_string(),
        );
    }
    let project_paths = state
        .db()?
        .editable_file_project_paths(node_id)
        .map_err(to_message)?;
    if project_paths.is_empty() {
        return Err(
            "Not edited: the file is no longer attached to a registered local project.".to_string(),
        );
    }
    Ok((target.path, project_paths))
}

#[cfg(feature = "mutation")]
fn validate_editable_disk_target(path: &str, project_paths: &[String]) -> Result<(), String> {
    let target = Path::new(path);
    let identity = hangar_fs::inspect_path_identity(target);
    if identity.is_reparse || identity.reparse_kind.is_some() {
        return Err(
            "Not edited: the file is now a symlink, junction, reparse point or cloud placeholder."
                .to_string(),
        );
    }
    let canonical_target = target
        .canonicalize()
        .map_err(|_| "Not edited: the file is no longer available on disk.".to_string())?;
    if !canonical_target.is_file() {
        return Err("Not edited: the selected item is no longer a regular file.".to_string());
    }
    let inside_registered_project = project_paths.iter().any(|project_path| {
        Path::new(project_path)
            .canonicalize()
            .map(|canonical_root| canonical_target.starts_with(canonical_root))
            .unwrap_or(false)
    });
    if !inside_registered_project {
        return Err(
            "Not edited: the file now resolves outside its registered project boundary."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(feature = "agent_automation")]
fn resolve_ai_explain_inventory_target(
    state: &AppState,
    node_id: i64,
) -> Result<(String, Vec<String>), String> {
    resolve_editable_inventory_target(state, node_id)
        .map_err(|error| error.replacen("Not edited:", "Not sent:", 1))
}

#[cfg(feature = "agent_automation")]
fn validate_ai_explain_disk_target(path: &str, project_paths: &[String]) -> Result<(), String> {
    validate_editable_disk_target(path, project_paths)
        .map_err(|error| error.replacen("Not edited:", "Not sent:", 1))
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

#[cfg(feature = "agent_automation")]
const AI_PREPARED_SEND_TTL: Duration = Duration::from_secs(2 * 60);
#[cfg(feature = "agent_automation")]
const AI_PREPARED_SEND_CAP: usize = 32;

#[cfg(feature = "agent_automation")]
pub(crate) fn lock_ai_credential_operation(
    state: &AppState,
) -> Result<std::sync::MutexGuard<'_, ()>, String> {
    state
        .ai_credential_operations
        .lock()
        .map_err(|_| "The provider credential operation lock is unavailable.".to_string())
}

#[cfg(feature = "agent_automation")]
fn freeze_prepared_credential_binding(
    state: &AppState,
    request: &hangar_ai::PreparedRequest,
) -> Result<Option<hangar_core::AiProviderCredentialBinding>, String> {
    if !request.has_credential() {
        return Ok(None);
    }
    if request.is_local() {
        return Err(
            "A local provider request unexpectedly selected a saved API credential. Nothing was sent."
                .to_string(),
        );
    }
    let origin = hangar_ai::endpoint_origin(&request.disclosure().url).ok_or_else(|| {
        "The prepared provider destination has no valid credential origin. Nothing was sent."
            .to_string()
    })?;
    let fingerprint = request.credential_fingerprint().ok_or_else(|| {
        "The prepared provider credential has no verifiable fingerprint. Nothing was sent."
            .to_string()
    })?;
    let binding = state
        .db()?
        .ai_provider_credential_binding()
        .map_err(to_message)?
        .ok_or_else(|| {
            "A saved provider key has no verified origin binding. Save the API provider, then set its key again. Nothing was sent."
                .to_string()
        })?;
    if !credential_binding_authorizes(&binding, &origin, &fingerprint) {
        return Err(
            "The saved provider key does not match this request's origin binding. Save the provider and set its key again. Nothing was sent."
                .to_string(),
        );
    }
    Ok(Some(binding))
}

#[cfg(feature = "agent_automation")]
fn credential_binding_authorizes(
    binding: &hangar_core::AiProviderCredentialBinding,
    request_origin: &str,
    request_fingerprint: &str,
) -> bool {
    binding.status == hangar_core::AiProviderCredentialBindingStatus::Active
        && binding.origin == request_origin
        && binding.fingerprint == request_fingerprint
}

#[cfg(feature = "agent_automation")]
fn validate_pending_credential_binding(
    state: &AppState,
    pending: &PendingAiSend,
) -> Result<(), String> {
    if pending.request.is_local() {
        if pending.credential_binding.is_some() || pending.request.has_credential() {
            return Err(
                "A local provider preview contains remote credential state. Nothing was sent."
                    .to_string(),
            );
        }
        return Ok(());
    }
    let current_fingerprint = hangar_ai::current_key_fingerprint();
    let current_binding = state
        .db()?
        .ai_provider_credential_binding()
        .map_err(to_message)?;
    let prepared_fingerprint = pending.request.credential_fingerprint();
    if !credential_binding_snapshot_matches(
        pending.credential_binding.as_ref(),
        current_binding.as_ref(),
        prepared_fingerprint.as_deref(),
        current_fingerprint.as_deref(),
    ) {
        return Err(
            "The provider credential origin, key or binding version changed after review. Review the exact request again; nothing was sent."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(feature = "agent_automation")]
fn credential_binding_snapshot_matches(
    reviewed: Option<&hangar_core::AiProviderCredentialBinding>,
    current: Option<&hangar_core::AiProviderCredentialBinding>,
    prepared_fingerprint: Option<&str>,
    current_fingerprint: Option<&str>,
) -> bool {
    match reviewed {
        None => {
            current.is_none() && prepared_fingerprint.is_none() && current_fingerprint.is_none()
        }
        Some(reviewed) => {
            current == Some(reviewed)
                && prepared_fingerprint == Some(reviewed.fingerprint.as_str())
                && current_fingerprint == Some(reviewed.fingerprint.as_str())
        }
    }
}

/// Keep a credential-free immutable provider request in process memory until the user accepts
/// its literal disclosure. It is never written to SQLite, logs, fixtures or an IPC response
/// other than the deliberately displayed key-free body/destination.
#[cfg(feature = "agent_automation")]
fn stage_ai_prepared_send(
    state: &AppState,
    kind: AiPreparedKind,
    request: hangar_ai::PreparedRequest,
) -> Result<hangar_core::AiSendDisclosure, String> {
    stage_ai_prepared_send_inner(state, kind, request, None, Vec::new())
}

#[cfg(feature = "agent_automation")]
fn stage_ai_prepared_safe_manage_send(
    state: &AppState,
    request: hangar_ai::PreparedRequest,
    receipt_id: String,
    selected_context_ids: Vec<String>,
) -> Result<hangar_core::AiSendDisclosure, String> {
    stage_ai_prepared_send_inner(
        state,
        AiPreparedKind::SafeManageAdvisory,
        request,
        Some(receipt_id),
        selected_context_ids,
    )
}

#[cfg(feature = "agent_automation")]
fn stage_ai_prepared_send_inner(
    state: &AppState,
    kind: AiPreparedKind,
    request: hangar_ai::PreparedRequest,
    receipt_id: Option<String>,
    selected_context_ids: Vec<String>,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let credential_binding = freeze_prepared_credential_binding(state, &request)?;
    let monotonic_now = Instant::now();
    let unix_now_ms = u128::from(now_millis());
    let preview_id = format!(
        "ai-preview-{}",
        hangar_agent::random_token(24)
            .map_err(|_| "Could not create a secure AI preview id.".to_string())?
    );
    let disclosure = request.disclosure().clone();
    let model = request.model().to_string();
    let mode = if request.is_local() { "local" } else { "api" }.to_string();
    let format = request.format().as_tag().to_string();
    let credential_use = match request.credential_use() {
        hangar_ai::CredentialUse::None => hangar_core::AiCredentialUse::None,
        hangar_ai::CredentialUse::BearerSaved => hangar_core::AiCredentialUse::BearerSaved,
        hangar_ai::CredentialUse::XApiKeySaved => hangar_core::AiCredentialUse::XApiKeySaved,
    };
    let send_chars = disclosure.request_body.chars().count() as u64;
    // Wall time is presentation-only. Authorization expiry below is based exclusively on
    // monotonic time, so rolling the system clock back cannot extend a reviewed capability.
    let expires_at_unix = (unix_now_ms.saturating_add(AI_PREPARED_SEND_TTL.as_millis()) / 1000)
        .min(u128::from(u64::MAX)) as u64;

    let mut store = state
        .ai_prepared_sends
        .lock()
        .map_err(|_| "The prepared AI request store is unavailable.".to_string())?;
    store.requests.retain(|_, pending| {
        monotonic_now.saturating_duration_since(pending.created_at) <= AI_PREPARED_SEND_TTL
    });
    if store.requests.contains_key(&preview_id) {
        return Err("Could not create a unique AI preview id.".to_string());
    }
    if store.requests.len() >= AI_PREPARED_SEND_CAP {
        if let Some(oldest) = store
            .requests
            .iter()
            .min_by_key(|(_, pending)| pending.created_at)
            .map(|(id, _)| id.clone())
        {
            store.requests.remove(&oldest);
        }
    }
    store.requests.insert(
        preview_id.clone(),
        PendingAiSend {
            kind,
            request,
            credential_binding,
            created_at: monotonic_now,
            receipt_id: receipt_id.clone(),
            selected_context_ids,
        },
    );
    Ok(hangar_core::AiSendDisclosure {
        preview_id,
        receipt_id,
        expires_at_unix,
        method: disclosure.method,
        url: disclosure.url,
        request_body: disclosure.request_body,
        fallback_request_body: disclosure.fallback_request_body,
        transport: disclosure.transport,
        mode,
        model,
        format,
        credential_use,
        send_chars,
        est_tokens: send_chars.div_ceil(4),
    })
}

/// Consume first, then validate. A stale, replayed or wrong-purpose id is destroyed and can never
/// authorize a later send. The only returned value is the exact immutable request that was shown.
#[cfg(feature = "agent_automation")]
fn take_ai_prepared_send(
    state: &AppState,
    preview_id: &str,
    expected_kind: AiPreparedKind,
) -> Result<hangar_ai::PreparedRequest, String> {
    Ok(take_ai_prepared_send_pending(state, preview_id, expected_kind)?.request)
}

#[cfg(feature = "agent_automation")]
fn take_ai_prepared_safe_manage_send(
    state: &AppState,
    preview_id: &str,
) -> Result<(hangar_ai::PreparedRequest, String, Vec<String>), String> {
    let pending =
        take_ai_prepared_send_pending(state, preview_id, AiPreparedKind::SafeManageAdvisory)?;
    let receipt_id = pending.receipt_id.ok_or_else(|| {
        "That advisory preview has no durable receipt. Review the exact request again; nothing was sent."
            .to_string()
    })?;
    if pending.selected_context_ids.is_empty() {
        return Err(
            "That advisory preview has no explicit context selection. Review the exact request again; nothing was sent."
                .to_string(),
        );
    }
    Ok((pending.request, receipt_id, pending.selected_context_ids))
}

#[cfg(feature = "agent_automation")]
fn take_ai_prepared_send_pending(
    state: &AppState,
    preview_id: &str,
    expected_kind: AiPreparedKind,
) -> Result<PendingAiSend, String> {
    take_ai_prepared_send_pending_at(state, preview_id, expected_kind, Instant::now())
}

#[cfg(feature = "agent_automation")]
fn take_ai_prepared_send_pending_at(
    state: &AppState,
    preview_id: &str,
    expected_kind: AiPreparedKind,
    monotonic_now: Instant,
) -> Result<PendingAiSend, String> {
    let pending = state
        .ai_prepared_sends
        .lock()
        .map_err(|_| "The prepared AI request store is unavailable.".to_string())?
        .requests
        .remove(preview_id)
        .ok_or_else(|| {
            "That AI send preview is missing, expired or was already used. Review the exact request again; nothing was sent."
                .to_string()
        })?;
    if monotonic_now.saturating_duration_since(pending.created_at) > AI_PREPARED_SEND_TTL {
        return Err(
            "That AI send preview expired. Review the exact request again; nothing was sent."
                .to_string(),
        );
    }
    if pending.kind != expected_kind {
        return Err(
            "That AI send preview belongs to a different action. Review this request again; nothing was sent."
                .to_string(),
        );
    }
    validate_pending_credential_binding(state, &pending)?;
    Ok(pending)
}

#[cfg(feature = "agent_automation")]
fn ensure_ai_prepared_request_still_matches(
    reviewed: &hangar_ai::PreparedRequest,
    rebuilt: &hangar_ai::PreparedRequest,
) -> Result<(), String> {
    if reviewed.disclosure() != rebuilt.disclosure()
        || reviewed.model() != rebuilt.model()
        || reviewed.is_local() != rebuilt.is_local()
        || reviewed.format() != rebuilt.format()
    {
        return Err(
            "The file, provider, model, payload or destination changed after preview. Review the exact request again; nothing was sent."
                .to_string(),
        );
    }
    Ok(())
}

/// Read-only cost and safety preview for AI Assist. The webview supplies only a node id;
/// the backend resolves and authorizes the current inventory record before reading bytes.
#[cfg(feature = "agent_automation")]
pub fn ai_explain_preview(state: &AppState, node_id: i64) -> Result<AiExplainPreview, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    ai_assist::ai_explain_preview_for_path(&path)
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
    let request = ai_assist::ai_prepare_read_for_path(&path, snippet, lens, level, &config)?;
    stage_ai_prepared_send(state, AiPreparedKind::Read, request)
}

/// Stream the primary Explain/What-to-check result. The same fresh target resolution, send-gate
/// and prompt builders power disclosure and the real send. The callback receives text deltas only.
#[cfg(feature = "agent_automation")]
pub fn ai_read_stream<F>(state: &AppState, preview_id: &str, on_delta: F) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let _credential_operation = lock_ai_credential_operation(state)?;
    let request = take_ai_prepared_send(state, preview_id, AiPreparedKind::Read)?;
    if request.is_local() {
        hangar_ai::send_prepared_stream(request, on_delta)
    } else {
        let text = hangar_ai::send_prepared(request)?;
        let mut on_delta = on_delta;
        on_delta(&text)?;
        Ok(text)
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
    preview_id: &str,
) -> Result<String, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed = take_ai_prepared_send(state, preview_id, AiPreparedKind::Walkthrough)?;
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let rebuilt = ai_assist::ai_prepare_walkthrough_for_path(&path, &section_ids, level, &config)?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt)?;
    hangar_ai::send_prepared(reviewed)
}

#[cfg(feature = "agent_automation")]
pub fn ai_walkthrough_disclosure(
    state: &AppState,
    node_id: i64,
    section_ids: Vec<String>,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let request = ai_assist::ai_prepare_walkthrough_for_path(&path, &section_ids, level, &config)?;
    stage_ai_prepared_send(state, AiPreparedKind::Walkthrough, request)
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
pub struct AiFollowUpRequest<'a> {
    pub node_id: i64,
    pub section_id: &'a str,
    pub conversation_id: Option<&'a str>,
    pub question: &'a str,
    pub level: &'a str,
    pub model: &'a str,
    pub preview_id: &'a str,
}

#[cfg(feature = "agent_automation")]
pub fn ai_follow_up(
    state: &AppState,
    request: AiFollowUpRequest<'_>,
) -> Result<AiFollowUpResult, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed = take_ai_prepared_send(state, request.preview_id, AiPreparedKind::FollowUp)?;
    let path = resolve_ai_explain_target(state, request.node_id)?;
    let config = resolve_ai_provider_with_model(state, request.model)?;
    let history = follow_up_history(
        state,
        request.node_id,
        request.section_id,
        request.conversation_id,
    )?;
    let rebuilt = ai_assist::ai_prepare_follow_up_for_path(
        &path,
        request.section_id,
        &history,
        request.question,
        request.level,
        &config,
    )?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt)?;
    let reservation = reserve_follow_up_turn(
        state,
        request.node_id,
        request.section_id,
        request.conversation_id,
        request.question,
    )?;
    if reservation.history != history {
        finish_follow_up_turn(state, &reservation.conversation_id, reservation.turn, None);
        return Err(
            "The follow-up changed while its request was being prepared. Review it again; nothing was sent."
                .to_string(),
        );
    }
    let answer = hangar_ai::send_prepared(reviewed);
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
                section_id: request.section_id.to_string(),
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
pub fn ai_follow_up_disclosure(
    state: &AppState,
    node_id: i64,
    section_id: &str,
    conversation_id: Option<&str>,
    question: &str,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let history = follow_up_history(state, node_id, section_id, conversation_id)?;
    let request = ai_assist::ai_prepare_follow_up_for_path(
        &path, section_id, &history, question, level, &config,
    )?;
    stage_ai_prepared_send(state, AiPreparedKind::FollowUp, request)
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

#[cfg(feature = "agent_automation")]
#[allow(clippy::too_many_arguments)]
pub fn ai_change_disclosure(
    state: &AppState,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: &str,
    lens: &str,
    file_path: Option<&str>,
    edit_index: Option<usize>,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let (read_lens, kind) = match lens {
        "narration" => (
            ai_assist::AiChangeLens::Narration,
            AiPreparedKind::ChangeNarration,
        ),
        "learning" => {
            if file_path.is_none() || edit_index.is_none() {
                return Err(
                    "Select one recorded edit before preparing its explanation.".to_string()
                );
            }
            (
                ai_assist::AiChangeLens::Learning,
                AiPreparedKind::ChangeLearning,
            )
        }
        "review" => (
            ai_assist::AiChangeLens::Review,
            AiPreparedKind::ChangeReview,
        ),
        _ => return Err("Unknown retrospective AI lens.".to_string()),
    };
    let request = ai_assist::ai_prepare_change_request(
        &change_set,
        file_path,
        edit_index,
        level,
        read_lens,
        &config,
    )?;
    stage_ai_prepared_send(state, kind, request)
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
    preview_id: &str,
) -> Result<String, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed = take_ai_prepared_send(state, preview_id, AiPreparedKind::ChangeNarration)?;
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let rebuilt = ai_assist::ai_prepare_change_request(
        &change_set,
        None,
        None,
        level,
        ai_assist::AiChangeLens::Narration,
        &config,
    )?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt)?;
    hangar_ai::send_prepared(reviewed)
}

/// Teach the user how to read one selected recorded edit. File and edit are
/// resolved from a fresh backend Recap, never from an untrusted webview body.
#[cfg(feature = "agent_automation")]
pub struct AiRecordedEditSelector<'a> {
    pub file_path: &'a str,
    pub edit_index: usize,
}

#[cfg(feature = "agent_automation")]
pub struct AiExplainChangeRequest<'a> {
    pub project_id: i64,
    pub session_paths: Vec<String>,
    pub source_mode: &'a str,
    pub edit: AiRecordedEditSelector<'a>,
    pub level: &'a str,
    pub model: &'a str,
    pub preview_id: &'a str,
}

#[cfg(feature = "agent_automation")]
pub fn ai_explain_change(
    state: &AppState,
    request: AiExplainChangeRequest<'_>,
) -> Result<String, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed =
        take_ai_prepared_send(state, request.preview_id, AiPreparedKind::ChangeLearning)?;
    let change_set = project_review::project_recap_for_ai(
        state,
        request.project_id,
        request.session_paths,
        request.source_mode,
    )?;
    let config = resolve_ai_provider_with_model(state, request.model)?;
    let rebuilt = ai_assist::ai_prepare_change_request(
        &change_set,
        Some(request.edit.file_path),
        Some(request.edit.edit_index),
        request.level,
        ai_assist::AiChangeLens::Learning,
        &config,
    )?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt)?;
    hangar_ai::send_prepared(reviewed)
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
    preview_id: &str,
) -> Result<String, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed = take_ai_prepared_send(state, preview_id, AiPreparedKind::ChangeReview)?;
    let change_set =
        project_review::project_recap_for_ai(state, project_id, session_paths, source_mode)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let rebuilt = ai_assist::ai_prepare_change_request(
        &change_set,
        None,
        None,
        level,
        ai_assist::AiChangeLens::Review,
        &config,
    )?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt)?;
    hangar_ai::send_prepared(reviewed)
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
    preview_id: &str,
) -> Result<AiRewriteProposal, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let reviewed = take_ai_prepared_send(state, preview_id, AiPreparedKind::Rewrite)?;
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let rebuilt = ai_assist::ai_prepare_rewrite_text_with_config(
        snippet,
        &path,
        instruction,
        level,
        &config,
    )?;
    ensure_ai_prepared_request_still_matches(&reviewed, &rebuilt.request)?;
    let rewritten = hangar_ai::send_prepared(reviewed)?;
    let staged = ai_assist::ai_finish_rewrite(rebuilt.source, rebuilt.language, &rewritten)?;
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
pub fn ai_rewrite_disclosure(
    state: &AppState,
    node_id: i64,
    snippet: &str,
    instruction: &str,
    level: &str,
    model: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let path = resolve_ai_explain_target(state, node_id)?;
    let config = resolve_ai_provider_with_model(state, model)?;
    let prepared = ai_assist::ai_prepare_rewrite_text_with_config(
        snippet,
        &path,
        instruction,
        level,
        &config,
    )?;
    stage_ai_prepared_send(state, AiPreparedKind::Rewrite, prepared.request)
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
    preview_id: &str,
) -> Result<hangar_core::AiProjectSummary, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let request = take_ai_prepared_send(state, preview_id, AiPreparedKind::ProjectSummary)?;
    let estimated_input_tokens = request
        .disclosure()
        .request_body
        .chars()
        .count()
        .div_ceil(4) as u64;
    let model = request.model().to_string();
    let summary = hangar_ai::send_prepared(request)?;
    Ok(hangar_core::AiProjectSummary {
        summary,
        estimated_input_tokens,
        model,
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
    let request = ai_assist::ai_prepare_project_summary_with_config(&context, level, &config)?;
    stage_ai_prepared_send(state, AiPreparedKind::ProjectSummary, request)
}

/// Resolve one immutable, backend-saved assessment from the latest completed Safe Manage run.
/// The webview supplies selectors only. An older run, mismatched revision, stale row, or project
/// mismatch fails closed before a provider request can be prepared or sent.
#[cfg(feature = "agent_automation")]
fn resolve_safe_manage_ai_assessment(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
) -> Result<hangar_core::SafeManageProjectAssessment, String> {
    if analysis_run_id.trim().is_empty()
        || analysis_run_id.len() > 128
        || evidence_revision.len() != 64
        || !evidence_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("That Safe Manage evidence selector is invalid. Analyze again.".to_string());
    }
    let run = state
        .db()?
        .safe_manage_analysis_latest_complete()
        .map_err(to_message)?
        .ok_or_else(|| {
            "Run Safe Manage analysis before requesting AI recommendation enrichment.".to_string()
        })?;
    if run.id != analysis_run_id || run.state != "completed" {
        return Err(
            "That Safe Manage assessment is no longer the latest completed evidence. Analyze again."
                .to_string(),
        );
    }
    let assessment = run
        .assessments
        .into_iter()
        .find(|assessment| assessment.project_id == project_id)
        .ok_or_else(|| {
            "That project is not part of the selected Safe Manage analysis.".to_string()
        })?;
    if assessment.analysis_run_id != analysis_run_id
        || assessment.evidence_revision != evidence_revision
        || assessment.evidence_stale
    {
        return Err(
            "That Safe Manage evidence changed or is stale. Analyze again before AI recommendation enrichment."
                .to_string(),
        );
    }
    Ok(assessment)
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
/// the user configures a provider. Treat persisted settings as untrusted legacy/corrupt state:
/// validate them again here, and let `hangar-ai` validate the final request URL once more at send.
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
    let base_url = stored.base_url.trim();
    if base_url.is_empty() {
        return Err("No AI provider endpoint is set. Add one in Settings ▸ AI Assist.".to_string());
    }
    if local {
        hangar_ai::validate_local_endpoint(base_url)?;
    } else {
        hangar_ai::validate_remote_endpoint(base_url)?;
    }
    let format = hangar_ai::ProviderFormat::try_from_tag(stored.format.trim())?;
    Ok(hangar_ai::ProviderConfig {
        base_url: base_url.to_string(),
        model: stored.model,
        format,
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
    let _credential_operation = lock_ai_credential_operation(state)?;
    ai_provider_set_with_key_clear(
        state,
        mode,
        base_url,
        model,
        format,
        hangar_ai::current_key_fingerprint(),
        ai_assist::ai_key_clear,
    )
}

#[cfg(feature = "agent_automation")]
fn ai_provider_set_with_key_clear<F>(
    state: &AppState,
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
    current_fingerprint: Option<String>,
    clear_key: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
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
    let db = state.db()?;
    let binding = db.ai_provider_credential_binding().map_err(to_message)?;
    if provider_set_requires_credential_clear(
        binding.as_ref(),
        current_fingerprint.as_deref(),
        mode,
        base_url,
    )? {
        // Clearing is a prerequisite, never best-effort. Binding is removed first so a keychain
        // failure leaves the old provider unavailable but cannot activate the new origin with a
        // stale credential. The new config is persisted only after both steps succeed.
        clear_provider_credential_with(state, clear_key)?;
    }
    let config = AiProviderConfig {
        mode: mode.to_string(),
        base_url: base_url.to_string(),
        model: model.trim().to_string(),
        format: format.to_string(),
    };
    db.set_ai_provider_config(&config).map_err(to_message)
}

#[cfg(feature = "agent_automation")]
fn provider_set_requires_credential_clear(
    binding: Option<&hangar_core::AiProviderCredentialBinding>,
    current_fingerprint: Option<&str>,
    next_mode: &str,
    next_base_url: &str,
) -> Result<bool, String> {
    let Some(binding) = binding else {
        // A key with no binding is legacy/ambiguous and must be removed before any new config can
        // become active. With neither key nor binding there is nothing to clear.
        return Ok(current_fingerprint.is_some());
    };
    if binding.status != hangar_core::AiProviderCredentialBindingStatus::Active
        || current_fingerprint != Some(binding.fingerprint.as_str())
        || next_mode != "api"
    {
        return Ok(true);
    }
    let next_origin = hangar_ai::remote_credential_origin(next_base_url)?;
    Ok(binding.origin != next_origin)
}

#[cfg(feature = "agent_automation")]
fn clear_provider_credential_with<F>(state: &AppState, clear_key: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let db = state.db()?;
    db.set_ai_provider_credential_binding(None)
        .map_err(to_message)?;
    clear_key().map_err(|error| {
        format!(
            "The saved provider credential could not be cleared, so the provider change was not activated. The origin binding was revoked and no request can use that key: {error}"
        )
    })
}

#[cfg(feature = "agent_automation")]
fn ai_key_set_with_writer<F>(state: &AppState, key: &str, write_key: F) -> Result<(), String>
where
    F: FnOnce(&str) -> Result<String, String>,
{
    ai_key_set_with_writer_and_restore(state, key, write_key, |db, previous| {
        db.set_ai_provider_credential_binding(previous)
            .map_err(to_message)
    })
}

#[cfg(feature = "agent_automation")]
fn ai_key_set_with_writer_and_restore<F, R>(
    state: &AppState,
    key: &str,
    write_key: F,
    restore_binding: R,
) -> Result<(), String>
where
    F: FnOnce(&str) -> Result<String, String>,
    R: FnOnce(
        &hangar_db::Db,
        Option<&hangar_core::AiProviderCredentialBinding>,
    ) -> Result<(), String>,
{
    let key = key.trim();
    if key.len() < 12 {
        return Err("That does not look like an API key.".to_string());
    }
    let db = state.db()?;
    let provider = db.ai_provider_config().map_err(to_message)?;
    if provider.mode.trim() != "api" {
        return Err(
            "Save an API provider before saving its key. No credential was changed.".to_string(),
        );
    }
    hangar_ai::validate_remote_endpoint(provider.base_url.trim())?;
    let origin = hangar_ai::remote_credential_origin(&provider.base_url)?;
    let expected_fingerprint = hangar_ai::key_material_fingerprint(key);
    let previous = db.ai_provider_credential_binding().map_err(to_message)?;
    let mut binding = hangar_core::AiProviderCredentialBinding {
        origin,
        fingerprint: expected_fingerprint.clone(),
        version: hangar_agent::random_token(32)
            .map_err(|_| "Could not create a credential binding version.".to_string())?,
        status: hangar_core::AiProviderCredentialBindingStatus::Pending,
    };

    // Publish a pending binding first. Until the key write completes, its fingerprint cannot
    // match the old key, so concurrent sends fail closed. If the write fails, restore the prior
    // metadata; a partial external write still mismatches that old fingerprint and remains closed.
    db.set_ai_provider_credential_binding(Some(&binding))
        .map_err(to_message)?;
    let actual_fingerprint = match write_key(key) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let rollback = restore_binding(&db, previous.as_ref());
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error} The previous credential binding could not be restored, so all provider sends remain blocked: {rollback_error}"
                )),
            };
        }
    };
    if actual_fingerprint != expected_fingerprint {
        db.set_ai_provider_credential_binding(None)
            .map_err(to_message)?;
        return Err(
            "Credential Manager did not retain the key that was just reviewed. Its binding was revoked and nothing can be sent until the key is saved again."
                .to_string(),
        );
    }
    binding.status = hangar_core::AiProviderCredentialBindingStatus::Active;
    db.set_ai_provider_credential_binding(Some(&binding))
        .map_err(|error| {
            format!(
                "The provider key was saved but its origin binding could not be activated. It remains pending and no request can use it: {error}"
            )
        })
}

#[cfg(feature = "agent_automation")]
pub fn ai_key_set(state: &AppState, key: &str) -> Result<(), String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    ai_key_set_with_writer(state, key, |key| {
        ai_assist::ai_key_set(key)?;
        let fingerprint = hangar_ai::current_key_fingerprint().ok_or_else(|| {
            "Credential Manager did not return the saved provider key.".to_string()
        })?;
        Ok(fingerprint)
    })
}

#[cfg(feature = "agent_automation")]
pub fn ai_key_status(state: &AppState) -> Result<bool, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let Some(current_fingerprint) = hangar_ai::current_key_fingerprint() else {
        return Ok(false);
    };
    let db = state.db()?;
    let Some(binding) = db.ai_provider_credential_binding().map_err(to_message)? else {
        // Legacy unbound key: visible to Credential Manager, but never authorized for transport.
        return Ok(false);
    };
    let provider = db.ai_provider_config().map_err(to_message)?;
    if provider.mode.trim() != "api" {
        return Ok(false);
    }
    let provider_origin = hangar_ai::remote_credential_origin(&provider.base_url)?;
    Ok(
        binding.status == hangar_core::AiProviderCredentialBindingStatus::Active
            && provider_origin == binding.origin
            && current_fingerprint == binding.fingerprint,
    )
}

#[cfg(feature = "agent_automation")]
pub fn ai_key_clear(state: &AppState) -> Result<(), String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    clear_provider_credential_with(state, ai_assist::ai_key_clear)
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
        format: hangar_ai::ProviderFormat::try_from_tag(format.trim())?,
        local,
    })
}

/// Prepare the fixed reachability ping for a provider DRAFT. This validates the on-screen fields
/// without persisting them and returns only a credential-free disclosure plus a short-lived,
/// one-shot capability; no provider transport occurs here.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_test_disclosure(
    state: &AppState,
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let config = build_ai_provider_config(mode, base_url, model, format)?;
    let request = ai_assist::ai_prepare_provider_test_with_config(&config)?;
    stage_ai_prepared_send(state, AiPreparedKind::ProviderTest, request)
}

/// Consume a reviewed provider-test capability before sending its immutable fixed ping. Draft
/// endpoint/model/format fields are intentionally absent so IPC cannot swap them after review.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_test(state: &AppState, preview_id: &str) -> Result<String, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let request = take_ai_prepared_send(state, preview_id, AiPreparedKind::ProviderTest)?;
    hangar_ai::send_prepared(request).map(|_| "Provider responded.".to_string())
}

/// Prepare an exact `GET /models` for a provider DRAFT without sending or persisting anything.
/// Unsupported formats fail locally and the UI retains its free-text model field.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_models_disclosure(
    state: &AppState,
    mode: &str,
    base_url: &str,
    model: &str,
    format: &str,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let config = build_ai_provider_config(mode, base_url, model, format)?;
    let request = ai_assist::ai_prepare_provider_models_with_config(&config)?;
    stage_ai_prepared_send(state, AiPreparedKind::ProviderModels, request)
}

/// Consume a reviewed model-list capability before issuing its immutable GET. Authentication,
/// timeout, invalid-response and empty-list failures remain visible to the free-text UI.
#[cfg(feature = "agent_automation")]
pub fn ai_provider_models(state: &AppState, preview_id: &str) -> Result<Vec<String>, String> {
    let _credential_operation = lock_ai_credential_operation(state)?;
    let request = take_ai_prepared_send(state, preview_id, AiPreparedKind::ProviderModels)?;
    hangar_ai::send_prepared_provider_models(request)
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
    project_id: Option<i64>,
    mode: PreviewMode,
    record_recent: Option<bool>,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    state
        .db()?
        .file_preview_with_policy_for_project(
            node_id,
            project_id,
            mode,
            record_recent.unwrap_or(true),
            policy.unwrap_or_default(),
        )
        .map_err(to_message)
}

pub fn file_reveal(
    state: &AppState,
    node_id: i64,
    project_id: Option<i64>,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    state
        .db()?
        .file_reveal_with_policy_for_project(node_id, project_id, mode, policy.unwrap_or_default())
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
        let mut file = hangar_fs::open_local_file_no_recall(path)?;
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
        hangar_fs::open_local_file_no_recall(path)?
            .take(cap)
            .read_to_end(&mut buffer)?;
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

    let file = hangar_fs::open_local_file_no_recall(path)?;
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
    let mut file = hangar_fs::open_local_file_no_recall(path)?;
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
    let mut file = hangar_fs::open_local_file_no_recall(path)?;
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
    undated_session_fingerprint: Option<&str>,
) -> Result<ProjectReviewCheckpoint, String> {
    project_review::mark_project_reviewed(
        state,
        project_id,
        session_cutoff_ms,
        undated_session_fingerprint,
    )
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

    if hangar_discovery::is_opencode_state_db(&canonical) {
        if let Some(session_id) = fragment
            .as_deref()
            .and_then(|value| value.strip_prefix("opencode-session="))
        {
            if let Some((transcript, transcript_truncated)) =
                hangar_discovery::opencode_session_transcript_window(
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
                        "OpenCode · {}",
                        session_id.chars().take(10).collect::<String>()
                    ),
                    session_kind: "OpenCode".to_string(),
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
            // A recognized OpenCode DB must never fall through to the generic
            // byte renderer, which would expose SQLite binary data as gibberish.
            return Ok(unreadable_session_store_preview(
                path,
                &canonical,
                "OpenCode",
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
        let input = "Edited C:/Users/Person/Projects/ExampleProject/src/main.rs at commit 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b today";
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

fn require_exact_node_membership(db: &Db, project_id: i64, node_id: i64) -> Result<(), String> {
    let memberships = db.node_project_ids(node_id).map_err(to_message)?;
    if memberships.contains(&project_id) {
        Ok(())
    } else {
        Err("Node is not part of the requested project.".to_string())
    }
}

pub fn node_relationships(
    state: &AppState,
    project_id: i64,
    node_id: i64,
) -> Result<NodeRelationships, String> {
    let db = state.db()?;
    require_exact_node_membership(&db, project_id, node_id)?;
    db.node_relationships_for_project(project_id, node_id)
        .map_err(to_message)
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

pub fn node_orphan_status(
    state: &AppState,
    project_id: i64,
    node_id: i64,
) -> Result<OrphanStatus, String> {
    let db = state.db()?;
    require_exact_node_membership(&db, project_id, node_id)?;
    db.node_orphan_status_for_project(project_id, node_id)
        .map_err(to_message)
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
    session_limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    let wsl_enabled = wsl_scan_enabled(state);
    let candidate_limit = limit.unwrap_or(100).min(500);
    let session_limit = session_limit.unwrap_or(candidate_limit).min(5_000);
    let snapshot = state
        .project_discovery_source
        .snapshot(wsl_enabled, ProjectDiscoveryScope::Global);
    let registered_roots = registered_roots_for_state(state)?;
    Ok(hangar_discovery::discover_known_projects_with_wsl_snapshot(
        &registered_roots,
        DiscoveryOptions {
            limit: candidate_limit,
            session_limit,
            // Loose (project-less) sessions default ON now, so "sessions soltas"
            // show out of the box; the caller/UI can still pass false to hide them.
            include_loose_sessions: include_loose_sessions.unwrap_or(true),
            include_agents: include_agents.unwrap_or(false),
            include_technical_candidates: include_technical_candidates.unwrap_or(false),
        },
        &snapshot,
    ))
}

pub fn project_discovery_deep_scan(
    state: &AppState,
    root_path: String,
    limit: Option<usize>,
    session_limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    let wsl_enabled = wsl_scan_enabled(state);
    let root = PathBuf::from(display_path_for_path(&root_path));
    if !root.is_absolute() {
        return Err("Choose an absolute local folder or drive for Deep Scan.".to_string());
    }
    reject_remote_windows_drive(&root)?;
    let root = hangar_fs::validate_local_scan_root(&root)
        .map_err(|error| format!("Cannot safely open the Deep Scan root: {error}"))?;
    let candidate_limit = limit.unwrap_or(250).min(1_000);
    let session_limit = session_limit.unwrap_or(candidate_limit).min(5_000);
    let snapshot = state
        .project_discovery_source
        .snapshot(wsl_enabled, ProjectDiscoveryScope::Folder);
    let registered_roots = registered_roots_for_state(state)?;
    Ok(
        hangar_discovery::discover_projects_in_root_with_wsl_snapshot(
            &root,
            &registered_roots,
            DiscoveryOptions {
                limit: candidate_limit,
                session_limit,
                // Default ON to match project_discovery_report (loose sessions visible
                // by default); an explicit false from the UI still hides them.
                include_loose_sessions: include_loose_sessions.unwrap_or(true),
                include_agents: include_agents.unwrap_or(false),
                include_technical_candidates: include_technical_candidates.unwrap_or(false),
            },
            &snapshot,
        ),
    )
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
    let _inventory_guard = state
        .inventory_mutation_gate
        .write()
        .map_err(|_| "Inventory/mutation coordination lock is poisoned.".to_string())?;
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
    operation_plan_start_with_safe_manage_binding(
        state,
        target_node_id,
        action_label,
        performance_mode,
        None,
    )
}

/// Start a preview job whose target was resolved from a current Safe Manage
/// decision. The request is checked once before the job is admitted and again
/// while holding the inventory/mutation write gate, immediately before and
/// after the plan/risk snapshot is built. A stale decision, changed analysis or
/// substituted regenerable target therefore fails the job closed instead of
/// silently falling back to a caller-supplied node id.
pub(crate) fn operation_plan_start_with_safe_manage_binding(
    state: &AppState,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
    safe_manage_request: Option<SafeManageOperationPlanRequest>,
) -> Result<String, String> {
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    let db = state.db()?;
    if let Some(request) = safe_manage_request.as_ref() {
        let resolved = db
            .safe_manage_operation_plan_target(request)
            .map_err(to_message)?;
        if resolved != target_node_id {
            return Err(
                "The Safe Manage operation target changed before preview admission.".to_string(),
            );
        }
    }
    let (job_id, cancel) = state
        .plan_jobs
        .create_running(target_node_id, action_label.clone());
    let jobs = state.plan_jobs.clone();
    let inventory_mutation_gate = SharedArc::clone(&state.inventory_mutation_gate);
    let worker_state = state.clone();
    let thread_job_id = job_id.clone();

    thread::spawn(move || {
        let _inventory_guard = match inventory_mutation_gate.write() {
            Ok(guard) => guard,
            Err(_) => {
                jobs.fail(
                    &thread_job_id,
                    "Inventory/mutation coordination lock is poisoned.".to_string(),
                );
                return;
            }
        };
        let _performance = PerformanceScope::enter(mode);
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            jobs.cancel(&thread_job_id);
            return;
        }
        if let Some(request) = safe_manage_request.as_ref() {
            if let Err(message) = safe_manage::require_current_project_revision(
                &worker_state,
                request.project_id,
                &request.evidence_revision,
            ) {
                jobs.fail(&thread_job_id, message);
                return;
            }
            match db
                .safe_manage_operation_plan_target(request)
                .map_err(to_message)
            {
                Ok(resolved) if resolved == target_node_id => {}
                Ok(_) => {
                    jobs.fail(
                        &thread_job_id,
                        "The Safe Manage operation target changed before preview construction."
                            .to_string(),
                    );
                    return;
                }
                Err(message) => {
                    jobs.fail(&thread_job_id, message);
                    return;
                }
            }
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
                if let Some(request) = safe_manage_request.as_ref() {
                    if let Err(message) = safe_manage::require_current_project_revision(
                        &worker_state,
                        request.project_id,
                        &request.evidence_revision,
                    ) {
                        jobs.fail(&thread_job_id, message);
                        return;
                    }
                    match db
                        .safe_manage_operation_plan_target(request)
                        .map_err(to_message)
                    {
                        Ok(resolved) if resolved == target_node_id => {}
                        Ok(_) => {
                            jobs.fail(
                                &thread_job_id,
                                "The Safe Manage operation target changed while the preview was being built."
                                    .to_string(),
                            );
                            return;
                        }
                        Err(message) => {
                            jobs.fail(&thread_job_id, message);
                            return;
                        }
                    }
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
                    if let Some(request) = safe_manage_request.as_ref() {
                        if let Err(message) = safe_manage::require_current_project_revision(
                            &worker_state,
                            request.project_id,
                            &request.evidence_revision,
                        ) {
                            jobs.fail(&thread_job_id, message);
                            return;
                        }
                        match db
                            .safe_manage_operation_plan_target(request)
                            .map_err(to_message)
                        {
                            Ok(resolved) if resolved == target_node_id => {}
                            Ok(_) => {
                                jobs.fail(
                                    &thread_job_id,
                                    "The Safe Manage operation target changed before preview completion."
                                        .to_string(),
                                );
                                return;
                            }
                            Err(message) => {
                                jobs.fail(&thread_job_id, message);
                                return;
                            }
                        }
                    }
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
    #[cfg(feature = "agent_automation")]
    let edition = "Connector";
    #[cfg(not(feature = "agent_automation"))]
    let edition = "Local";
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

#[cfg(not(feature = "agent_automation"))]
fn diagnostics_security_payload(security: &SecurityStatus) -> serde_json::Value {
    serde_json::json!({
        "mutationExecutor": security.mutation_executor,
        "activeFeatures": security.active_features,
    })
}

#[cfg(feature = "agent_automation")]
fn diagnostics_security_payload(security: &SecurityStatus) -> serde_json::Value {
    serde_json::json!({
        "outboundNetwork": security.outbound_network,
        "mutationExecutor": security.mutation_executor,
        "agentIpc": security.agent_ipc,
        "activeFeatures": security.active_features,
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
        "security": diagnostics_security_payload(security),
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

pub fn pin_item(
    state: &AppState,
    node_id: i64,
    item_kind: String,
    project_id: Option<i64>,
) -> Result<(), String> {
    state
        .db()?
        .pin_item_for_project(node_id, &item_kind, project_id)
        .map_err(to_message)
}

pub fn unpin_item(
    state: &AppState,
    node_id: i64,
    item_kind: String,
    project_id: Option<i64>,
) -> Result<(), String> {
    state
        .db()?
        .unpin_item_for_project(node_id, &item_kind, project_id)
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
#[cfg(feature = "agent_automation")]
pub fn comment_write_enabled(state: &AppState) -> Result<bool, String> {
    state
        .db()?
        .comment_write_enabled_value()
        .map_err(to_message)
}

#[cfg(feature = "agent_automation")]
pub fn set_comment_write_enabled(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_comment_write_enabled(enabled)
        .map_err(to_message)
}

/// Whether the "AI total control" tier is enabled (default OFF, heavily signposted).
/// Even when on, irreversible or human-data-destroying actions still require the
/// in-app double confirmation with a backup offer.
#[cfg(feature = "agent_automation")]
pub fn mcp_full_control_enabled(state: &AppState) -> Result<bool, String> {
    state
        .db()?
        .mcp_full_control_enabled_value()
        .map_err(to_message)
}

#[cfg(feature = "agent_automation")]
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
/// advertises no scoped tools. This does NOT relax enforcement: every
/// `tools/call` still runs the full authenticated, scope- and toggle-gated dispatch;
/// filtering the advertised list is a UX affordance so an app is not shown tools it
/// cannot use, and `tools/list` is per-session so a per-token view is spec-legal.
#[cfg(feature = "agent_automation")]
#[derive(Debug, Clone)]
pub struct McpCatalogContext {
    /// The transport-bound app's granted scopes, or `None` for an unbound
    /// protocol-only test/session.
    pub scopes: Option<Vec<String>>,
    /// The default-OFF advanced-request tier. Comment-change requests additionally
    /// need the base `comments_write` scope; disk-action requests need `execute_plan`.
    pub total_control_enabled: bool,
    /// The default-OFF local permanent-removal capability. Connected-app review
    /// recommendations are also hidden while the owner keeps this capability off.
    pub final_remove_enabled: bool,
}

#[cfg(feature = "agent_automation")]
impl McpCatalogContext {
    /// Whether the bound app holds a given scope. False when no identity resolved.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes
            .as_ref()
            .map(|scopes| scopes.iter().any(|candidate| candidate == scope))
            .unwrap_or(false)
    }
}

#[cfg(feature = "agent_automation")]
pub fn mcp_read_only_mode(state: &AppState) -> Result<bool, String> {
    state.db()?.mcp_read_only_mode_value().map_err(to_message)
}

#[cfg(feature = "agent_automation")]
pub fn set_mcp_read_only_mode(state: &AppState, enabled: bool) -> Result<(), String> {
    state
        .db()?
        .set_mcp_read_only_mode(enabled)
        .map_err(to_message)
}

fn validated_shell_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("No local file or folder path was supplied.".to_string());
    }
    if trimmed.contains("://") {
        return Err("Code Hangar shell-open accepts local paths, not URLs.".to_string());
    }
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err(
            "Code Hangar shell-open accepts local drives, not UNC or network paths.".to_string(),
        );
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err("Code Hangar shell-open requires an absolute local path.".to_string());
    }
    reject_remote_windows_drive(&candidate)?;
    if candidate
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(
            "Code Hangar shell-open does not accept parent-directory traversal.".to_string(),
        );
    }
    Ok(candidate)
}

fn canonical_shell_path(path: &str) -> Result<PathBuf, String> {
    let candidate = validated_shell_path(path)?;
    hangar_fs::validate_local_content_ancestors(&candidate).map_err(|error| {
        format!(
            "Code Hangar will not open a path through a linked, junction or cloud-only parent folder: {error}"
        )
    })?;
    let identity = hangar_fs::inspect_path_identity(&candidate);
    if identity.inaccessible {
        return Err(identity
            .error
            .unwrap_or_else(|| "Cannot inspect this local path.".to_string()));
    }
    // Keep the validated absolute spelling. `Path::canonicalize` follows a
    // mutable path and can itself touch a just-swapped junction/UNC target
    // before a post-check rejects it. Every actual read/root operation below
    // performs its own no-follow/no-recall proof instead.
    Ok(candidate)
}

#[cfg(windows)]
fn reject_remote_windows_drive(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return Err("Code Hangar shell-open requires a local Windows drive path.".to_string());
    }
    let root = format!("{}:\\", bytes[0] as char);
    let root = root
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `root` is a NUL-terminated drive-root string that lives for the
    // duration of the call. GetDriveTypeW reads no caller-owned output buffer.
    if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE {
        return Err("Code Hangar shell-open does not scan mapped network drives.".to_string());
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_remote_windows_drive(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn comparable_local_path(path: &Path) -> String {
    display_path_for_path(&path.to_string_lossy())
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn local_path_is_within(path: &Path, root: &Path) -> bool {
    let path = comparable_local_path(path);
    let root = comparable_local_path(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn relative_shell_path(root: &Path, target: &Path) -> Result<String, String> {
    if !local_path_is_within(target, root) {
        return Err("The selected file is outside the resolved project root.".to_string());
    }
    let root_parts = root.components().count();
    let relative = target
        .components()
        .skip(root_parts)
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if relative.is_empty() {
        return Err("The selected path resolves to the project root, not a file.".to_string());
    }
    Ok(relative)
}

fn known_project_root_for_target(db: &Db, target: &Path) -> Result<Option<PathBuf>, String> {
    let mut matches = db
        .registered_project_paths()
        .map_err(to_message)?
        .into_iter()
        .filter_map(|path| {
            let root = PathBuf::from(path);
            local_path_is_within(target, &root).then_some(root)
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|root| std::cmp::Reverse(comparable_local_path(root).len()));
    Ok(matches.into_iter().next())
}

fn shell_target_kind(target: &Path) -> Result<(&'static str, bool), String> {
    let metadata = std::fs::symlink_metadata(target)
        .map_err(|err| format!("Cannot inspect this local path: {err}"))?;
    if metadata.is_dir() {
        Ok(("folder", true))
    } else if metadata.is_file() {
        Ok(("file", false))
    } else {
        Err("The selected path is not a regular file or folder.".to_string())
    }
}

fn automatic_project_root_for_target(target: &Path, is_folder: bool) -> Result<PathBuf, String> {
    hangar_discovery::nearest_project_root_for_path(target)
        .or_else(|| {
            if is_folder {
                Some(target.to_path_buf())
            } else {
                target.parent().map(Path::to_path_buf)
            }
        })
        .ok_or_else(|| "The selected file has no parent folder to scan.".to_string())
}

fn viewer_root_for_target(target: &Path, is_folder: bool) -> Result<PathBuf, String> {
    if is_folder {
        Ok(target.to_path_buf())
    } else {
        target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "The selected file has no parent folder to view.".to_string())
    }
}

fn canonical_shell_root(root: &Path, label: &str) -> Result<PathBuf, String> {
    hangar_fs::validate_local_scan_root(root)
        .map_err(|err| format!("Cannot safely resolve the {label}: {err}"))
}

/// Read a shell-supplied file without requiring the inventory database to have
/// opened. This is the cold-start fast path: it performs the same bounded,
/// no-recall and Protected Zone checks as the later project-scoped preview, but
/// creates no root, project, scan job, Recent item or other persistent state.
pub fn open_local_file_preview(
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<Option<DirectFilePreview>, String> {
    open_local_file_preview_with_budget(path, mode, policy, true)
}

/// Complete the same DB-independent local preview after the first frame has
/// painted. Revalidation and `OPEN_NO_RECALL` are intentionally repeated so no
/// file body or cloud/reparse decision is cached across the paint boundary.
pub fn open_local_file_preview_full(
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<Option<DirectFilePreview>, String> {
    open_local_file_preview_with_budget(path, mode, policy, false)
}

fn open_local_file_preview_with_budget(
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
    first_frame: bool,
) -> Result<Option<DirectFilePreview>, String> {
    let requested = validated_shell_path(&path)?;
    let target = requested.clone();
    // Prove the parent chain before even asking for final-entry metadata. That
    // avoids walking through a cloud-only directory or a junction merely to
    // classify its apparent child. An unsafe chain becomes a blocked,
    // metadata-only file response; no target metadata or body is touched.
    let ancestor_error = hangar_fs::validate_local_content_ancestors(&requested).err();
    let (mut identity, is_folder) = if let Some(error) = ancestor_error {
        (
            hangar_core::FileIdentity {
                size_apparent: None,
                size_allocated: None,
                modified_at: None,
                readonly: false,
                is_symlink: false,
                is_reparse: true,
                reparse_kind: Some("unsafe_path_ancestor".to_string()),
                volume_id: None,
                inode_key: None,
                link_count: None,
                inaccessible: false,
                error: Some(error.to_string()),
            },
            false,
        )
    } else {
        // Metadata on the final entry is safe after the ancestor proof. The
        // content opener repeats the full proof and uses OPEN_NO_RECALL.
        let identity = hangar_fs::inspect_path_identity(&requested);
        let (_, is_folder) = shell_target_kind(&target)?;
        (identity, is_folder)
    };
    if let Err(error) = hangar_fs::validate_local_content_path(&requested) {
        // Preserve genuine final-entry cloud/link metadata, but make a parent
        // race fail the same metadata-only preview gate.
        if hangar_fs::identity_allows_local_content(&identity) {
            identity.is_reparse = true;
            identity.reparse_kind = Some("unsafe_path_ancestor".to_string());
            identity.error = Some(error.to_string());
        }
    }
    if is_folder {
        return Ok(None);
    }
    let viewer_root = viewer_root_for_target(&target, false)?;
    let relative = relative_shell_path(&viewer_root, &target)?;
    let requested_text = requested.to_string_lossy();
    let preview_target = hangar_db::TransientPreviewTarget {
        node_id: -1,
        project_id: -1,
        absolute_path: &requested_text,
        relative_path: &relative,
        policy_path: &requested_text,
        identity: &identity,
    };
    let preview = if first_frame {
        Db::transient_file_preview_first_frame(preview_target, mode, policy.unwrap_or_default())
    } else {
        Db::transient_file_preview(preview_target, mode, policy.unwrap_or_default())
    };
    Ok(Some(DirectFilePreview {
        input_path: display_path_for_path(&target.to_string_lossy()),
        viewer_root: display_path_for_path(&viewer_root.to_string_lossy()),
        preview,
    }))
}

/// Inspect a local shell path without registering a root or starting a scan.
/// Known paths can open immediately; unknown paths must first pass through the
/// Viewer / Automatic / Manual choice in the frontend.
pub fn inspect_open_target(state: &AppState, path: String) -> Result<OpenTargetInspection, String> {
    let target = canonical_shell_path(&path)?;
    let (target_kind, is_folder) = shell_target_kind(&target)?;
    let discovery_target = PathBuf::from(display_path_for_path(&target.to_string_lossy()));
    let db = state.db()?;
    let known = known_project_root_for_target(&db, &discovery_target)?
        .map(|root| canonical_shell_root(&root, "known project root"))
        .transpose()?;
    let viewer = canonical_shell_root(
        &viewer_root_for_target(&discovery_target, is_folder)?,
        "Viewer root",
    )?;
    // A file association opens an unknown file in Viewer mode immediately; it
    // never presents the Automatic/Manual folder dialog. Avoid probing every
    // ancestor for project markers in that common path. Known targets already
    // have their correct root. Only an unknown folder needs marker discovery.
    let automatic = if let Some(known_root) = known.as_ref() {
        known_root.clone()
    } else if is_folder {
        canonical_shell_root(
            &automatic_project_root_for_target(&discovery_target, true)?,
            "automatic project root",
        )?
    } else {
        viewer.clone()
    };

    Ok(OpenTargetInspection {
        input_path: display_path_for_path(&target.to_string_lossy()),
        target_kind: target_kind.to_string(),
        known_project_root: known
            .as_ref()
            .map(|root| display_path_for_path(&root.to_string_lossy())),
        automatic_project_root: display_path_for_path(&automatic.to_string_lossy()),
        viewer_root: display_path_for_path(&viewer.to_string_lossy()),
    })
}

fn parse_shell_open_mode(mode: &str) -> Result<&'static str, String> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "known" => Ok("known"),
        "viewer" => Ok("viewer"),
        "automatic" => Ok("automatic"),
        "manual" => Ok("manual"),
        _ => Err("Choose Viewer, Automatic, or Manual before opening this path.".to_string()),
    }
}

/// Resolve a local path supplied by Explorer or an OS file association using
/// the explicit opening mode chosen by the user. This function deliberately
/// does not start a new scan: the caller must be able to render the requested
/// file before any inventory work competes for the database or filesystem.
/// Viewer roots stay ad-hoc and hidden from discovery.
pub fn prepare_open_target(
    state: &AppState,
    path: String,
    open_mode: String,
    manual_root: Option<String>,
    _performance_mode: Option<String>,
) -> Result<OpenTargetPreparation, String> {
    let target = canonical_shell_path(&path)?;
    let (target_kind, is_folder) = shell_target_kind(&target)?;
    let open_mode = parse_shell_open_mode(&open_mode)?;
    let db = state.db()?;
    // Windows canonical paths often carry a `\\?\` prefix. The scanner and
    // discovery marker checks intentionally work with the ordinary display form
    // so marker joins and stored project paths share one representation.
    let discovery_target = PathBuf::from(display_path_for_path(&target.to_string_lossy()));
    // Viewer and Manual already carry an explicit boundary; querying the full
    // registered-root list again would add encrypted DB work before the direct
    // preview without changing either decision.
    let known_root = if matches!(open_mode, "known" | "automatic") {
        known_project_root_for_target(&db, &discovery_target)?
    } else {
        None
    };
    let unresolved_root = match open_mode {
        "known" => known_root.ok_or_else(|| {
            "This folder is not part of a project Code Hangar already knows. Choose Viewer, Automatic, or Manual.".to_string()
        })?,
        "automatic" => known_root.unwrap_or(automatic_project_root_for_target(
            &discovery_target,
            is_folder,
        )?),
        "viewer" => viewer_root_for_target(&discovery_target, is_folder)?,
        "manual" => {
            let manual = manual_root
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Choose the project root folder for Manual mode.".to_string())?;
            let manual = canonical_shell_path(&manual)?;
            // A manual root comes from a shell/UI boundary. `Path::is_dir`
            // follows the final entry, so a junction could be traversed before
            // the scan-root gate had a chance to reject it. Validate the exact
            // final directory with the same no-follow policy the worker uses.
            let manual = canonical_shell_root(&manual, "Manual project root")?;
            if !local_path_is_within(&target, &manual) {
                return Err(
                    "The selected Manual root must contain the file or folder being opened."
                        .to_string(),
                );
            }
            manual
        }
        _ => unreachable!("shell-open mode was validated"),
    };
    let project_root = canonical_shell_root(&unresolved_root, "project root")?;
    // Keep the database identity in the same canonical representation used by
    // ordinary scan-root registration (`\\?\C:\...` on Windows). Display paths
    // are stripped only at API/UI boundaries. Otherwise Viewer followed by
    // Manual/Automatic could create two textual rows for one real directory.
    let stored_root = project_root.to_string_lossy().into_owned();

    let root = if open_mode == "viewer" {
        // Ad-hoc roots are intentionally absent from project/discovery
        // snapshots, so invalidating (and synchronously rewriting) the encrypted
        // catalog cache here would add I/O to a plain file-open for no benefit.
        db.roots_add_adhoc(&stored_root).map_err(to_message)?
    } else {
        roots_add(state, stored_root.clone())?
    };
    let project_id = db
        .project_id_for_root_path(&stored_root)
        .map_err(to_message)?
        .ok_or_else(|| {
            "Code Hangar registered the folder but could not resolve its project row.".to_string()
        })?;
    let node_id = if !is_folder {
        let relative = relative_shell_path(&project_root, &target)?;
        db.nav_node_for_relative_path(project_id, &relative)
            .map_err(to_message)?
    } else {
        None
    };

    // Preserve an existing refresh, but never create one on the preview's
    // critical path. `start_open_target_scan` is called after the frontend has
    // rendered/yielded the directly requested document.
    let scan_job_id = state.jobs.running_job_id_for_root(root.id);
    let scan_already_running = scan_job_id.is_some();

    Ok(OpenTargetPreparation {
        input_path: display_path_for_path(&target.to_string_lossy()),
        target_kind: target_kind.to_string(),
        project_root: display_path_for_path(&project_root.to_string_lossy()),
        project_id,
        root_id: root.id,
        node_id,
        scan_job_id,
        scan_already_running,
        open_mode: open_mode.to_string(),
        temporary: db.root_is_adhoc(root.id).map_err(to_message)?,
    })
}

/// Start (or reuse) the focused inventory refresh for an already prepared
/// Explorer target. Kept separate from [`prepare_open_target`] so the direct
/// file preview always wins the latency race against scanning and indexing.
pub fn start_open_target_scan(
    state: &AppState,
    root_id: i64,
    performance_mode: Option<String>,
) -> Result<OpenTargetScanStart, String> {
    let outcome = scan_start_internal(state, Some(vec![root_id]), performance_mode, true)?;
    Ok(OpenTargetScanStart {
        job_id: outcome.job_id,
        started_here: outcome.started_here,
    })
}

/// Read exactly the file requested by Explorer without waiting for a project
/// inventory scan. The target is revalidated as local and contained by the
/// chosen project/viewer root, then the ordinary bounded preview policy is
/// applied directly to that path. A negative node id means the background scan
/// has not indexed it yet; the frontend replaces it when the real node resolves.
pub fn open_target_preview(
    state: &AppState,
    project_id: i64,
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    let requested = PathBuf::from(path.trim());
    let target = canonical_shell_path(&path)?;
    if !matches!(shell_target_kind(&target), Ok(("file", false))) {
        return Err("The shell preview target must be a regular local file.".to_string());
    }
    let db = state.db()?;
    let project_path = db
        .project_path(project_id)
        .map_err(to_message)?
        .ok_or_else(|| "The project/viewer is no longer available.".to_string())?;
    let root = hangar_fs::validate_local_scan_root(Path::new(&project_path))
        .map_err(|err| format!("Cannot safely resolve the project root: {err}"))?;
    if !local_path_is_within(&target, &root) {
        return Err("The requested file is outside the selected project/viewer root.".to_string());
    }
    let relative = relative_shell_path(&root, &target)?;
    let node_id = db
        .nav_node_for_relative_path(project_id, &relative)
        .map_err(to_message)?
        .unwrap_or(-1);
    // Inspect the originally requested entry, not only its canonical target. A
    // symlink/reparse point must remain blocked even when it points back inside
    // the project, and cloud placeholders must never hydrate on preview.
    let identity = hangar_fs::inspect_path_identity(&requested);
    Ok(Db::transient_file_preview(
        hangar_db::TransientPreviewTarget {
            node_id,
            project_id,
            absolute_path: &requested.to_string_lossy(),
            relative_path: &relative,
            policy_path: &requested.to_string_lossy(),
            identity: &identity,
        },
        mode,
        policy.unwrap_or_default(),
    ))
}

/// Resolve the file node after the scan started by [`start_open_target_scan`]
/// finishes. The owning project is re-checked so a stale caller cannot resolve a
/// file through a different project boundary.
pub fn resolve_open_target(
    state: &AppState,
    project_id: i64,
    path: String,
) -> Result<Option<i64>, String> {
    let target = canonical_shell_path(&path)?;
    if !matches!(shell_target_kind(&target), Ok(("file", false))) {
        return Ok(None);
    }
    let db = state.db()?;
    let project = db
        .project_get(project_id)
        .map_err(to_message)?
        .ok_or_else(|| "The project is no longer registered in Code Hangar.".to_string())?;
    let root = hangar_fs::validate_local_scan_root(Path::new(&project.path))
        .map_err(|err| format!("Cannot safely resolve the registered project root: {err}"))?;
    let relative = relative_shell_path(&root, &target)?;
    db.nav_node_for_relative_path(project_id, &relative)
        .map_err(to_message)
}

pub fn roots_list(state: &AppState) -> Result<Vec<ScanRoot>, String> {
    state.db()?.roots_list().map_err(to_message)
}

pub fn roots_add(state: &AppState, path: String) -> Result<ScanRoot, String> {
    let normalized = normalize_root_path(path)?;
    let db = state.db()?;
    let was_registered = db
        .roots_list()
        .map_err(to_message)?
        .iter()
        .any(|root| same_local_path(&root.path, &normalized));
    let root = db.roots_add(&normalized).map_err(to_message)?;
    if !was_registered {
        state.invalidate_project_caches();
    }
    Ok(root)
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
    state.db()?.roots_unregister(root_id).map_err(to_message)?;
    state.invalidate_project_caches();
    Ok(())
}

pub fn projects_unregister(state: &AppState, project_id: i64) -> Result<(), String> {
    state
        .db()?
        .project_unregister(project_id)
        .map_err(to_message)?;
    state.invalidate_project_caches();
    Ok(())
}

/// Reset all: unregister every scan root and every real project at once, in one
/// atomic transaction. Demo projects are kept; files on disk are never touched.
/// Returns the number of real projects removed.
pub fn reset_all_projects(state: &AppState) -> Result<u64, String> {
    if state.jobs.has_any_running_job() {
        return Err("Cancel the active scan before resetting all projects.".to_string());
    }
    let removed = state.db()?.reset_local_inventory().map_err(to_message)?;
    state.invalidate_project_caches();
    Ok(removed)
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
    scan_start_internal(state, root_ids, performance_mode, false).map(|outcome| outcome.job_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanStartOutcome {
    job_id: String,
    started_here: bool,
}

fn scan_start_internal(
    state: &AppState,
    root_ids: Option<Vec<i64>>,
    performance_mode: Option<String>,
    reuse_running_job: bool,
) -> Result<ScanStartOutcome, String> {
    let root_ids = root_ids.unwrap_or_default();
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    let db = state.db()?;
    let targets = db.scan_targets_for_ids(&root_ids).map_err(to_message)?;
    let target_root_ids: Vec<i64> = targets.iter().map(|target| target.root_id).collect();
    let cached_estimate = db
        .complete_scan_estimate_for_roots(&target_root_ids)
        .map_err(to_message)?;
    let admission = state.jobs.admit_running_for_roots_with_estimate(
        if let Some(estimate) = cached_estimate {
            format!(
                "Using previous inventory estimate: {} items. Starting local metadata scan.",
                estimate
            )
        } else if mode.is_background() {
            "Starting a low-resource incremental background scan.".to_string()
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
    let job_id = match admission {
        RunningJobAdmission::Created(job_id) => job_id,
        RunningJobAdmission::Existing { job_id, .. } if reuse_running_job => {
            return Ok(ScanStartOutcome {
                job_id,
                started_here: false,
            });
        }
        RunningJobAdmission::Existing { root_id, .. } => {
            let path = targets
                .iter()
                .find(|target| target.root_id == root_id)
                .map(|target| target.display_path.as_str())
                .unwrap_or("the requested root");
            return Err(format!("A scan is already running for {path}."));
        }
    };
    state
        .jobs
        .set_worker_count(&job_id, scan_limits(false, mode).worker_count as u64);
    let jobs = state.jobs.clone();
    let worker_job_id = job_id.clone();
    let inventory_mutation_gate = SharedArc::clone(&state.inventory_mutation_gate);

    thread::spawn(move || {
        let _inventory_guard = match inventory_mutation_gate.read() {
            Ok(guard) => guard,
            Err(_) => {
                jobs.fail(
                    &worker_job_id,
                    "Inventory/mutation coordination lock is poisoned.".to_string(),
                );
                return;
            }
        };
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
        } else if mode.is_background() {
            jobs.update_phase(
                &worker_job_id,
                "scanning",
                None,
                "Scanning incrementally with one low-priority background worker.",
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
        jobs.update_phase(
            &worker_job_id,
            "preparing",
            None,
            "Opening the local incremental inventory writer.",
        );
        let mut writer = match db.open_write_session() {
            Ok(writer) => writer,
            Err(err) => {
                jobs.fail(&worker_job_id, to_message(err));
                return;
            }
        };

        if jobs.is_cancelled(&worker_job_id) {
            jobs.cancel(&worker_job_id, scanned, indexed);
            return;
        }

        for target in targets {
            if jobs.is_cancelled(&worker_job_id) {
                jobs.cancel(&worker_job_id, scanned, indexed);
                return;
            }

            if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
                jobs.cancel(&worker_job_id, scanned, indexed);
                return;
            }

            jobs.update_phase(
                &worker_job_id,
                "preparing",
                None,
                "Loading the previous inventory snapshot for incremental comparison.",
            );
            let project_id = match writer.begin_root_scan(&target.raw_path) {
                Ok(project_id) => project_id,
                Err(err) => {
                    jobs.fail(&worker_job_id, to_message(err));
                    return;
                }
            };

            if jobs.is_cancelled(&worker_job_id) {
                jobs.cancel(&worker_job_id, scanned, indexed);
                return;
            }

            let cached_body_fingerprints = mode
                .is_background()
                .then(|| writer.active_root_body_fingerprints(project_id));

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
            jobs.update_phase(
                &worker_job_id,
                "scanning",
                None,
                "Scanning local metadata with the low-resource resident profile.",
            );
            let scan_result = hangar_fs::scan_inventory_stream(
                Path::new(&target.raw_path),
                None,
                limits,
                cached_body_fingerprints.as_ref(),
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
                    if let Some(pause) = mode.batch_pause() {
                        thread::sleep(pause);
                    }
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
                    refresh_derived: !mode.is_background(),
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

    Ok(ScanStartOutcome {
        job_id,
        started_here: true,
    })
}

pub fn scan_resume_subtree(
    state: &AppState,
    nav_id: i64,
    performance_mode: Option<String>,
) -> Result<String, String> {
    let db = state.db()?;
    let target = db.subtree_scan_target(nav_id).map_err(to_message)?;
    let mode = PerformanceMode::parse(performance_mode.as_deref());
    let admission = state.jobs.admit_running_for_roots_with_estimate(
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
    let job_id = match admission {
        RunningJobAdmission::Created(job_id) => job_id,
        RunningJobAdmission::Existing { .. } => {
            return Err(format!(
                "A scan is already running for {}.",
                target.display_root_path
            ));
        }
    };
    state
        .jobs
        .set_worker_count(&job_id, scan_limits(true, mode).worker_count as u64);
    let jobs = state.jobs.clone();
    let worker_job_id = job_id.clone();
    let inventory_mutation_gate = SharedArc::clone(&state.inventory_mutation_gate);

    thread::spawn(move || {
        let _inventory_guard = match inventory_mutation_gate.read() {
            Ok(guard) => guard,
            Err(_) => {
                jobs.fail(
                    &worker_job_id,
                    "Inventory/mutation coordination lock is poisoned.".to_string(),
                );
                return;
            }
        };
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
            None,
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
    // Callers may already hold Rust's canonical Windows `\\?\C:\...`
    // representation. Convert only that display prefix before validating the
    // local drive; the final stored value is canonicalized again below.
    let path_buf = PathBuf::from(display_path_for_path(&path));
    if !path_buf.is_absolute() {
        return Err("Cannot register scan root: an absolute local path is required.".to_string());
    }
    reject_remote_windows_drive(&path_buf)?;
    let canonical = hangar_fs::validate_local_scan_root(&path_buf)
        .map_err(|err| format!("Cannot safely register scan root: {err}"))?;
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
            #[cfg(feature = "agent_automation")]
            outbound_network: "disabled".to_string(),
            mutation_executor: "compiled".to_string(),
            #[cfg(feature = "agent_automation")]
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

    fn exercise_project_discovery_entrypoints(
        state: &AppState,
        root: &Path,
    ) -> (ProjectDiscoveryReport, ProjectDiscoveryReport) {
        let global = project_discovery_report(
            state,
            Some(1),
            Some(1),
            Some(false),
            Some(false),
            Some(false),
        )
        .expect("global discovery fixture");
        let folder = project_discovery_deep_scan(
            state,
            root.to_string_lossy().into_owned(),
            Some(1),
            Some(1),
            Some(false),
            Some(false),
            Some(false),
        )
        .expect("folder discovery fixture");
        (global, folder)
    }

    #[test]
    fn production_app_state_defaults_to_system_project_discovery_source() {
        let state = AppState::memory().unwrap();
        assert!(matches!(
            state.project_discovery_source,
            ProjectDiscoverySource::System
        ));
    }

    #[test]
    fn project_discovery_entrypoints_never_select_wsl_source_when_opted_out() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = tempfile::tempdir().expect("folder discovery fixture");
        let wsl_accesses = SharedArc::new(AtomicUsize::new(0));
        let observed = SharedArc::clone(&wsl_accesses);
        let mut state = AppState::memory().unwrap();
        state.db().unwrap().set_wsl_scan_enabled(false).unwrap();
        state.project_discovery_source = ProjectDiscoverySource::fixture(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            vec![(
                "FixtureDistro".to_string(),
                PathBuf::from("the OFF branch must never request fixture homes"),
            )]
        });

        let (global, folder) = hangar_discovery::with_wsl_scan_snapshot(true, || {
            exercise_project_discovery_entrypoints(&state, root.path())
        });
        assert_eq!(
            wsl_accesses.load(Ordering::SeqCst),
            0,
            "WSL OFF must not enumerate, spawn, or touch a WSL discovery source"
        );
        assert!(global
            .searched_locations
            .iter()
            .all(|source| !source.source_label.starts_with("WSL FixtureDistro")));
        assert!(folder
            .searched_locations
            .iter()
            .all(|source| !source.source_label.starts_with("WSL FixtureDistro")));
    }

    #[test]
    fn project_discovery_entrypoints_select_injected_wsl_source_when_opted_in() {
        let root = tempfile::tempdir().expect("folder discovery fixture");
        let fixture_home = root.path().join("fixture-home");
        std::fs::create_dir_all(fixture_home.join(".claude").join("projects")).unwrap();
        let observed = SharedArc::new(Mutex::new(Vec::new()));
        let observed_scopes = SharedArc::clone(&observed);
        let injected_home = fixture_home.clone();
        let mut state = AppState::memory().unwrap();
        state.db().unwrap().set_wsl_scan_enabled(true).unwrap();
        state.project_discovery_source = ProjectDiscoverySource::fixture(move |scope| {
            observed_scopes.lock().unwrap().push(scope);
            vec![("FixtureDistro".to_string(), injected_home.clone())]
        });

        let (global, folder) = hangar_discovery::with_wsl_scan_snapshot(false, || {
            exercise_project_discovery_entrypoints(&state, root.path())
        });
        assert_eq!(
            *observed.lock().unwrap(),
            vec![ProjectDiscoveryScope::Global, ProjectDiscoveryScope::Folder],
            "WSL ON must select the injected source once at each production entrypoint"
        );
        for report in [&global, &folder] {
            assert!(
                report.searched_locations.iter().any(|source| {
                    source.source_label.starts_with("WSL FixtureDistro")
                        && source.path.contains("fixture-home")
                }),
                "the real discovery implementation did not consume the injected WSL home"
            );
            assert!(
                report
                    .searched_locations
                    .iter()
                    .all(|source| !source.path.to_ascii_lowercase().starts_with(r"\\wsl")),
                "the controlled fixture must never redirect discovery to a host WSL share"
            );
        }
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
        let home = tempfile::tempdir().unwrap();
        let error = mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Cursor,
            home.path(),
            None,
            Vec::new(),
            false,
            false,
        )
        .expect_err("an empty project scope must fail closed");
        assert_eq!(
            error,
            "Choose at least one project before connecting an AI app."
        );
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_orphans_remain_db_revocable_without_touching_external_configs() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let home = tempfile::tempdir().unwrap();
        let project_id = db.projects_list().unwrap()[0].id;
        for host in hangar_appconfig::Host::ALL {
            connected_app_test_initial_registration(&state, host, home.path(), project_id);
        }

        // Missing config.
        let missing_host = hangar_appconfig::Host::Claude;
        let missing_path = hangar_appconfig::host_config_path(missing_host, home.path());
        fs::remove_file(&missing_path).unwrap();

        // Malformed config.
        let malformed_host = hangar_appconfig::Host::Codex;
        let malformed_path = hangar_appconfig::host_config_path(malformed_host, home.path());
        let malformed_bytes = b"[mcp_servers.code-hangar\nowner bytes are malformed".to_vec();
        fs::write(&malformed_path, &malformed_bytes).unwrap();

        // Valid config with no Code Hangar entry.
        let absent_entry_host = hangar_appconfig::Host::Cursor;
        let absent_entry_path = hangar_appconfig::host_config_path(absent_entry_host, home.path());
        let mut absent_entry: serde_json::Value =
            serde_json::from_slice(&fs::read(&absent_entry_path).unwrap()).unwrap();
        absent_entry["mcpServers"]
            .as_object_mut()
            .unwrap()
            .remove("code-hangar");
        let absent_entry_bytes = serde_json::to_vec_pretty(&absent_entry).unwrap();
        fs::write(&absent_entry_path, &absent_entry_bytes).unwrap();

        for host in [missing_host, malformed_host, absent_entry_host] {
            let before = connected_app_effective_status(&db, host, home.path(), false).unwrap();
            assert!(
                before.credential_orphaned,
                "{} should be orphaned",
                host.id()
            );
            assert!(before.durable_credential_enabled);
            assert!(!before.credential_active);

            let revoked = mcp_appconfig_revoke_orphan_at(&state, host, home.path()).unwrap();
            assert!(!revoked.durable_credential_enabled);
            assert!(!revoked.credential_active);
            assert!(revoked.durable_agent_id.is_some());

            let forgotten = mcp_appconfig_forget_orphan_at(&state, host, home.path()).unwrap();
            assert!(forgotten.durable_agent_id.is_none());
            assert!(db
                .connected_app_binding(core_connected_app_host(host))
                .unwrap()
                .is_some());
        }

        assert!(!missing_path.exists());
        assert_eq!(fs::read(&malformed_path).unwrap(), malformed_bytes);
        assert_eq!(fs::read(&absent_entry_path).unwrap(), absent_entry_bytes);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn invalid_connected_app_scope_does_not_revoke_the_current_credential() {
        let state = AppState::memory().unwrap();
        let home = tempfile::tempdir().unwrap();
        let existing =
            register_test_automation(&state, "Cursor", "existing-token", &["read_structure"], &[]);

        let error = mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Cursor,
            home.path(),
            None,
            vec![9_999],
            false,
            false,
        )
        .expect_err("an unknown project must be rejected before credential rotation");
        assert_eq!(error, "One or more selected projects no longer exist.");
        let agents = state.db().unwrap().automation_agents().unwrap();
        assert!(agents
            .iter()
            .any(|agent| agent.id == existing.id && agent.enabled));
    }

    #[cfg(feature = "agent_automation")]
    fn connected_app_test_spec(
        host: hangar_appconfig::Host,
        token: &str,
        identity_id: &str,
    ) -> hangar_appconfig::ServerSpec {
        hangar_appconfig::ServerSpec {
            command: r"C:\Apps\code-hangar-mcp.exe".to_string(),
            args: Vec::new(),
            env: vec![
                ("CODEHANGAR_MCP_TOKEN".to_string(), token.to_string()),
                ("CODEHANGAR_MCP_HOST".to_string(), host.id().to_string()),
                (
                    "CODEHANGAR_MCP_AGENT_ID".to_string(),
                    identity_id.to_string(),
                ),
            ],
            startup_timeout_sec: 20,
        }
    }

    #[cfg(feature = "agent_automation")]
    fn connected_app_test_initial_registration(
        state: &AppState,
        host: hangar_appconfig::Host,
        home: &Path,
        project_id: i64,
    ) -> (
        String,
        AutomationAgentSummary,
        hangar_appconfig::RegistrationBinding,
    ) {
        mcp_appconfig_register_at(
            state,
            host,
            home,
            Some(&home.join("code-hangar-mcp.exe")),
            vec![project_id],
            false,
            false,
        )
        .unwrap();
        let db = state.db().unwrap();
        let old_hash = hangar_appconfig::inspect(host, home)
            .configured_token_hash
            .unwrap();
        let agent = db
            .automation_authenticate_for_transport(&old_hash, Some(AutomationTransport::McpStdio))
            .unwrap()
            .unwrap();
        let durable = db
            .connected_app_binding(core_connected_app_host(host))
            .unwrap()
            .unwrap();
        let registration_binding = hangar_appconfig::RegistrationBinding::from_hex(
            &durable.agent_identity_id,
            &durable.state_auth_key_hex,
        )
        .unwrap();
        assert_eq!(agent.identity_id, durable.agent_identity_id);
        (old_hash, agent, registration_binding)
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_scope_sets_are_minimal_and_opt_in_independently() {
        let base = CONNECTED_APP_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect::<Vec<_>>();
        assert_eq!(connected_app_scopes(false, false), base);

        let mut history = base.clone();
        history.push("history_search".to_string());
        assert_eq!(connected_app_scopes(true, false), history);

        let mut execute = base.clone();
        execute.push("execute_plan".to_string());
        assert_eq!(connected_app_scopes(false, true), execute);

        let mut both = history;
        both.push("execute_plan".to_string());
        assert_eq!(connected_app_scopes(true, true), both);
        assert!(!connected_app_scopes(true, true)
            .iter()
            .any(|scope| scope == "read_body"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_reconnect_rotates_in_place_and_reports_exact_effective_access() {
        let state = AppState::memory().unwrap();
        let home = tempfile::tempdir().unwrap();
        let server = home.path().join("code-hangar-mcp.exe");
        let project_id = state.db().unwrap().projects_list().unwrap()[0].id;

        let first = mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Cursor,
            home.path(),
            Some(&server),
            vec![project_id],
            false,
            false,
        )
        .unwrap();
        assert!(first.credential_active);
        assert_eq!(first.effective_scopes, connected_app_scopes(false, false));
        assert_eq!(first.effective_project_ids, vec![project_id]);
        let old_hash = hangar_appconfig::inspect(hangar_appconfig::Host::Cursor, home.path())
            .configured_token_hash
            .unwrap();
        let old_agent = state
            .db()
            .unwrap()
            .automation_authenticate(&old_hash)
            .unwrap()
            .unwrap();

        let second = mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Cursor,
            home.path(),
            Some(&server),
            vec![project_id],
            true,
            true,
        )
        .unwrap();
        let new_hash = hangar_appconfig::inspect(hangar_appconfig::Host::Cursor, home.path())
            .configured_token_hash
            .unwrap();
        assert_ne!(old_hash, new_hash);
        assert!(state
            .db()
            .unwrap()
            .automation_authenticate(&old_hash)
            .unwrap()
            .is_none());
        let new_agent = state
            .db()
            .unwrap()
            .automation_authenticate(&new_hash)
            .unwrap()
            .unwrap();
        assert_eq!(
            new_agent.id, old_agent.id,
            "reconnect preserves comment identity"
        );
        assert_eq!(second.effective_scopes, connected_app_scopes(true, true));
        assert_eq!(second.effective_project_ids, vec![project_id]);
        assert!(second.credential_active);
        assert!(!second.recovery_required);
        assert!(state
            .db()
            .unwrap()
            .connected_app_change("cursor")
            .unwrap()
            .is_none());

        let config: serde_json::Value = serde_json::from_slice(
            &fs::read(hangar_appconfig::host_config_path(
                hangar_appconfig::Host::Cursor,
                home.path(),
            ))
            .unwrap(),
        )
        .unwrap();
        let raw_token = config["mcpServers"]["code-hangar"]["env"]["CODEHANGAR_MCP_TOKEN"]
            .as_str()
            .unwrap();
        let identity_id = config["mcpServers"]["code-hangar"]["env"]["CODEHANGAR_MCP_AGENT_ID"]
            .as_str()
            .unwrap();
        let binding = bind_mcp_transport(&state, raw_token, "cursor", identity_id).unwrap();
        let direct_execute = handle_automation_request(
            &state,
            automation_request(
                Some(raw_token),
                hangar_agent::AgentMethod::AgentPlanExecute,
                serde_json::json!({}),
            ),
        );
        assert!(!direct_execute.ok);
        let bound_execute = dispatch_mcp_bound_request(
            &state,
            &binding,
            "direct-execute".to_string(),
            hangar_agent::AgentMethod::AgentPlanExecute,
            serde_json::json!({}),
        );
        assert!(!bound_execute.ok);
        assert!(bound_execute.error.unwrap().contains("not allowed"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_disconnect_verifies_config_removal_before_revoking_token() {
        let state = AppState::memory().unwrap();
        let home = tempfile::tempdir().unwrap();
        let server = home.path().join("code-hangar-mcp.exe");
        let host = hangar_appconfig::Host::Claude;
        let project_id = state.db().unwrap().projects_list().unwrap()[0].id;
        mcp_appconfig_register_at(
            &state,
            host,
            home.path(),
            Some(&server),
            vec![project_id],
            false,
            false,
        )
        .unwrap();
        let token_hash = hangar_appconfig::inspect(host, home.path())
            .configured_token_hash
            .unwrap();
        assert!(state
            .db()
            .unwrap()
            .automation_authenticate(&token_hash)
            .unwrap()
            .is_some());

        let removed = mcp_appconfig_remove_at(&state, host, home.path()).unwrap();
        assert!(!removed.registered);
        assert!(!removed.credential_active);
        assert!(state
            .db()
            .unwrap()
            .automation_authenticate(&token_hash)
            .unwrap()
            .is_none());
        assert!(state
            .db()
            .unwrap()
            .connected_app_change(host.id())
            .unwrap()
            .is_none());
        assert!(!hangar_appconfig::pending_sidecars_present(host, home.path()).unwrap());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_reconcile_aborts_prepared_and_completes_config_written() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let home = tempfile::tempdir().unwrap();
        let host = hangar_appconfig::Host::Claude;
        let project_id = db.projects_list().unwrap()[0].id;
        let (old_hash, old_agent, registration_binding) =
            connected_app_test_initial_registration(&state, host, home.path(), project_id);

        let prepared_only = hangar_appconfig::prepare_register_authenticated(
            host,
            home.path(),
            &connected_app_test_spec(host, "never-written-token", &old_agent.identity_id),
            &registration_binding,
        )
        .unwrap();
        db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id: "prepared-only".to_string(),
            kind: "register".to_string(),
            agent_name: host.label().to_string(),
            new_token_hash: Some(automation_token_hash("never-written-token")),
            new_scopes: connected_app_scopes(true, false),
            new_project_ids: vec![project_id],
            fs: db_fs_contract(prepared_only.fingerprints()),
        })
        .unwrap();
        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        assert!(db.connected_app_change(host.id()).unwrap().is_none());
        assert_eq!(
            db.automation_authenticate(&old_hash).unwrap().unwrap().id,
            old_agent.id
        );
        assert!(db
            .automation_authenticate(&automation_token_hash("never-written-token"))
            .unwrap()
            .is_none());

        let new_token = "config-written-token";
        let new_hash = automation_token_hash(new_token);
        let config_written = hangar_appconfig::prepare_register_authenticated(
            host,
            home.path(),
            &connected_app_test_spec(host, new_token, &old_agent.identity_id),
            &registration_binding,
        )
        .unwrap();
        db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id: "config-written".to_string(),
            kind: "register".to_string(),
            agent_name: host.label().to_string(),
            new_token_hash: Some(new_hash.clone()),
            new_scopes: connected_app_scopes(true, true),
            new_project_ids: vec![project_id],
            fs: db_fs_contract(config_written.fingerprints()),
        })
        .unwrap();
        config_written.apply().unwrap();
        assert!(db.automation_authenticate(&new_hash).unwrap().is_none());
        assert!(db.automation_authenticate(&old_hash).unwrap().is_some());

        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        assert!(db.connected_app_change(host.id()).unwrap().is_none());
        assert!(db.automation_authenticate(&old_hash).unwrap().is_none());
        assert_eq!(
            db.automation_authenticate(&new_hash).unwrap().unwrap().id,
            old_agent.id
        );
        assert!(!hangar_appconfig::pending_sidecars_present(host, home.path()).unwrap());
        let effective = connected_app_effective_status(&db, host, home.path(), false).unwrap();
        assert_eq!(effective.effective_scopes, connected_app_scopes(true, true));
        assert_eq!(effective.effective_project_ids, vec![project_id]);
        assert!(effective.credential_active);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_db_rejection_restores_exact_config_and_old_credential() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let home = tempfile::tempdir().unwrap();
        let host = hangar_appconfig::Host::Cursor;
        let project_id = db.projects_list().unwrap()[0].id;
        let (old_hash, old_agent, registration_binding) =
            connected_app_test_initial_registration(&state, host, home.path(), project_id);
        let config_path = hangar_appconfig::host_config_path(host, home.path());
        let sidecar_path = |suffix: &str| {
            let mut value = config_path.as_os_str().to_os_string();
            value.push(suffix);
            PathBuf::from(value)
        };
        let backup_path = sidecar_path(".codehangar.bak");
        let state_path = sidecar_path(".codehangar.state");
        let before_config = fs::read(&config_path).unwrap();
        let before_sidecars = [fs::read(&backup_path).ok(), fs::read(&state_path).ok()];

        let new_token = "db-failure-new-token";
        let new_hash = automation_token_hash(new_token);
        let prepared = hangar_appconfig::prepare_register_authenticated(
            host,
            home.path(),
            &connected_app_test_spec(host, new_token, &old_agent.identity_id),
            &registration_binding,
        )
        .unwrap();
        db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id: "db-rejection".to_string(),
            kind: "register".to_string(),
            agent_name: host.label().to_string(),
            new_token_hash: Some(new_hash.clone()),
            new_scopes: connected_app_scopes(true, false),
            new_project_ids: vec![project_id],
            fs: db_fs_contract(prepared.fingerprints()),
        })
        .unwrap();
        prepared.apply().unwrap();

        let error = db
            .connected_app_change_commit(host.id(), "not-the-owner")
            .expect_err("SQLite must reject a non-owner commit");
        let error = to_message(error);
        assert!(!error.contains(new_token));
        prepared.rollback().unwrap();
        assert!(db
            .connected_app_change_abort_prepared(host.id(), "db-rejection")
            .unwrap());

        assert_eq!(fs::read(&config_path).unwrap(), before_config);
        assert_eq!(fs::read(&backup_path).ok(), before_sidecars[0]);
        assert_eq!(fs::read(&state_path).ok(), before_sidecars[1]);
        assert!(db.automation_authenticate(&old_hash).unwrap().is_some());
        assert!(db.automation_authenticate(&new_hash).unwrap().is_none());
        assert!(!hangar_appconfig::pending_sidecars_present(host, home.path()).unwrap());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_ambiguous_config_fails_closed_without_overwrite() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let home = tempfile::tempdir().unwrap();
        let host = hangar_appconfig::Host::Codex;
        let project_id = db.projects_list().unwrap()[0].id;
        let (old_hash, old_agent, registration_binding) =
            connected_app_test_initial_registration(&state, host, home.path(), project_id);

        let new_token = "ambiguous-new-token";
        let new_hash = automation_token_hash(new_token);
        let prepared = hangar_appconfig::prepare_register_authenticated(
            host,
            home.path(),
            &connected_app_test_spec(host, new_token, &old_agent.identity_id),
            &registration_binding,
        )
        .unwrap();
        db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id: "ambiguous-config".to_string(),
            kind: "register".to_string(),
            agent_name: host.label().to_string(),
            new_token_hash: Some(new_hash.clone()),
            new_scopes: connected_app_scopes(true, true),
            new_project_ids: vec![project_id],
            fs: db_fs_contract(prepared.fingerprints()),
        })
        .unwrap();
        let config_path = hangar_appconfig::host_config_path(host, home.path());
        let malformed = b"[mcp_servers.code-hangar\nthis is neither before nor after";
        fs::write(&config_path, malformed).unwrap();

        let error = reconcile_connected_app_host(&db, host, home.path()).unwrap_err();
        assert!(error.contains("ambiguous or changed"));
        assert_eq!(fs::read(&config_path).unwrap(), malformed);
        assert!(db.automation_authenticate(&old_hash).unwrap().is_some());
        assert!(db.automation_authenticate(&new_hash).unwrap().is_none());
        assert_eq!(
            db.connected_app_change(host.id()).unwrap().unwrap().state,
            "prepared"
        );
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_absent_config_aborts_prepared_without_reconstruction() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let home = tempfile::tempdir().unwrap();
        let host = hangar_appconfig::Host::Cursor;
        let project_id = db.projects_list().unwrap()[0].id;
        let (old_hash, old_agent, registration_binding) =
            connected_app_test_initial_registration(&state, host, home.path(), project_id);

        let new_token = "absent-new-token";
        let new_hash = automation_token_hash(new_token);
        let prepared = hangar_appconfig::prepare_register_authenticated(
            host,
            home.path(),
            &connected_app_test_spec(host, new_token, &old_agent.identity_id),
            &registration_binding,
        )
        .unwrap();
        db.connected_app_change_begin(&hangar_db::ConnectedAppCredentialChangeStart {
            host: host.id().to_string(),
            operation_id: "externally-absent".to_string(),
            kind: "register".to_string(),
            agent_name: host.label().to_string(),
            new_token_hash: Some(new_hash.clone()),
            new_scopes: connected_app_scopes(true, true),
            new_project_ids: vec![project_id],
            fs: db_fs_contract(prepared.fingerprints()),
        })
        .unwrap();
        let config_path = hangar_appconfig::host_config_path(host, home.path());
        fs::remove_file(&config_path).unwrap();

        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        reconcile_connected_app_host(&db, host, home.path()).unwrap();
        assert!(
            !config_path.exists(),
            "recovery must not reconstruct owner-removed config bytes"
        );
        assert!(db.connected_app_change(host.id()).unwrap().is_none());
        assert!(db.automation_authenticate(&old_hash).unwrap().is_some());
        assert!(db.automation_authenticate(&new_hash).unwrap().is_none());
        assert!(!hangar_appconfig::pending_sidecars_present(host, home.path()).unwrap());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_concurrent_reconnects_are_serialized_per_host() {
        use std::sync::{Arc, Barrier};

        let state = Arc::new(AppState::memory().unwrap());
        let home = tempfile::tempdir().unwrap();
        let home_path = Arc::new(home.path().to_path_buf());
        let server_path = Arc::new(home.path().join("code-hangar-mcp.exe"));
        let project_id = state.db().unwrap().projects_list().unwrap()[0].id;
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for (history, execute) in [(true, false), (false, true)] {
            let state = Arc::clone(&state);
            let home_path = Arc::clone(&home_path);
            let server_path = Arc::clone(&server_path);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                mcp_appconfig_register_at(
                    &state,
                    hangar_appconfig::Host::Cursor,
                    &home_path,
                    Some(&server_path),
                    vec![project_id],
                    history,
                    execute,
                )
            }));
        }
        barrier.wait();
        for worker in workers {
            worker
                .join()
                .expect("connector worker must not panic")
                .expect("both reconnects serialize and succeed");
        }

        let inspection = hangar_appconfig::inspect(hangar_appconfig::Host::Cursor, &home_path);
        let final_hash = inspection.configured_token_hash.unwrap();
        let final_agent = state
            .db()
            .unwrap()
            .automation_authenticate(&final_hash)
            .unwrap()
            .expect("the exact final config token is active");
        assert_eq!(final_agent.project_ids, vec![project_id]);
        assert_eq!(
            state
                .db()
                .unwrap()
                .automation_agents()
                .unwrap()
                .iter()
                .filter(|agent| agent.enabled && agent.name == "Cursor")
                .count(),
            1
        );
        assert!(state
            .db()
            .unwrap()
            .connected_app_change("cursor")
            .unwrap()
            .is_none());
        assert!(!hangar_appconfig::pending_sidecars_present(
            hangar_appconfig::Host::Cursor,
            &home_path
        )
        .unwrap());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn connected_app_effective_access_is_isolated_by_host_config_token() {
        let state = AppState::memory().unwrap();
        let home = tempfile::tempdir().unwrap();
        let server = home.path().join("code-hangar-mcp.exe");
        let project_id = state.db().unwrap().projects_list().unwrap()[0].id;

        mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Cursor,
            home.path(),
            Some(&server),
            vec![project_id],
            true,
            false,
        )
        .unwrap();
        mcp_appconfig_register_at(
            &state,
            hangar_appconfig::Host::Claude,
            home.path(),
            Some(&server),
            vec![project_id],
            false,
            true,
        )
        .unwrap();

        let db = state.db().unwrap();
        let cursor =
            connected_app_effective_status(&db, hangar_appconfig::Host::Cursor, home.path(), false)
                .unwrap();
        let claude =
            connected_app_effective_status(&db, hangar_appconfig::Host::Claude, home.path(), false)
                .unwrap();
        assert_eq!(cursor.effective_scopes, connected_app_scopes(true, false));
        assert_eq!(claude.effective_scopes, connected_app_scopes(false, true));
        assert_eq!(cursor.effective_project_ids, vec![project_id]);
        assert_eq!(claude.effective_project_ids, vec![project_id]);
        assert!(cursor.credential_active && claude.credential_active);
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
        #[cfg(not(feature = "mutation"))]
        assert!(status.mutation_executor.contains("not compiled"));
        #[cfg(feature = "mutation")]
        assert!(status.mutation_executor.contains("feature-gated"));
        #[cfg(feature = "agent_automation")]
        assert!(status.outbound_network.contains("explicit"));
        #[cfg(feature = "agent_automation")]
        assert!(status.agent_ipc.contains("local named pipe"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn stored_provider_config_is_revalidated_before_authorizing_requests() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();

        db.set_ai_provider_config(&AiProviderConfig {
            mode: "api".to_string(),
            base_url: "http://api.example.com/v1".to_string(),
            model: "model".to_string(),
            format: "chat_completions".to_string(),
        })
        .unwrap();
        let error = resolve_ai_provider_config(&state)
            .expect_err("legacy cleartext remote state must fail closed");
        assert!(
            error.contains("must use https://"),
            "unexpected error: {error}"
        );

        db.set_ai_provider_config(&AiProviderConfig {
            mode: "local".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            model: "model".to_string(),
            format: "chat_completions".to_string(),
        })
        .unwrap();
        let error = resolve_ai_provider_config(&state)
            .expect_err("legacy non-loopback local state must fail closed");
        assert!(
            error.contains("must be on this machine"),
            "unexpected error: {error}"
        );

        db.set_ai_provider_config(&AiProviderConfig {
            mode: "api".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            model: "model".to_string(),
            format: "corrupt_format".to_string(),
        })
        .unwrap();
        let error = resolve_ai_provider_config(&state)
            .expect_err("unknown persisted provider format must fail closed");
        assert!(
            error.contains("Unknown AI provider format"),
            "unexpected error: {error}"
        );

        db.set_ai_provider_config(&AiProviderConfig {
            mode: "api".to_string(),
            base_url: "  https://api.example.com/v1  ".to_string(),
            model: "model".to_string(),
            format: "openai_compatible".to_string(),
        })
        .unwrap();
        let resolved = resolve_ai_provider_config(&state).unwrap();
        assert_eq!(resolved.base_url, "https://api.example.com/v1");
        assert_eq!(resolved.format, hangar_ai::ProviderFormat::ChatCompletions);
        assert!(!resolved.local);
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn provider_draft_format_is_strict_before_probe_transport() {
        let error = build_ai_provider_config(
            "api",
            "https://api.example.com/v1",
            "model",
            "corrupt_format",
        )
        .expect_err("unknown draft format must fail closed");
        assert!(error.contains("Unknown AI provider format"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn credential_binding_authorizes_only_exact_origin_key_and_version_snapshots() {
        let binding = hangar_core::AiProviderCredentialBinding {
            origin: "https://api-a.example:443".to_string(),
            fingerprint: "11".repeat(32),
            version: "binding-version-a-0123456789".to_string(),
            status: hangar_core::AiProviderCredentialBindingStatus::Active,
        };
        assert!(credential_binding_authorizes(
            &binding,
            "https://api-a.example:443",
            &binding.fingerprint,
        ));
        assert!(!credential_binding_authorizes(
            &binding,
            "https://api-b.example:443",
            &binding.fingerprint,
        ));
        assert!(!credential_binding_authorizes(
            &binding,
            &binding.origin,
            &"22".repeat(32),
        ));
        let loopback_binding = hangar_core::AiProviderCredentialBinding {
            origin: "http://127.0.0.1:4000".to_string(),
            ..binding.clone()
        };
        let effective_loopback_origin =
            hangar_ai::remote_credential_origin("http://localhost:4000/v1").unwrap();
        assert!(credential_binding_authorizes(
            &loopback_binding,
            &effective_loopback_origin,
            &loopback_binding.fingerprint,
        ));

        assert!(credential_binding_snapshot_matches(
            Some(&binding),
            Some(&binding),
            Some(&binding.fingerprint),
            Some(&binding.fingerprint),
        ));
        let rotated_version = hangar_core::AiProviderCredentialBinding {
            version: "binding-version-b-0123456789".to_string(),
            ..binding.clone()
        };
        assert!(!credential_binding_snapshot_matches(
            Some(&binding),
            Some(&rotated_version),
            Some(&binding.fingerprint),
            Some(&binding.fingerprint),
        ));
        assert!(!credential_binding_snapshot_matches(
            Some(&binding),
            Some(&binding),
            Some(&binding.fingerprint),
            Some(&"22".repeat(32)),
        ));
        assert!(!credential_binding_snapshot_matches(
            Some(&binding),
            None,
            Some(&binding.fingerprint),
            None,
        ));
        assert!(credential_binding_snapshot_matches(None, None, None, None));
        assert!(!credential_binding_snapshot_matches(
            None,
            Some(&binding),
            None,
            Some(&binding.fingerprint),
        ));

        let source = include_str!("lib.rs");
        for (start, end) in [
            (
                "pub fn ai_provider_test_disclosure(",
                "pub fn ai_provider_test(",
            ),
            (
                "pub fn ai_provider_models_disclosure(",
                "pub fn ai_provider_models(",
            ),
        ] {
            let body = source
                .split_once(start)
                .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
                .unwrap();
            assert!(body.contains("stage_ai_prepared_send("));
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn api_mode_localhost_binding_matches_test_and_models_preview_origin() {
        let config = build_ai_provider_config(
            "api",
            "http://localhost:4000/v1",
            "gateway-model",
            "chat_completions",
        )
        .unwrap();
        assert!(!config.local);
        let effective = hangar_ai::remote_credential_origin(&config.base_url).unwrap();
        assert_eq!(effective, "http://127.0.0.1:4000");
        let binding = hangar_core::AiProviderCredentialBinding {
            origin: effective.clone(),
            fingerprint: "33".repeat(32),
            version: "binding-local-gateway-0123456789".to_string(),
            status: hangar_core::AiProviderCredentialBindingStatus::Active,
        };

        for request in [
            ai_assist::ai_prepare_provider_test_with_config(&config).unwrap(),
            ai_assist::ai_prepare_provider_models_with_config(&config).unwrap(),
        ] {
            let preview_origin = hangar_ai::endpoint_origin(&request.disclosure().url).unwrap();
            assert_eq!(preview_origin, effective);
            assert!(credential_binding_authorizes(
                &binding,
                &preview_origin,
                &binding.fingerprint,
            ));
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn provider_change_clears_bound_or_legacy_keys_before_activation() {
        let binding = hangar_core::AiProviderCredentialBinding {
            origin: "https://api-a.example:443".to_string(),
            fingerprint: "11".repeat(32),
            version: "binding-version-a-0123456789".to_string(),
            status: hangar_core::AiProviderCredentialBindingStatus::Active,
        };
        assert!(!provider_set_requires_credential_clear(
            Some(&binding),
            Some(&binding.fingerprint),
            "api",
            "https://api-a.example/v2",
        )
        .unwrap());
        assert!(provider_set_requires_credential_clear(
            Some(&binding),
            Some(&binding.fingerprint),
            "api",
            "https://api-b.example/v1",
        )
        .unwrap());
        assert!(provider_set_requires_credential_clear(
            Some(&binding),
            Some(&binding.fingerprint),
            "off",
            "",
        )
        .unwrap());
        assert!(provider_set_requires_credential_clear(
            Some(&binding),
            Some(&binding.fingerprint),
            "local",
            "http://127.0.0.1:11434/v1",
        )
        .unwrap());
        assert!(provider_set_requires_credential_clear(
            None,
            Some(&binding.fingerprint),
            "api",
            "https://api-a.example/v1",
        )
        .unwrap());

        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let provider_a = AiProviderConfig {
            mode: "api".to_string(),
            base_url: "https://api-a.example/v1".to_string(),
            model: "a".to_string(),
            format: "chat_completions".to_string(),
        };
        db.set_ai_provider_config(&provider_a).unwrap();
        db.set_ai_provider_credential_binding(Some(&binding))
            .unwrap();
        let error = ai_provider_set_with_key_clear(
            &state,
            "api",
            "https://api-b.example/v1",
            "b",
            "chat_completions",
            Some(binding.fingerprint.clone()),
            || Err("injected key-store clear failure".to_string()),
        )
        .expect_err("B must not activate when clearing A fails");
        assert!(error.contains("not activated"), "{error}");
        assert_eq!(db.ai_provider_config().unwrap(), provider_a);
        assert!(db.ai_provider_credential_binding().unwrap().is_none());

        // Exact historical bypasses A -> Off -> B and A -> Local -> B both revoke before the
        // intermediate config is committed. B then activates without any stale binding.
        for intermediate in ["off", "local"] {
            db.set_ai_provider_config(&provider_a).unwrap();
            db.set_ai_provider_credential_binding(Some(&binding))
                .unwrap();
            let intermediate_url = if intermediate == "off" {
                ""
            } else {
                "http://127.0.0.1:11434/v1"
            };
            ai_provider_set_with_key_clear(
                &state,
                intermediate,
                intermediate_url,
                "",
                "chat_completions",
                Some(binding.fingerprint.clone()),
                || Ok(()),
            )
            .unwrap();
            assert!(db.ai_provider_credential_binding().unwrap().is_none());
            ai_provider_set_with_key_clear(
                &state,
                "api",
                "https://api-b.example/v1",
                "b",
                "chat_completions",
                None,
                || panic!("no stale credential remains to clear"),
            )
            .unwrap();
            assert_eq!(
                db.ai_provider_config().unwrap().base_url,
                "https://api-b.example/v1"
            );
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn key_set_requires_saved_api_origin_and_rolls_binding_back_on_failure() {
        let state = AppState::memory().unwrap();
        let db = state.db().unwrap();
        let called = std::cell::Cell::new(false);
        let error = ai_key_set_with_writer(&state, "key-0123456789", |_| {
            called.set(true);
            Ok(String::new())
        })
        .expect_err("first-use key must follow provider Save");
        assert!(error.contains("Save an API provider"), "{error}");
        assert!(!called.get());

        db.set_ai_provider_config(&AiProviderConfig {
            mode: "api".to_string(),
            base_url: "https://api-a.example/v1".to_string(),
            model: "a".to_string(),
            format: "chat_completions".to_string(),
        })
        .unwrap();
        let old_binding = hangar_core::AiProviderCredentialBinding {
            origin: "https://api-a.example:443".to_string(),
            fingerprint: "11".repeat(32),
            version: "binding-version-old-0123456789".to_string(),
            status: hangar_core::AiProviderCredentialBindingStatus::Active,
        };
        db.set_ai_provider_credential_binding(Some(&old_binding))
            .unwrap();
        let error = ai_key_set_with_writer(&state, "new-key-0123456789", |_| {
            Err("injected key-store write failure".to_string())
        })
        .expect_err("failed rotation must restore metadata");
        assert!(error.contains("injected"), "{error}");
        assert_eq!(
            db.ai_provider_credential_binding().unwrap(),
            Some(old_binding)
        );

        let error = ai_key_set_with_writer_and_restore(
            &state,
            "partial-key-0123456789",
            |_| Err("injected partial key write".to_string()),
            |_, _| Err("injected binding rollback failure".to_string()),
        )
        .expect_err("failed key write plus failed rollback must remain pending");
        assert!(error.contains("remain blocked"), "{error}");
        let pending = db.ai_provider_credential_binding().unwrap().unwrap();
        assert_eq!(
            pending.status,
            hangar_core::AiProviderCredentialBindingStatus::Pending
        );
        assert!(!credential_binding_authorizes(
            &pending,
            &pending.origin,
            &pending.fingerprint,
        ));

        db.set_ai_provider_credential_binding(None).unwrap();
        let key = "first-key-0123456789";
        let expected = hangar_ai::key_material_fingerprint(key);
        ai_key_set_with_writer(&state, key, |_| Ok(expected.clone())).unwrap();
        let binding = db.ai_provider_credential_binding().unwrap().unwrap();
        assert_eq!(binding.origin, "https://api-a.example:443");
        assert_eq!(binding.fingerprint, expected);
        assert!(binding.version.len() >= 24);

        let error =
            ai_key_set_with_writer(&state, "rotated-key-0123456789", |_| Ok("ff".repeat(32)))
                .expect_err("mismatched key-store readback must revoke binding");
        assert!(error.contains("did not retain"), "{error}");
        assert!(db.ai_provider_credential_binding().unwrap().is_none());
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn provider_test_and_models_use_typed_consuming_preview_capabilities() {
        let state = AppState::memory().unwrap();

        let test_disclosure = ai_provider_test_disclosure(
            &state,
            "local",
            "http://localhost:11434/v1",
            "draft-model",
            "chat_completions",
        )
        .unwrap();
        assert_eq!(test_disclosure.method, "POST");
        assert_eq!(
            test_disclosure.url,
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert!(test_disclosure.request_body.contains("\"ping\""));
        assert!(test_disclosure.send_chars > 0);
        let reviewed = take_ai_prepared_send(
            &state,
            &test_disclosure.preview_id,
            AiPreparedKind::ProviderTest,
        )
        .unwrap();
        assert_eq!(
            reviewed.disclosure().request_body,
            test_disclosure.request_body
        );
        let replay = take_ai_prepared_send(
            &state,
            &test_disclosure.preview_id,
            AiPreparedKind::ProviderTest,
        )
        .expect_err("a consumed provider-test preview must not replay");
        assert!(
            replay.contains("missing, expired or was already used"),
            "{replay}"
        );

        let wrong_purpose = ai_provider_test_disclosure(
            &state,
            "local",
            "http://127.0.0.1:11434/v1",
            "draft-model",
            "chat_completions",
        )
        .unwrap();
        let error = take_ai_prepared_send(
            &state,
            &wrong_purpose.preview_id,
            AiPreparedKind::ProviderModels,
        )
        .expect_err("a provider-test preview must not authorize model listing");
        assert!(error.contains("different action"), "{error}");
        let consumed = take_ai_prepared_send(
            &state,
            &wrong_purpose.preview_id,
            AiPreparedKind::ProviderTest,
        )
        .expect_err("wrong-purpose validation must consume first");
        assert!(
            consumed.contains("missing, expired or was already used"),
            "{consumed}"
        );

        let models_disclosure = ai_provider_models_disclosure(
            &state,
            "local",
            "http://localhost:1234/v1",
            "draft-model",
            "chat_completions",
        )
        .unwrap();
        assert_eq!(models_disclosure.method, "GET");
        assert_eq!(models_disclosure.url, "http://127.0.0.1:1234/v1/models");
        assert!(models_disclosure.request_body.is_empty());
        assert_eq!(models_disclosure.send_chars, 0);
        assert_eq!(models_disclosure.est_tokens, 0);
        let reviewed = take_ai_prepared_send(
            &state,
            &models_disclosure.preview_id,
            AiPreparedKind::ProviderModels,
        )
        .unwrap();
        assert_eq!(reviewed.disclosure().method, "GET");
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn prepared_send_ttl_expires_via_monotonic_time_seam() {
        let state = AppState::memory().unwrap();
        let disclosure = ai_provider_test_disclosure(
            &state,
            "local",
            "http://127.0.0.1:11434/v1",
            "draft-model",
            "chat_completions",
        )
        .unwrap();
        let created_at = state
            .ai_prepared_sends
            .lock()
            .unwrap()
            .requests
            .get(&disclosure.preview_id)
            .unwrap()
            .created_at;
        let expired_at = created_at + AI_PREPARED_SEND_TTL + Duration::from_millis(1);
        let error = take_ai_prepared_send_pending_at(
            &state,
            &disclosure.preview_id,
            AiPreparedKind::ProviderTest,
            expired_at,
        )
        .expect_err("monotonic TTL must expire the one-shot capability");
        assert!(error.contains("preview expired"), "{error}");
        assert!(!state
            .ai_prepared_sends
            .lock()
            .unwrap()
            .requests
            .contains_key(&disclosure.preview_id));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn credential_mutation_waits_until_linearized_provider_send_finishes() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let payload = r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            request_line
        });

        let state = AppState::memory().unwrap();
        let disclosure = ai_provider_test_disclosure(
            &state,
            "local",
            &format!("http://127.0.0.1:{port}/v1"),
            "test-model",
            "chat_completions",
        )
        .unwrap();
        let send_state = state.clone();
        let preview_id = disclosure.preview_id;
        let send = thread::spawn(move || ai_provider_test(&send_state, &preview_id));
        accepted_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("provider request reached the loopback server");

        let mutation_state = state.clone();
        let (mutation_done_tx, mutation_done_rx) = mpsc::channel();
        let mutation = thread::spawn(move || {
            let result = ai_provider_set(
                &mutation_state,
                "local",
                "http://127.0.0.1:1234/v1",
                "next-model",
                "chat_completions",
            );
            mutation_done_tx.send(result).unwrap();
        });
        assert!(
            mutation_done_rx
                .recv_timeout(Duration::from_millis(150))
                .is_err(),
            "provider mutation crossed the in-flight send linearization point"
        );
        assert_eq!(
            state.db().unwrap().ai_provider_config().unwrap().mode,
            "off"
        );

        release_tx.send(()).unwrap();
        assert_eq!(send.join().unwrap().unwrap(), "Provider responded.");
        assert!(server
            .join()
            .unwrap()
            .starts_with("POST /v1/chat/completions"));
        mutation_done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        mutation.join().unwrap();
        assert_eq!(
            state.db().unwrap().ai_provider_config().unwrap().base_url,
            "http://127.0.0.1:1234/v1"
        );
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn editable_file_resolution_accepts_only_registered_unprotected_nodes() {
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
        assert!(resolve_editable_inventory_target(&state, normal.node_id)
            .unwrap()
            .0
            .starts_with("fixture://markdown-project/"));

        let sensitive = quick_open(&state, ".env".to_string(), Some(20))
            .unwrap()
            .into_iter()
            .find(|item| item.project_id == sensitive_project_id)
            .expect("sensitive fixture file");
        assert!(resolve_editable_inventory_target(&state, sensitive.node_id)
            .unwrap_err()
            .contains("Protected Zone"));
        assert!(resolve_editable_inventory_target(&state, i64::MAX)
            .unwrap_err()
            .contains("not a present item"));
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn editable_disk_target_must_remain_inside_the_registered_project() {
        let project = unique_temp_dir("codehangar-edit-boundary");
        let outside = unique_temp_dir("codehangar-edit-outside");
        let file = project.join("src").join("main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}").unwrap();

        validate_editable_disk_target(
            &file.to_string_lossy(),
            &[project.to_string_lossy().to_string()],
        )
        .unwrap();
        assert!(validate_editable_disk_target(
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
                    source_project_id: Some(granted),
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    evidence: Some(dup_evidence.into()),
                },
                // in-grant duplicate edge: kept, but its count evidence is scrubbed.
                GraphEdge {
                    source_node_id: 10,
                    target_node_id: 11,
                    source_project_id: Some(granted),
                    kind: "duplicate_model_candidate".into(),
                    confidence: "Medium".into(),
                    evidence: Some(dup_evidence.into()),
                },
                // Both endpoints are grant-visible, but this edge was produced by
                // another project's membership. Its provenance is itself scoped
                // data, so the edge must still be removed.
                GraphEdge {
                    source_node_id: 10,
                    target_node_id: 11,
                    source_project_id: Some(ungranted),
                    kind: "markdown_link".into(),
                    confidence: "High".into(),
                    evidence: Some("foreign membership".into()),
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
            total_edges: 3,
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
        assert_eq!(
            map.edges.len(),
            1,
            "cross-project endpoint or provenance edge survived"
        );
        assert_eq!(map.edges[0].source_project_id, Some(granted));
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
                serde_json::json!({
                    "projectId": ungranted,
                    "nodeId": ungranted_node
                }),
            ),
        );
        // An app scoped only to `granted` can never select the ungranted
        // membership, even if it guesses a real node id.
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("not scoped"));
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn relationship_and_orphan_requests_require_project_id() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let project_id = projects[0].id;
        let node_id = project_graph_map(&state, project_id, Some(50))
            .unwrap()
            .nodes[0]
            .node_id;
        let token = "membership-required-token";
        register_test_automation(
            &state,
            "Membership agent",
            token,
            &["read_graph"],
            &[project_id],
        );

        for method in [
            hangar_agent::AgentMethod::NodeRelationships,
            hangar_agent::AgentMethod::NodeOrphanStatus,
        ] {
            let missing = handle_automation_request(
                &state,
                automation_request(
                    Some(token),
                    method,
                    serde_json::json!({ "nodeId": node_id }),
                ),
            );
            assert!(!missing.ok);
            assert!(
                missing.error.unwrap().contains("projectId"),
                "missing membership identity was not rejected clearly"
            );
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn relationship_and_orphan_requests_reject_mismatched_project_and_node_membership() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let project_a = projects[0].id;
        let project_b = projects[1].id;
        let node_b = project_graph_map(&state, project_b, Some(50))
            .unwrap()
            .nodes[0]
            .node_id;
        let token = "membership-mismatch-token";
        register_test_automation(
            &state,
            "Membership mismatch agent",
            token,
            &["read_graph"],
            &[project_a, project_b],
        );

        for method in [
            hangar_agent::AgentMethod::NodeRelationships,
            hangar_agent::AgentMethod::NodeOrphanStatus,
        ] {
            let denied = handle_automation_request(
                &state,
                automation_request(
                    Some(token),
                    method,
                    serde_json::json!({ "projectId": project_a, "nodeId": node_b }),
                ),
            );

            assert!(!denied.ok, "mismatched membership unexpectedly succeeded");
        }
    }

    #[cfg(feature = "agent_automation")]
    #[test]
    fn shared_node_uses_the_explicit_granted_membership() {
        let state = AppState::memory().unwrap();
        let projects = projects_list(&state).unwrap();
        let (project_a, node_id) = projects
            .iter()
            .find_map(|project| {
                project_context_files(&state, project.id)
                    .ok()
                    .and_then(|files| files.first().map(|file| (project.id, file.node_id)))
            })
            .expect("fixture project with a context node");
        let project_b = projects
            .iter()
            .find(|project| project.id != project_a)
            .expect("second fixture project")
            .id;
        state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                conn.execute(
                    "INSERT INTO nav_item(project_id, node_id, path, display_path, display_name,
                                          item_kind, priority, sort_key, is_markdown)
                     VALUES(?1, ?2, 'shared/api-membership.md', 'shared/api-membership.md',
                            'api-membership.md', 'file', 0, 'api-membership.md', 1)",
                    params![project_b, node_id],
                )?;
                Ok(())
            })
            .unwrap();

        let both_token = "shared-membership-both-token";
        register_test_automation(
            &state,
            "Shared membership agent",
            both_token,
            &["read_graph"],
            &[project_a, project_b],
        );
        for (method, project_id) in [
            (hangar_agent::AgentMethod::NodeRelationships, project_a),
            (hangar_agent::AgentMethod::NodeRelationships, project_b),
            (hangar_agent::AgentMethod::NodeOrphanStatus, project_a),
            (hangar_agent::AgentMethod::NodeOrphanStatus, project_b),
        ] {
            let response = handle_automation_request(
                &state,
                automation_request(
                    Some(both_token),
                    method,
                    serde_json::json!({ "projectId": project_id, "nodeId": node_id }),
                ),
            );
            assert!(
                response.ok,
                "explicit shared membership was refused: {:?}",
                response.error
            );
            assert_eq!(response.result.unwrap()["projectId"], project_id);
        }

        let a_only_token = "shared-membership-a-only-token";
        register_test_automation(
            &state,
            "Shared A-only agent",
            a_only_token,
            &["read_graph"],
            &[project_a],
        );
        let denied = handle_automation_request(
            &state,
            automation_request(
                Some(a_only_token),
                hangar_agent::AgentMethod::NodeRelationships,
                serde_json::json!({ "projectId": project_b, "nodeId": node_id }),
            ),
        );
        assert!(!denied.ok, "shared node borrowed an ungranted membership");
        assert!(denied.error.unwrap().contains("not scoped"));
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

        // The separate final-removal recommendation authorization gates filing,
        // but never the primary local Recovery workflow.
        let recommendation_disabled = handle_automation_request(
            &state,
            automation_request(
                Some(token),
                hangar_agent::AgentMethod::RequestPermanentDelete,
                serde_json::json!({ "entryId": 999_999 }),
            ),
        );
        assert!(!recommendation_disabled.ok);
        assert!(recommendation_disabled
            .error
            .unwrap()
            .contains("Permanent removal is off"));
        state.db().unwrap().set_final_remove_enabled(true).unwrap();

        // Once recommendations are authorized, a holding entry that does not
        // exist is still refused at filing — never an opaque numeric target.
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
    fn legacy_single_entry_final_remove_stays_fail_closed() {
        let state = AppState::memory().unwrap();

        // The action-only compatibility surface can never carry the immutable
        // preview/group binding required by final removal. It therefore stays
        // retired even when the owner enables the primary batch capability.
        let token_error = mutation_token_issue(&state, "final_remove".to_string()).unwrap_err();
        assert!(
            token_error.to_lowercase().contains("legacy single-entry")
                && token_error.to_lowercase().contains("preview-bound"),
            "{token_error}"
        );
        let command_error =
            mutation_final_remove_start(&state, 1, "not-a-real-token".to_string()).unwrap_err();
        assert!(
            command_error.to_lowercase().contains("legacy single-entry")
                && command_error.to_lowercase().contains("preview-bound"),
            "{command_error}"
        );

        assert!(set_final_remove_enabled(&state, true, Some("yes".to_string())).is_err());
        set_final_remove_enabled(
            &state,
            true,
            Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT.to_string()),
        )
        .unwrap();
        assert!(mutation_final_remove_enabled(&state));
        assert!(mutation_token_issue(&state, "final_remove".to_string()).is_err());
        assert!(mutation_final_remove_start(&state, 1, "retired".to_string()).is_err());
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn final_remove_disable_reports_off_only_after_the_shared_boundary_exits() {
        let state = AppState::memory().unwrap();
        set_final_remove_enabled(
            &state,
            true,
            Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT.to_string()),
        )
        .unwrap();
        let admitted_boundary = state.inventory_mutation_gate.write().unwrap();
        let disable_state = state.clone();
        let disable =
            std::thread::spawn(move || set_final_remove_enabled(&disable_state, false, None));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !state.final_remove_disable_latch.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "the disable request did not publish its fail-closed latch"
            );
            std::thread::yield_now();
        }
        assert!(
            mutation_final_remove_enabled(&state),
            "the public setting must remain durably ON until the admitted boundary exits"
        );
        assert!(
            !final_remove_runtime_enabled(&state),
            "new work must already be refused while durable OFF is waiting to linearize"
        );

        drop(admitted_boundary);
        disable.join().unwrap().unwrap();
        assert!(!mutation_final_remove_enabled(&state));
        assert!(!final_remove_runtime_enabled(&state));
    }

    /// CI-safe primary-flow journey: verified backup -> move-to-holding -> immutable batch
    /// preview -> scoped confirmation. Permanent removal starts OFF and must be explicitly
    /// enabled, while neither preview nor confirmation touches the held copy.
    #[cfg(feature = "mutation")]
    #[test]
    fn held_project_is_visible_to_primary_final_remove_batch() {
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

        // The legacy unscoped action stays unavailable.
        assert!(mutation_token_issue(&state, "final_remove".to_string()).is_err());

        let disabled =
            mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible).unwrap_err();
        assert!(disabled.contains("Permanent removal is off"));
        assert!(set_final_remove_enabled(&state, true, None).is_err());
        set_final_remove_enabled(
            &state,
            true,
            Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT.to_string()),
        )
        .unwrap();

        // Once explicitly enabled, the primary batch is discoverable and confirmable.
        // This legacy-content backup needs one elevated ArchiveV2 capture later,
        // which is exactly what the batch contract reports.
        let preview = mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible).unwrap();
        let decision = preview
            .objects
            .iter()
            .find(|object| object.entry_id == stored_id)
            .expect("the held object must be present in the primary batch preview");
        assert_eq!(decision.eligibility, "needsArchiveV2");
        assert!(preview.requires_elevation);
        assert!(!preview.eligible_topology_group_ids.is_empty());
        let confirmation = mutation_final_remove_confirm(
            &state,
            preview.preview_id.clone(),
            preview.preview_digest.clone(),
            preview.eligible_topology_group_ids.clone(),
        )
        .unwrap();
        assert_eq!(confirmation.preview_id, preview.preview_id);
        assert_eq!(confirmation.preview_digest, preview.preview_digest);

        // Turning the capability off invalidates the path before helper lookup or
        // token consumption, even if a fresh scoped confirmation was just minted.
        set_final_remove_enabled(&state, false, None).unwrap();
        let disabled_start = mutation_final_remove_batch_start(
            &state,
            FinalRemoveBatchStartRequest {
                preview_id: preview.preview_id.clone(),
                preview_digest: preview.preview_digest.clone(),
                selected_topology_group_ids: preview.eligible_topology_group_ids.clone(),
                confirmation_token: confirmation.token.clone(),
            },
        )
        .unwrap_err();
        assert!(disabled_start.contains("Permanent removal is off"));
        assert!(mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible).is_err());

        // Deterministically pause a successfully admitted worker between the
        // caller-side preflight and its authoritative execution lock. Disabling
        // during that hand-off must win: the worker rechecks the durable flag
        // before helper resolution or confirmation-token consumption.
        set_final_remove_enabled(
            &state,
            true,
            Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT.to_string()),
        )
        .unwrap();
        let worker_handoff = state.final_remove_worker_test_gate.write().unwrap();
        let started = mutation_final_remove_batch_start(
            &state,
            FinalRemoveBatchStartRequest {
                preview_id: preview.preview_id.clone(),
                preview_digest: preview.preview_digest.clone(),
                selected_topology_group_ids: preview.eligible_topology_group_ids.clone(),
                confirmation_token: confirmation.token.clone(),
            },
        )
        .unwrap();
        let handoff_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !state.final_remove_jobs.worker_started(&started.job_id) {
            assert!(
                std::time::Instant::now() < handoff_deadline,
                "the final-removal worker did not reach the deterministic hand-off"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        set_final_remove_enabled(&state, false, None).unwrap();
        drop(worker_handoff);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let worker_error = loop {
            match mutation_final_remove_batch_status(&state, started.job_id.clone()) {
                Err(error) => break error,
                Ok(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the paused final-removal worker did not publish a terminal refusal"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        };
        assert!(worker_error.contains("Permanent removal is off"));
        let confirmation_binding = state
            .db()
            .unwrap()
            .with_recovery_writer(|conn| {
                hangar_mutation::final_remove_confirmation_binding(
                    conn,
                    &preview.preview_id,
                    &preview.preview_digest,
                    preview.eligible_topology_group_ids.clone(),
                )
                .map_err(|error| DbError::FileRead(error.to_string()))
            })
            .unwrap();
        assert!(
            state.mutation_tokens.consume_scoped(
                &confirmation.token,
                hangar_mutation::ConfirmAction::PermanentDelete,
                &confirmation_binding,
            ),
            "the worker-side OFF recheck must happen before token consumption"
        );

        // The retired unscoped command remains unavailable regardless of the flag.
        assert!(mutation_token_issue(&state, "final_remove".to_string()).is_err());
        assert!(mutation_final_remove_start(&state, stored_id, "retired".to_string()).is_err());
        let after = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(after
            .stored_entries
            .iter()
            .any(|entry| entry.id == stored_id && entry.status == "quarantined"));
        assert!(
            Path::new(&backup.manifest_path).exists(),
            "preview/confirmation must leave the verified backup untouched"
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

    // A connected app may request final removal, but approval cannot turn an
    // unscoped holding-entry identifier into an irreversible action. The owner
    // must review the immutable local batch in Recovery & cleanup.
    #[cfg(all(feature = "mutation", feature = "agent_automation"))]
    #[test]
    fn connected_app_single_entry_final_remove_requires_local_batch_review() {
        let state = AppState::memory().unwrap();

        // A live agent with the execute_plan scope final_remove requires (no project, so the
        // non-cross-scope project check is skipped).
        let agent = register_test_automation(
            &state,
            "claude-code",
            "tok-final-remove",
            &["execute_plan"],
            &[],
        );

        // The entry need not exist: authorization is refused before any mutation.
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

        let err =
            agent_request_resolve(&state, request.id, true, ResolveInputs::default()).unwrap_err();
        assert!(
            err.contains("immutable project/batch preview"),
            "expected immutable-batch refusal, got: {err}"
        );
        assert!(
            err.contains("Recovery & cleanup"),
            "the refusal must direct the owner to the local batch review: {err}"
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
        assert!(npm_test
            .risk_disclosure
            .contains("not isolated from arbitrary sockets"));

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
        assert!(run
            .limits_summary
            .contains("project code is not network-sandboxed"));

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

    #[cfg(all(feature = "mutation", windows))]
    #[test]
    fn reparse_item_blocks_preview_and_backup_before_any_file_is_touched() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-protected-mutation");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        let sensitive = project_dir.join(".env");
        let reparse = project_dir.join("linked-outside");
        let reparse_target = temp_root.join("outside-target");
        std::fs::write(&source, "local mutation fixture").unwrap();
        std::fs::write(&sensitive, "TOKEN=local-only").unwrap();
        std::fs::create_dir_all(&reparse_target).unwrap();
        std::fs::write(reparse_target.join("must-survive.txt"), "outside").unwrap();
        if !create_test_directory_link(&reparse, &reparse_target) {
            let _ = std::fs::remove_dir_all(temp_root);
            return;
        }
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
        let preview_error = mutation_preview_protected(&state, plan.clone()).unwrap_err();
        assert!(
            preview_error.contains("reversible link journaling"),
            "{preview_error}"
        );

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup_error = mutation_backup_start(
            &state,
            plan,
            backup_dir.to_string_lossy().to_string(),
            "standard".to_string(),
            Some(true),
            true,
            token,
        )
        .unwrap_err();
        assert!(
            backup_error.contains("reversible link journaling"),
            "{backup_error}"
        );
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "local mutation fixture"
        );
        assert_eq!(
            std::fs::read_to_string(&sensitive).unwrap(),
            "TOKEN=local-only"
        );
        assert!(!backup_dir.exists());
        assert!(reparse.exists());
        assert!(reparse_target.join("must-survive.txt").exists());

        std::fs::remove_dir(&reparse).unwrap();
        let _ = std::fs::remove_dir_all(temp_root);
    }

    /// Gate 3 adversarial QA on real temporary files: move-without-backup is
    /// refused; backup -> move succeeds; the primary final-remove surface then
    /// produces an immutable preview and scoped confirmation while preserving
    /// both held copy and verified backup until the supervised batch actually runs.
    #[cfg(feature = "mutation")]
    #[test]
    fn gate3_primary_final_remove_preview_on_real_files() {
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

        // (C) The legacy unscoped command remains retired. The primary immutable
        // path starts OFF, rejects an unacknowledged enable, then becomes available
        // only after the exact owner activation phrase.
        assert!(mutation_token_issue(&state, "final_remove".to_string()).is_err());
        let disabled =
            mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible).unwrap_err();
        assert!(disabled.contains("Permanent removal is off"));
        assert!(set_final_remove_enabled(&state, true, Some("enable".to_string())).is_err());
        set_final_remove_enabled(
            &state,
            true,
            Some(FINAL_REMOVE_ENABLE_ACKNOWLEDGEMENT.to_string()),
        )
        .unwrap();
        let preview = mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible).unwrap();
        assert!(preview
            .objects
            .iter()
            .any(|item| item.entry_id == stored_id));
        let confirmation = mutation_final_remove_confirm(
            &state,
            preview.preview_id.clone(),
            preview.preview_digest.clone(),
            preview.eligible_topology_group_ids.clone(),
        )
        .unwrap();
        assert_eq!(confirmation.preview_digest, preview.preview_digest);

        // (D) Review/confirmation is non-mutating: the held copy and verified
        // backup both remain until the supervised batch is started.
        let after = mutation_activity_log(&state, Some(20)).unwrap();
        assert!(after
            .stored_entries
            .iter()
            .any(|entry| entry.id == stored_id && entry.status == "quarantined"));
        assert!(
            Path::new(&backup.manifest_path).exists(),
            "the verified backup must survive preview and confirmation"
        );

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
        assert!(
            error.contains("file identity") && error.contains("changed"),
            "{error}"
        );
        assert!(source.exists());

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn move_uses_backup_hash_and_stamp_through_the_executor_handle() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-api-bound-move");
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
            "Bound backup-to-move proof".to_string(),
            None,
        )
        .unwrap();

        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let backup = mutation_backup_start(
            &state,
            plan.clone(),
            backup_dir.to_string_lossy().into_owned(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap();

        // Preserve volume, file id, byte length and mtime, but change content
        // after the verified backup. Restoring the exact reviewed timestamp makes
        // this deterministic across a whole-second boundary and forces the
        // executor to reject the changed bytes through its already-bound handle.
        let reviewed_modified = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, "LOCAL mutation fixture").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(reviewed_modified))
            .unwrap();
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let error = mutation_move_start(
            &state,
            plan,
            holding_dir.to_string_lossy().into_owned(),
            backup.backup_id,
            false,
            token,
        )
        .unwrap_err();

        assert!(error.contains("Move is incomplete"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "LOCAL mutation fixture"
        );
        assert!(!holding_dir.exists());
        let entries = mutation_activity_log(&state, Some(20))
            .unwrap()
            .stored_entries;
        assert!(
            entries.is_empty(),
            "no changed source may become a held entry"
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn mutation_revalidation_rejects_a_tampered_wire_target() {
        let state = AppState::memory().unwrap();
        let temp_root = unique_temp_dir("codehangar-api-plan-envelope");
        let project_dir = temp_root.join("project");
        let backup_dir = temp_root.join("backup");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = project_dir.join("artifact.txt");
        std::fs::write(&source, "local mutation fixture").unwrap();

        insert_mutation_fixture_project(&state, &project_dir, &source);
        let mut plan = operation_plan_build(
            &state,
            90_000,
            "Wire envelope tamper check".to_string(),
            None,
        )
        .unwrap();
        // Keep the authentic fingerprint but redirect the untrusted envelope. The
        // backend must compare against its rebuilt plan before creating a backup or
        // accepting any wire path as mutation authority.
        plan.target.path = temp_root
            .join("forged-target")
            .to_string_lossy()
            .into_owned();
        let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
            .unwrap()
            .token;
        let error = mutation_backup_start(
            &state,
            plan,
            backup_dir.to_string_lossy().into_owned(),
            "standard".to_string(),
            Some(true),
            false,
            token,
        )
        .unwrap_err();

        assert!(error.contains("envelope was altered"), "{error}");
        assert!(
            source.exists(),
            "wire-plan tampering must not touch source bytes"
        );
        assert!(
            !backup_dir.exists(),
            "wire-plan tampering must be rejected before backup creation"
        );
        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn both_cloud_states_block_every_project_mutation_entrypoint_before_source_io() {
        for (kind, is_reparse) in [("cloud_local", true), ("cloud_placeholder", false)] {
            let state = AppState::memory().unwrap();
            let temp_root = unique_temp_dir(&format!("codehangar-{kind}-mutation-block"));
            let project_dir = temp_root.join("project");
            let bootstrap_backup_dir = temp_root.join("bootstrap-backup");
            let rejected_backup_dir = temp_root.join("rejected-backup");
            let holding_dir = temp_root.join("holding");
            std::fs::create_dir_all(&project_dir).unwrap();
            let source = project_dir.join("artifact.txt");
            let original = b"fully local fixture bytes with provider-backed catalog identity";
            std::fs::write(&source, original).unwrap();
            insert_mutation_fixture_project(&state, &project_dir, &source);

            // Create one valid backup while the fixture is still an ordinary local file. That
            // lets the real holding entrypoint get past its prerequisite and reach the shared
            // project-admission gate after the catalog changes to either Cloud Files state.
            let ordinary_plan =
                operation_plan_build(&state, 90_000, "SAFE-08 bootstrap backup".to_string(), None)
                    .unwrap();
            let calibration_trap = PlanSourceInspectionTrap::arm();
            let calibration_error =
                mutation_preview_protected(&state, ordinary_plan.clone()).unwrap_err();
            assert!(
                calibration_error.contains("SAFE-08 detected a source I/O boundary"),
                "the test trap must observe the ordinary production source-open path: {calibration_error}"
            );
            assert_eq!(calibration_trap.attempts(), 1);
            drop(calibration_trap);
            let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
                .unwrap()
                .token;
            let bootstrap_backup = mutation_backup_start(
                &state,
                ordinary_plan,
                bootstrap_backup_dir.to_string_lossy().into_owned(),
                "standard".to_string(),
                Some(true),
                false,
                token,
            )
            .unwrap();
            assert!(bootstrap_backup.verified);

            state
                .db()
                .unwrap()
                .with_recovery_writer(|conn| {
                    conn.execute(
                        "UPDATE node
                         SET is_reparse = ?1, reparse_kind = ?2
                         WHERE id = 90001",
                        params![i64::from(is_reparse), kind],
                    )?;
                    Ok(())
                })
                .unwrap();
            let cloud_plan =
                operation_plan_build(&state, 90_000, format!("SAFE-08 {kind} refusal"), None)
                    .unwrap();
            let activity_before = mutation_activity_log(&state, Some(100)).unwrap();
            let inspection_trap = PlanSourceInspectionTrap::arm();

            let preview_error = mutation_preview_protected(&state, cloud_plan.clone()).unwrap_err();
            assert!(preview_error.contains("cloud-backed"), "{preview_error}");

            let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
                .unwrap()
                .token;
            let backup_error = mutation_backup_start(
                &state,
                cloud_plan.clone(),
                rejected_backup_dir.to_string_lossy().into_owned(),
                "standard".to_string(),
                Some(true),
                true,
                token,
            )
            .unwrap_err();
            assert!(backup_error.contains("cloud-backed"), "{backup_error}");

            let token = mutation_token_issue(&state, "enter_mutation_mode".to_string())
                .unwrap()
                .token;
            let move_error = mutation_move_start(
                &state,
                cloud_plan,
                holding_dir.to_string_lossy().into_owned(),
                bootstrap_backup.backup_id,
                true,
                token,
            )
            .unwrap_err();
            assert!(move_error.contains("cloud-backed"), "{move_error}");

            assert_eq!(
                inspection_trap.attempts(),
                0,
                "{kind} reached the no-follow/no-recall source-open primitive"
            );
            let activity_after = mutation_activity_log(&state, Some(100)).unwrap();
            assert_eq!(
                activity_after.operations.len(),
                activity_before.operations.len()
            );
            assert_eq!(activity_after.items.len(), activity_before.items.len());
            assert_eq!(activity_after.backups.len(), activity_before.backups.len());
            assert_eq!(
                activity_after.stored_entries.len(),
                activity_before.stored_entries.len()
            );
            assert_eq!(std::fs::read(&source).unwrap(), original);
            assert!(!rejected_backup_dir.exists());
            assert!(!holding_dir.exists());
            drop(inspection_trap);

            let _ = std::fs::remove_dir_all(temp_root);
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{nonce}"))
    }

    #[cfg(all(feature = "mutation", windows))]
    fn create_test_directory_link(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(all(feature = "mutation", unix))]
    fn create_test_directory_link(link: &Path, target: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(all(feature = "mutation", not(any(windows, unix))))]
    fn create_test_directory_link(_link: &Path, _target: &Path) -> bool {
        false
    }

    #[cfg(feature = "mutation")]
    fn insert_mutation_fixture_project(state: &AppState, project_dir: &Path, source: &Path) {
        let now = "2026-01-01T00:00:00Z";
        let source_path = source.to_string_lossy();
        let project_path = project_dir.to_string_lossy();
        let identity = hangar_fs::inspect_path_identity(source);
        // Mutation revalidation binds the reviewed node timestamp to the same
        // no-follow handle primitive used at execution time. Keep this fixture
        // truthful rather than weakening that production proof for synthetic
        // projects.
        let (source_stamp, _) = hangar_mutation::inspect_local_mutation_file(source)
            .expect("mutation fixture source must have a local handle proof");
        let source_mtime = source_stamp
            .modified_unix_seconds
            .expect("mutation fixture source must have a modification-time proof");
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
                                      size_apparent, size_allocated, mtime, first_seen_at, last_seen_at, present)
                      VALUES(90001, 'file', ?1, 'artifact.txt', ?2, ?3,
                             1, ?4, ?4, ?5, ?6, ?6, 1)",
                    params![
                        source_path.as_ref(),
                        identity.volume_id.as_deref(),
                        identity.inode_key.as_deref(),
                        identity.size_apparent.unwrap_or(0) as i64,
                        source_mtime.to_string(),
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
                // Mutation plans fail closed unless both relationship families were
                // built with the current schema. This synthetic project has no
                // relationship evidence, so record the completed empty index exactly
                // as a real scan would before exercising Gate 3.
                for family in ["markdown", "workflow"] {
                    conn.execute(
                        "INSERT INTO relationship_index_state(
                           project_id, family, schema_version, state, built_at, error
                         ) VALUES(90000, ?1, 2, 'ready', ?2, NULL)
                         ON CONFLICT(project_id, family) DO UPDATE SET
                           schema_version = excluded.schema_version,
                           state = excluded.state,
                           built_at = excluded.built_at,
                           error = NULL",
                        params![family, now],
                    )?;
                }
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
    fn resident_refresh_admits_only_one_root_at_a_time() {
        let state = AppState::memory().unwrap();
        let first = unique_temp_dir("codehangar-resident-first");
        let second = unique_temp_dir("codehangar-resident-second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("README.md"), "# First").unwrap();
        std::fs::write(second.join("README.md"), "# Second").unwrap();
        roots_add(&state, first.to_string_lossy().into_owned()).unwrap();
        roots_add(&state, second.to_string_lossy().into_owned()).unwrap();

        let job_id = background_refresh_resident(&state, true)
            .unwrap()
            .expect("one eligible root should start");
        let status = scan_status(&state, job_id.clone()).unwrap();
        assert_eq!(status.root_ids.len(), 1);
        assert_eq!(status.worker_count, Some(1));
        if matches!(status.state.as_str(), "queued" | "running" | "cancelling") {
            scan_cancel(&state, job_id).unwrap();
        }

        let _ = std::fs::remove_dir_all(first);
        let _ = std::fs::remove_dir_all(second);
    }

    #[test]
    fn resident_quick_probe_never_promotes_drift_to_a_root_scan() {
        let state = AppState::memory().unwrap();
        let root_dir = unique_temp_dir("codehangar-resident-probe-only");
        std::fs::create_dir_all(&root_dir).unwrap();
        std::fs::write(root_dir.join("README.md"), "# Changed while resident").unwrap();
        roots_add(&state, root_dir.to_string_lossy().into_owned()).unwrap();

        assert_eq!(background_refresh_resident(&state, false).unwrap(), None);
        assert!(!state.jobs.has_any_running_job());

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
    fn shell_open_rejects_urls_before_filesystem_resolution() {
        let state = AppState::memory().unwrap();
        let error =
            inspect_open_target(&state, "codehangar://project/readme.md".to_string()).unwrap_err();
        assert!(error.contains("local paths, not URLs"));

        let unc_error =
            inspect_open_target(&state, r"\\server\share\project\README.md".to_string())
                .unwrap_err();
        assert!(unc_error.contains("not UNC or network paths"));
    }

    #[test]
    fn shell_open_automatic_finds_the_nearest_root_and_resolves_the_file() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("package.json"), "{}").unwrap();
        let nested = workspace.path().join("packages").join("guide-app");
        let docs = nested.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"guide-app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let guide = docs.join("guide.md");
        std::fs::write(&guide, "# Guide\n").unwrap();

        let database = tempfile::tempdir().unwrap();
        let state = AppState::open(database.path().join("shell-open.sqlite3")).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = startup_status(&state);
            if status.state == "ready" {
                break;
            }
            assert_ne!(status.state, "failed", "{}", status.message);
            assert!(
                std::time::Instant::now() < deadline,
                "file-backed shell-open state did not become ready within 10 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let canonical_guide = guide.canonicalize().unwrap();
        let discovery_guide =
            PathBuf::from(display_path_for_path(&canonical_guide.to_string_lossy()));
        let nearest = hangar_discovery::nearest_project_root_for_path(&discovery_guide)
            .expect("automatic discovery should find the nested project marker");
        // GitHub's Windows runner can spell the same temp directory through
        // either the long user profile or its 8.3 alias. Compare the resolved
        // filesystem identity, not those equivalent display spellings.
        assert_eq!(
            nearest.canonicalize().unwrap(),
            nested.canonicalize().unwrap()
        );
        let inspection = inspect_open_target(&state, guide.to_string_lossy().into_owned()).unwrap();
        assert_eq!(inspection.known_project_root, None);
        // Unknown files route directly to Viewer, so inspection deliberately
        // avoids ancestor marker probes. An explicit Automatic preparation
        // below still resolves the nearest real project root.
        assert_eq!(
            PathBuf::from(inspection.automatic_project_root)
                .canonicalize()
                .unwrap(),
            docs.canonicalize().unwrap()
        );
        let prepared = prepare_open_target(
            &state,
            guide.to_string_lossy().into_owned(),
            "automatic".to_string(),
            None,
            Some("balanced".to_string()),
        )
        .unwrap();

        assert_eq!(prepared.target_kind, "file");
        assert_eq!(prepared.open_mode, "automatic");
        assert!(!prepared.temporary);
        assert_eq!(
            PathBuf::from(&prepared.project_root)
                .canonicalize()
                .unwrap(),
            nested.canonicalize().unwrap()
        );
        assert_eq!(prepared.scan_job_id, None);
        assert!(!prepared.scan_already_running);
        // The requested Markdown is readable before a scan even exists. The
        // project scan below remains useful for navigation/correlation, but is
        // deliberately not on the file-open critical path.
        let immediate = open_target_preview(
            &state,
            prepared.project_id,
            prepared.input_path.clone(),
            PreviewMode::Rendered,
            None,
        )
        .unwrap();
        assert_eq!(immediate.state, hangar_core::PreviewState::Ready);
        assert!(immediate
            .rendered_html
            .as_deref()
            .is_some_and(|html| html.contains("Guide")));
        let started =
            start_open_target_scan(&state, prepared.root_id, Some("balanced".to_string()))
                .expect("post-preview shell-open starts a scan");
        assert!(started.started_here);
        let job_id = started.job_id;
        let terminal = (0..200)
            .find_map(|_| {
                let status = scan_status(&state, job_id.clone()).unwrap();
                if ["completed", "partial", "failed", "cancelled"].contains(&status.state.as_str())
                {
                    Some(status)
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    None
                }
            })
            .expect("shell-open scan did not finish");
        assert_eq!(terminal.state, "completed", "{}", terminal.message);
        assert!(
            resolve_open_target(&state, prepared.project_id, prepared.input_path)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn unknown_markdown_preview_precedes_scan_creation_and_stays_bounded() {
        let workspace = tempfile::tempdir().unwrap();
        let markdown = workspace.path().join("quick-note.md");
        std::fs::write(
            &markdown,
            "# Immediate note\n\nRead me before cataloguing.\n",
        )
        .unwrap();
        let state = AppState::memory().unwrap();

        let started = Instant::now();
        let cold = open_local_file_preview(
            markdown.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            None,
        )
        .expect("DB-independent local preview")
        .expect("file preview response");
        assert_eq!(cold.preview.state, hangar_core::PreviewState::Ready);
        assert_eq!(cold.preview.project_id, -1);
        let inspection = inspect_open_target(&state, markdown.to_string_lossy().into_owned())
            .expect("inspect unknown markdown");
        let prepared = prepare_open_target(
            &state,
            markdown.to_string_lossy().into_owned(),
            "viewer".to_string(),
            None,
            Some("background".to_string()),
        )
        .expect("prepare viewer without scan");
        assert_eq!(inspection.known_project_root, None);
        assert_eq!(prepared.scan_job_id, None);
        assert!(!state.jobs.has_any_running_job());

        let preview = open_target_preview(
            &state,
            prepared.project_id,
            prepared.input_path.clone(),
            PreviewMode::Rendered,
            None,
        )
        .expect("direct preview");
        assert_eq!(preview.state, hangar_core::PreviewState::Ready);
        assert!(preview
            .rendered_html
            .as_deref()
            .is_some_and(|html| html.contains("Immediate note")));
        assert!(!state.jobs.has_any_running_job());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "small local Markdown critical path unexpectedly took {:?}",
            started.elapsed()
        );

        let started =
            start_open_target_scan(&state, prepared.root_id, Some("background".to_string()))
                .expect("scan starts only after preview");
        assert!(started.started_here);
        scan_cancel(&state, started.job_id).unwrap();
    }

    #[test]
    fn direct_viewer_preview_keeps_sensitive_ancestors_in_its_policy_path() {
        let workspace = tempfile::tempdir().unwrap();
        let sensitive_dir = workspace.path().join(".ssh");
        std::fs::create_dir_all(&sensitive_dir).unwrap();
        let markdown = sensitive_dir.join("innocent-name.md");
        std::fs::write(&markdown, "# Must stay protected\n").unwrap();

        let direct = open_local_file_preview(
            markdown.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            Some(PreviewPolicy {
                allow_sensitive_reveal: true,
                relax_non_strong_protected_preview: true,
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(direct.preview.state, hangar_core::PreviewState::Blocked);
        assert!(direct.preview.source.is_none());
        assert!(direct.preview.rendered_html.is_none());
    }

    #[cfg(windows)]
    fn create_shell_preview_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
        use std::process::{Command, Stdio};

        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return Ok(());
        }
        let status = Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "mklink /J failed with {status}"
            )))
        }
    }

    #[cfg(windows)]
    fn create_shell_preview_file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(unix)]
    fn create_shell_preview_file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn project_preview_and_resolve_do_not_follow_a_final_file_symlink() {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("private.md");
        let linked = workspace.path().join("apparently-local.md");
        std::fs::write(&target, "# Must not be reached through a link\n").unwrap();
        if create_shell_preview_file_link(&target, &linked).is_err() {
            // Windows without Developer Mode cannot create an unprivileged file
            // symlink. Junction coverage below still exercises the mandatory
            // Windows reparse gate; Unix always executes this branch.
            return;
        }
        let state = AppState::memory().unwrap();

        let preview_error = open_target_preview(
            &state,
            1,
            linked.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            None,
        )
        .unwrap_err();
        assert!(preview_error.contains("regular local file"));
        assert_eq!(
            resolve_open_target(&state, 1, linked.to_string_lossy().into_owned()).unwrap(),
            None
        );

        std::fs::remove_file(&linked).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "# Must not be reached through a link\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn direct_viewer_preview_blocks_regular_file_below_a_junction_ancestor() {
        let workspace = tempfile::tempdir().unwrap();
        let protected = workspace.path().join(".ssh");
        let apparently_safe = workspace.path().join("safe");
        let linked = apparently_safe.join("docs");
        std::fs::create_dir_all(&protected).unwrap();
        std::fs::create_dir_all(&apparently_safe).unwrap();
        let secret = protected.join("innocent-name.md");
        std::fs::write(&secret, "# Must never cross the junction\n").unwrap();
        create_shell_preview_directory_link(&protected, &linked).unwrap();

        let aliased_file = linked.join("innocent-name.md");
        let direct = open_local_file_preview(
            aliased_file.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            Some(PreviewPolicy {
                allow_sensitive_reveal: true,
                relax_non_strong_protected_preview: true,
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(direct.preview.state, hangar_core::PreviewState::Blocked);
        assert!(direct.preview.source.is_none());
        assert!(direct.preview.rendered_html.is_none());
        assert!(direct
            .preview
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("parent folder") && reason.contains("junction")));
        assert!(!direct
            .preview
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("online-only")));
        let attach_error = canonical_shell_path(&aliased_file.to_string_lossy()).unwrap_err();
        assert!(attach_error.contains("linked, junction or cloud-only parent"));

        // Remove only the link and prove the preview neither followed nor
        // modified its protected target.
        std::fs::remove_dir(&linked).unwrap();
        assert_eq!(
            std::fs::read_to_string(secret).unwrap(),
            "# Must never cross the junction\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn root_registration_and_investigation_reject_linked_directories() {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join(".ssh");
        let linked = workspace.path().join("project-link");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("secret.md"), "# not inventory input\n").unwrap();
        create_shell_preview_directory_link(&target, &linked).unwrap();
        let state = AppState::memory().unwrap();

        let manual_error = prepare_open_target(
            &state,
            target.join("secret.md").to_string_lossy().into_owned(),
            "manual".to_string(),
            Some(linked.to_string_lossy().into_owned()),
            None,
        )
        .unwrap_err();
        assert!(manual_error.contains("Cannot safely resolve the Manual project root"));

        let register_error = roots_add(&state, linked.to_string_lossy().into_owned()).unwrap_err();
        assert!(register_error.contains("Cannot safely register scan root"));
        let investigate_error = investigate_folder(
            &state,
            linked.to_string_lossy().into_owned(),
            Some("background".to_string()),
        )
        .unwrap_err();
        assert!(investigate_error.contains("Cannot safely register scan root"));
        let deep_scan_error = project_discovery_deep_scan(
            &state,
            linked.to_string_lossy().into_owned(),
            Some(10),
            Some(10),
            Some(false),
            Some(false),
            Some(false),
        )
        .unwrap_err();
        assert!(deep_scan_error.contains("Cannot safely open the Deep Scan root"));
        assert!(state.db().unwrap().roots_list().unwrap().is_empty());
        assert!(!state.jobs.has_any_running_job());

        std::fs::remove_dir(&linked).unwrap();
        assert!(target.join("secret.md").is_file());
    }

    #[test]
    fn direct_viewer_preview_caps_first_frame_before_full_preview() {
        let workspace = tempfile::tempdir().unwrap();
        let markdown = workspace.path().join("large-first-frame.md");
        let first_frame_limit = hangar_db::FIRST_FRAME_PREVIEW_LIMIT_BYTES as usize;
        let mut body = String::with_capacity(first_frame_limit + 4 * 1024);
        body.push_str("# First frame heading\n\n");
        body.push_str(&"bounded preview text ".repeat((first_frame_limit / 21) + 64));
        body.push_str("\n\n## Full preview tail marker\n");
        std::fs::write(&markdown, body).unwrap();

        let direct = open_local_file_preview(
            markdown.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            direct.preview.preview_limit_bytes,
            hangar_db::FIRST_FRAME_PREVIEW_LIMIT_BYTES
        );
        assert!(direct.preview.truncated);
        let first_html = direct.preview.rendered_html.as_deref().unwrap();
        assert!(first_html.contains("First frame heading"));
        assert!(!first_html.contains("Full preview tail marker"));

        // The second DB-independent command remains the full bounded read. The
        // frontend requests it only after the provisional document has painted,
        // without waiting for SQLCipher or project registration.
        let full = open_local_file_preview_full(
            markdown.to_string_lossy().into_owned(),
            PreviewMode::Rendered,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(full.preview.preview_limit_bytes, 2 * 1024 * 1024);
        assert!(!full.preview.truncated);
        assert!(full
            .preview
            .rendered_html
            .as_deref()
            .is_some_and(|html| html.contains("Full preview tail marker")));
    }

    #[test]
    #[ignore = "manual release size-band timing probe"]
    fn direct_first_frame_preview_reports_size_bands() {
        let workspace = tempfile::tempdir().unwrap();
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        for size in [4 * 1024usize, 256 * 1024, 2 * 1024 * 1024] {
            let markdown = workspace.path().join(format!("preview-{size}.md"));
            let mut body = b"# Size-band preview\n\n".to_vec();
            body.resize(size, b'x');
            std::fs::write(&markdown, body).unwrap();
            let path = markdown.to_string_lossy().into_owned();

            let cold_started = Instant::now();
            let cold = open_local_file_preview(path.clone(), PreviewMode::Rendered, None)
                .unwrap()
                .unwrap();
            let cold_micros = cold_started.elapsed().as_micros();
            assert_eq!(
                cold.preview.preview_limit_bytes,
                hangar_db::FIRST_FRAME_PREVIEW_LIMIT_BYTES
            );
            assert_eq!(
                cold.preview.truncated,
                size > hangar_db::FIRST_FRAME_PREVIEW_LIMIT_BYTES as usize
            );

            let mut samples = Vec::with_capacity(20);
            for _ in 0..20 {
                let started = Instant::now();
                let preview = open_local_file_preview(path.clone(), PreviewMode::Rendered, None)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    preview.preview.preview_limit_bytes,
                    hangar_db::FIRST_FRAME_PREVIEW_LIMIT_BYTES
                );
                samples.push(started.elapsed().as_micros());
            }
            samples.sort_unstable();
            let p50 = samples[samples.len() / 2];
            let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];

            let full_cold_started = Instant::now();
            let full_cold = open_local_file_preview_full(path.clone(), PreviewMode::Rendered, None)
                .unwrap()
                .unwrap();
            let full_cold_micros = full_cold_started.elapsed().as_micros();
            assert_eq!(full_cold.preview.preview_limit_bytes, 2 * 1024 * 1024);
            let mut full_samples = Vec::with_capacity(20);
            for _ in 0..20 {
                let started = Instant::now();
                let preview =
                    open_local_file_preview_full(path.clone(), PreviewMode::Rendered, None)
                        .unwrap()
                        .unwrap();
                assert_eq!(preview.preview.preview_limit_bytes, 2 * 1024 * 1024);
                full_samples.push(started.elapsed().as_micros());
            }
            full_samples.sort_unstable();
            let full_p50 = full_samples[full_samples.len() / 2];
            let full_p95 =
                full_samples[(full_samples.len() * 95 / 100).min(full_samples.len() - 1)];
            println!(
                "direct-preview profile={profile} size_bytes={size} first_cold_us={cold_micros} first_p50_us={p50} first_p95_us={p95} full_cold_us={full_cold_micros} full_p50_us={full_p50} full_p95_us={full_p95}"
            );
        }
    }

    #[test]
    fn shell_open_known_descendant_reuses_its_registered_root_without_a_choice() {
        let workspace = tempfile::tempdir().unwrap();
        let nested = workspace.path().join("packages").join("guide-app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("Cargo.toml"), "[package]\nname='guide-app'\n").unwrap();

        let state = AppState::memory().unwrap();
        roots_add(&state, workspace.path().to_string_lossy().into_owned()).unwrap();
        let inspection =
            inspect_open_target(&state, nested.to_string_lossy().into_owned()).unwrap();
        assert_eq!(inspection.target_kind, "folder");
        assert_eq!(
            PathBuf::from(inspection.known_project_root.unwrap())
                .canonicalize()
                .unwrap(),
            workspace.path().canonicalize().unwrap()
        );

        let prepared = prepare_open_target(
            &state,
            nested.to_string_lossy().into_owned(),
            "known".to_string(),
            None,
            Some("balanced".to_string()),
        )
        .unwrap();
        assert_eq!(prepared.open_mode, "known");
        assert_eq!(
            PathBuf::from(&prepared.project_root)
                .canonicalize()
                .unwrap(),
            workspace.path().canonicalize().unwrap()
        );
        if let Some(job_id) = prepared.scan_job_id {
            scan_cancel(&state, job_id).unwrap();
        }
    }

    #[test]
    fn shell_open_unknown_folder_offers_root_or_temporary_viewer() {
        let workspace = tempfile::tempdir().unwrap();
        let nested = workspace.path().join("notes");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(workspace.path().join("package.json"), "{}").unwrap();

        let state = AppState::memory().unwrap();
        let inspection =
            inspect_open_target(&state, nested.to_string_lossy().into_owned()).unwrap();
        assert_eq!(inspection.known_project_root, None);
        assert_eq!(
            PathBuf::from(inspection.automatic_project_root)
                .canonicalize()
                .unwrap(),
            workspace.path().canonicalize().unwrap()
        );
        assert_eq!(
            PathBuf::from(inspection.viewer_root)
                .canonicalize()
                .unwrap(),
            nested.canonicalize().unwrap()
        );

        let prepared = prepare_open_target(
            &state,
            nested.to_string_lossy().into_owned(),
            "viewer".to_string(),
            None,
            Some("balanced".to_string()),
        )
        .unwrap();
        assert_eq!(prepared.target_kind, "folder");
        assert_eq!(prepared.open_mode, "viewer");
        assert!(prepared.temporary);
        assert_eq!(
            PathBuf::from(&prepared.project_root)
                .canonicalize()
                .unwrap(),
            nested.canonicalize().unwrap()
        );
        if let Some(job_id) = prepared.scan_job_id {
            scan_cancel(&state, job_id.clone()).unwrap();
            let _ = (0..100).find(|_| {
                let terminal = scan_status(&state, job_id.clone())
                    .map(|status| {
                        !["queued", "running", "cancelling"].contains(&status.state.as_str())
                    })
                    .unwrap_or(true);
                if !terminal {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                terminal
            });
        }
        assert!(state
            .db()
            .unwrap()
            .projects_list()
            .unwrap()
            .iter()
            .all(|project| !same_display_path(&project.path, &prepared.project_root)));

        let outside = tempfile::tempdir().unwrap();
        let manual_error = prepare_open_target(
            &state,
            nested.to_string_lossy().into_owned(),
            "manual".to_string(),
            Some(outside.path().to_string_lossy().into_owned()),
            None,
        )
        .unwrap_err();
        assert!(manual_error.contains("must contain"));
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
    #[ignore = "large generated fixture; run with scripts/release-stress-v013.ps1"]
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
        let file = dir.join("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.jsonl");

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
                session_limit: 0,
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
                session_limit: 0,
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

    fn file_change_fixture(project_id: i64) -> FileChangeEvent {
        FileChangeEvent {
            id: 7,
            project_id,
            project_name: "fixture".to_string(),
            project_path: r"C:\work\fixture".to_string(),
            node_id: Some(11),
            path: "README.md".to_string(),
            display_name: "README.md".to_string(),
            change_kind: "modified".to_string(),
            observed_at: "2026-08-13T00:00:00Z".to_string(),
            modified_at: None,
            size_bytes: Some(42),
            apps: vec!["stale".to_string()],
        }
    }

    #[test]
    fn recent_change_correlation_uses_cached_project_apps_by_stable_id() {
        let mut event = file_change_fixture(1);
        // Deliberately give the event a differently formatted path: correlation is
        // against the inventory's stable project id and performs no filesystem work.
        event.project_path = "c:/WORK/FIXTURE".to_string();
        let mut project = project_summary_fixture(r"C:\work\fixture");
        project.apps = vec!["omp".to_string(), "opencode".to_string()];

        apply_cached_project_apps(std::slice::from_mut(&mut event), &[project]);

        assert_eq!(event.apps, vec!["omp", "opencode"]);
    }

    #[test]
    fn recent_change_correlation_falls_back_to_primary_app_and_keeps_unknown_safe() {
        let known = file_change_fixture(1);
        let mut unknown = file_change_fixture(999);
        unknown.apps.clear();
        let mut project = project_summary_fixture(r"C:\work\fixture");
        project.app = Some("claude".to_string());

        let mut events = vec![known, unknown];
        apply_cached_project_apps(&mut events, &[project]);
        assert_eq!(events[0].apps, vec!["claude"]);
        assert!(events[1].apps.is_empty());
    }

    #[test]
    fn recent_change_correlation_with_empty_cache_is_a_safe_noop() {
        let mut event = file_change_fixture(1);
        event.apps.clear();

        apply_cached_project_apps(std::slice::from_mut(&mut event), &[]);

        assert!(event.apps.is_empty());
    }

    #[test]
    fn recent_change_correlation_rejects_reused_id_with_a_different_path() {
        let mut event = file_change_fixture(1);
        event.project_path = r"C:\work\replacement".to_string();
        let mut project = project_summary_fixture(r"C:\work\fixture");
        project.apps = vec!["opencode".to_string()];

        apply_cached_project_apps(std::slice::from_mut(&mut event), &[project]);

        assert!(event.apps.is_empty());
    }

    fn app_states_fixture(app: &str) -> ProjectAppStates {
        std::collections::HashMap::from([(
            "c:/work/fixture".to_string(),
            hangar_discovery::ProjectAppState {
                app: Some(app.to_string()),
                apps: vec![app.to_string()],
                is_current: true,
            },
        )])
    }

    #[test]
    fn project_app_state_cache_reuses_fresh_value_until_ttl() {
        let mut cache = ProjectAppStateCache::default();
        let now = Instant::now();
        let loads = std::cell::Cell::new(0_u32);

        let first = cache.get_or_load(now, PROJECT_APP_STATE_CACHE_TTL, || {
            loads.set(loads.get() + 1);
            app_states_fixture("claude")
        });
        let fresh = cache.get_or_load(
            now + PROJECT_APP_STATE_CACHE_TTL - Duration::from_millis(1),
            PROJECT_APP_STATE_CACHE_TTL,
            || {
                loads.set(loads.get() + 1);
                app_states_fixture("opencode")
            },
        );

        assert_eq!(loads.get(), 1);
        assert_eq!(fresh, first);
    }

    #[test]
    fn project_app_state_cache_reloads_at_ttl_boundary() {
        let mut cache = ProjectAppStateCache::default();
        let now = Instant::now();
        let loads = std::cell::Cell::new(0_u32);
        let _ = cache.get_or_load(now, PROJECT_APP_STATE_CACHE_TTL, || {
            loads.set(loads.get() + 1);
            app_states_fixture("claude")
        });

        let stale = cache.get_or_load(
            now + PROJECT_APP_STATE_CACHE_TTL,
            PROJECT_APP_STATE_CACHE_TTL,
            || {
                loads.set(loads.get() + 1);
                app_states_fixture("opencode")
            },
        );

        assert_eq!(loads.get(), 2);
        assert_eq!(stale["c:/work/fixture"].app.as_deref(), Some("opencode"));
    }

    #[test]
    fn project_app_state_cache_invalidation_forces_reload() {
        let mut cache = ProjectAppStateCache::default();
        let now = Instant::now();
        let loads = std::cell::Cell::new(0_u32);
        let _ = cache.get_or_load(now, PROJECT_APP_STATE_CACHE_TTL, || {
            loads.set(loads.get() + 1);
            app_states_fixture("claude")
        });
        cache.invalidate();

        let reloaded = cache.get_or_load(now, PROJECT_APP_STATE_CACHE_TTL, || {
            loads.set(loads.get() + 1);
            app_states_fixture("omp")
        });

        assert_eq!(loads.get(), 2);
        assert_eq!(reloaded["c:/work/fixture"].app.as_deref(), Some("omp"));
    }

    #[test]
    fn process_project_snapshot_keeps_more_than_disk_startup_cap() {
        let state = AppState::memory().unwrap();
        let projects = (0..205)
            .map(|id| {
                let mut project = project_summary_fixture(&format!(r"C:\work\project-{id}"));
                project.id = id;
                project
            })
            .collect::<Vec<_>>();

        state.write_project_cache_if_generation(&projects, state.project_cache_generation());

        assert_eq!(state.read_project_cache().len(), projects.len());
    }

    #[test]
    fn oversized_project_snapshot_serializes_as_bounded_empty_cold_cache() {
        let mut project = project_summary_fixture(r"C:\work\fixture");
        project.name = "x".repeat(PROJECT_DISK_CACHE_MAX_JSON_BYTES + 1);

        let json = project_disk_cache_json(&[project]).unwrap();

        assert_eq!(json, b"[]");
    }

    #[test]
    fn catalog_mutation_invalidates_project_and_app_state_caches() {
        let state = AppState::memory().unwrap();
        state.write_project_cache_if_generation(
            &[project_summary_fixture(r"C:\work\fixture")],
            state.project_cache_generation(),
        );
        {
            let mut cache = state
                .project_app_state_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = cache.get_or_load(Instant::now(), PROJECT_APP_STATE_CACHE_TTL, || {
                app_states_fixture("claude")
            });
        }
        let root_dir = unique_temp_dir("codehangar-cache-invalidation");
        std::fs::create_dir_all(&root_dir).unwrap();

        roots_add(&state, root_dir.to_string_lossy().into_owned()).unwrap();

        assert!(state.read_project_cache().is_empty());
        let app_cache = state
            .project_app_state_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(app_cache.loaded_at.is_none());
        assert!(app_cache.states.is_empty());
        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn registering_an_existing_root_keeps_fresh_caches() {
        let state = AppState::memory().unwrap();
        let root_dir = unique_temp_dir("codehangar-cache-existing-root");
        std::fs::create_dir_all(&root_dir).unwrap();
        roots_add(&state, root_dir.to_string_lossy().into_owned()).unwrap();
        state.write_project_cache_if_generation(
            &[project_summary_fixture(&root_dir.to_string_lossy())],
            state.project_cache_generation(),
        );
        {
            let mut cache = state
                .project_app_state_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = cache.get_or_load(Instant::now(), PROJECT_APP_STATE_CACHE_TTL, || {
                app_states_fixture("claude")
            });
        }

        roots_add(&state, root_dir.to_string_lossy().into_owned()).unwrap();

        assert_eq!(state.read_project_cache().len(), 1);
        let app_cache = state
            .project_app_state_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(app_cache.loaded_at.is_some());
        assert_eq!(app_cache.states.len(), 1);
        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn local_path_identity_ignores_windows_extended_prefix_case_and_trailing_separator() {
        assert!(same_local_path(r"\\?\C:\Work\Project\", r"c:\work\project"));
        assert!(same_local_path(
            r"\\?\UNC\server\share\Project\",
            r"\\SERVER\SHARE\project"
        ));
    }

    #[test]
    fn invalidation_rejects_an_in_flight_stale_project_snapshot() {
        let state = AppState::memory().unwrap();
        let stale_generation = state.project_cache_generation();
        let stale_projects = vec![project_summary_fixture(r"C:\work\removed")];

        state.invalidate_project_caches();
        state.write_project_cache_if_generation(&stale_projects, stale_generation);

        assert!(state.read_project_cache().is_empty());
    }

    #[test]
    fn enrich_current_state_leaves_unknown_paths_inactive() {
        // A path no registry/activity signal claims must stay is_current = false and
        // app = None — and enrichment must never panic on real-machine data.
        let mut projects = vec![project_summary_fixture(
            r"C:\definitely\not\a\real\registered\project\zzz",
        )];
        let state = AppState::memory().unwrap();
        enrich_current_state(&state, &mut projects);
        assert!(!projects[0].is_current);
        assert_eq!(projects[0].app, None);
    }

    #[test]
    fn project_app_enrichment_carries_opencode_and_omp_to_the_frontend_contract() {
        let path = r"C:\work\shared-project";
        let mut projects = vec![project_summary_fixture(path)];
        let mut states = std::collections::HashMap::new();
        states.insert(
            hangar_discovery::project_path_key(path),
            hangar_discovery::ProjectAppState {
                app: Some("opencode".to_string()),
                apps: vec!["omp".to_string(), "opencode".to_string()],
                is_current: true,
            },
        );

        apply_project_app_states(&mut projects, &states);

        assert_eq!(projects[0].app.as_deref(), Some("opencode"));
        assert_eq!(projects[0].apps, vec!["omp", "opencode"]);
        assert!(projects[0].is_current);
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
                 local AI apps currently track (e.g. C:\\Synthetic\\CodeHangarDemo)"
            );
            return;
        };
        let mut projects = vec![project_summary_fixture(path.trim())];
        let state = AppState::memory().unwrap();
        enrich_current_state(&state, &mut projects);
        assert!(
            projects[0].is_current,
            "a project the AI-app registries reference must be marked current by the API pass"
        );
    }
}
