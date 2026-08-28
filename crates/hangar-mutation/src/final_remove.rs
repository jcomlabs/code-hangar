//! Preview-bound project/batch permanent removal.
//!
//! The activity feed and legacy `backup_manifest/1` rows are never authority.
//! A preview is rebuilt from every selected holding row, persisted with one row
//! per exact object/topology group, and consumed only through a scoped,
//! short-lived confirmation capability. `object_archive/2` is the only proof
//! schema that can make an object immediately purge-ready.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::confirm::{
    secure_random_hex_256, ConfirmAction, ConfirmIssueError, ConfirmTokenStore, ConfirmationBinding,
};
use crate::elevated_transport::{
    invoke_elevated_helper_lazy, LazyElevatedCapabilityBatch, MaterializedCapabilityChunk,
};
#[cfg(windows)]
use crate::elevated_transport::{
    launch_disposition_guardian, DispositionGuardian, DispositionGuardianLaunch,
    GuardianCancelOutcome,
};
use crate::MAX_CAPABILITIES_PER_INVOCATION;
use crate::{
    archive_path_for_capability, current_parent_binding, scratch_leaf_for_capability,
    BoundObjectProof, BoundScratchRoot, CommittedObjectArchive, ElevatedCapability,
    ElevatedItemResult, ElevatedObjectResult, ElevatedRequest, ElevatedResponse, ExpectedObject,
    ExpectedScratchRoot, FileStamp, ObjectArchiveContainer, PROTOCOL_SCHEMA,
};

const PREVIEW_TTL_MINUTES: i64 = 5;
const OBJECT_ARCHIVE_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;
pub const OBJECT_ARCHIVE_PROOF_SCHEMA: &str = "object_archive/2";
pub(crate) const UNPROVED_FINAL_PROFILE_REASON_CODE: &str = "unprovedFinalProfile";
pub(crate) const PROVED_FINAL_PROFILE_REASON_CODE: &str = "finalProfileProvedHeld";
const UNSUPPORTED_FINAL_STREAM_PROFILE_REASON_CODE: &str = "unsupportedObjectStream";
const UNSUPPORTED_FINAL_STREAM_PROFILE_MESSAGE: &str =
    "The held object has a named, non-default, or otherwise unproved NTFS stream. Code Hangar v0.1.3 requires a file to enumerate exactly ::$DATA and a directory to enumerate zero FileStreamInfo entries.";

#[derive(Debug, Error)]
pub enum FinalRemoveError {
    #[error("final removal preview is invalid: {0}")]
    InvalidPreview(String),
    #[error("final removal confirmation is invalid or expired")]
    ConfirmRequired,
    #[error("final removal journal failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("final removal random source failed: {0}")]
    Random(#[from] ConfirmIssueError),
    #[error("final removal io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("final removal helper failed: {0}")]
    Helper(#[from] crate::ElevatedTransportError),
}

/// Exact wire shape consumed by the desktop UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FinalRemoveScope {
    Project { group_id: String },
    Groups { group_ids: Vec<String> },
    AllEligible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBlockedSubtree {
    pub root: String,
    pub count: u64,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveProjectPreview {
    pub group_id: String,
    pub project_name: String,
    pub original_root: String,
    pub total_objects: u64,
    pub ready: u64,
    pub needs_archive_v2: u64,
    pub blocked: u64,
    pub blocked_subtrees: Vec<FinalRemoveBlockedSubtree>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveObjectDecision {
    pub entry_id: i64,
    /// Project/removal group used for UI scope. Distinct from topology.
    pub group_id: String,
    /// Atomic same-object/hardlink identity group when journal identity exists.
    pub topology_group_id: String,
    pub relative_path: String,
    pub kind: String,
    pub lifecycle: String,
    pub eligibility: String,
    pub reason_code: String,
    pub reason: String,
    pub remediation: Option<String>,
    pub archive_id: Option<String>,
    pub object_archive_state: String,
    pub held_volume_id: String,
    pub held_volume_label: String,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
    pub measurement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveVolumeImpact {
    pub volume_id: String,
    pub label: String,
    pub already_freed_from_source_bytes: u64,
    pub held_allocated_bytes: u64,
    pub projected_release_bytes: u64,
    pub archive_retained_allocated_bytes: u64,
    pub free_bytes_before: Option<u64>,
    pub free_bytes_after: Option<u64>,
    pub observed_delta_bytes: Option<i64>,
    pub quality: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemovePreview {
    pub preview_id: String,
    pub preview_digest: String,
    pub expires_at: String,
    pub projects: Vec<FinalRemoveProjectPreview>,
    pub objects: Vec<FinalRemoveObjectDecision>,
    pub volumes: Vec<FinalRemoveVolumeImpact>,
    pub eligible_topology_group_ids: Vec<String>,
    pub requires_elevation: bool,
    pub max_delete_objects: u64,
    pub blocked_objects: u64,
    pub archives_retained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchItemResult {
    pub entry_id: i64,
    pub state: String,
    pub reason_code: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveProjectResult {
    pub group_id: String,
    pub deleted: u64,
    pub kept: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchResult {
    pub batch_id: String,
    pub status: String,
    pub requested_objects: u64,
    pub deleted_objects: u64,
    pub kept_objects: u64,
    pub failed_objects: u64,
    pub projects: Vec<FinalRemoveProjectResult>,
    pub volumes: Vec<FinalRemoveVolumeImpact>,
    pub items: Vec<FinalRemoveBatchItemResult>,
    pub archive_retained: bool,
}

/// Stable execution phases emitted by the permanent-removal engine.  The wire
/// names intentionally match the desktop job wrapper so the wrapper can copy a
/// progress event without inventing a second phase vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalRemoveBatchPhase {
    WaitingForUac,
    VerifyingArchives,
    Roundtrip,
    ParentDisposition,
    Deleting,
    CleaningDirs,
    Finished,
    Interrupted,
}

impl FinalRemoveBatchPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForUac => "waitingForUac",
            Self::VerifyingArchives => "verifyingArchives",
            Self::Roundtrip => "roundtrip",
            Self::ParentDisposition => "parentDisposition",
            Self::Deleting => "deleting",
            Self::CleaningDirs => "cleaningDirs",
            Self::Finished => "finished",
            Self::Interrupted => "interrupted",
        }
    }
}

/// Progress snapshot produced at durable/atomic execution boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalRemoveBatchProgress {
    pub batch_id: String,
    pub phase: FinalRemoveBatchPhase,
    pub total: u64,
    pub completed: u64,
    pub current_path: Option<String>,
}

/// Why execution must stop before the next topology-group disposition.
/// Owner intent is the only cause that may produce a clean `cancelled` batch;
/// internal control-plane failures remain visibly interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalRemoveInterruptionReason {
    OwnerStop,
    ProgressObserverFailed,
}

/// Cooperative stop signal for a running permanent-removal batch.
///
/// The engine consults this signal only immediately before disposing an
/// indivisible topology group. A request that arrives while a group is being
/// disposed therefore takes effect at the next group boundary.
pub trait FinalRemoveBatchControl: Send + Sync {
    fn stop_requested(&self) -> bool;

    /// Classify the interruption without weakening compatibility for existing
    /// boolean controls. A plain closure represents explicit owner intent;
    /// wrappers that also fail closed on internal errors must override this.
    fn interruption_reason(&self) -> Option<FinalRemoveInterruptionReason> {
        self.stop_requested()
            .then_some(FinalRemoveInterruptionReason::OwnerStop)
    }
}

impl<F> FinalRemoveBatchControl for F
where
    F: Fn() -> bool + Send + Sync,
{
    fn stop_requested(&self) -> bool {
        self()
    }
}

/// Observer used by API/job wrappers to mirror phase, progress and current
/// item without sharing mutation-engine internals.
pub trait FinalRemoveBatchObserver: Send {
    fn on_progress(&mut self, progress: FinalRemoveBatchProgress);
}

impl<F> FinalRemoveBatchObserver for F
where
    F: FnMut(FinalRemoveBatchProgress) + Send,
{
    fn on_progress(&mut self, progress: FinalRemoveBatchProgress) {
        self(progress);
    }
}

#[derive(Debug, Clone)]
struct PreviewRow {
    entry_id: i64,
    operation_id: Option<i64>,
    original_path: String,
    held_path: String,
    logical_bytes: u64,
    space_recovered: u64,
    backup_id: Option<i64>,
    backup_verified: bool,
    removal_group_id: Option<String>,
    removal_group_fingerprint: Option<String>,
    operation_target_fingerprint: Option<String>,
    expected_volume_id: Option<String>,
    result_volume_id: Option<String>,
    result_file_id: Option<String>,
    result_bytes: Option<u64>,
    result_modified_unix_seconds: Option<i64>,
    result_blake3: Option<String>,
    archive_id: Option<i64>,
    archive_path: Option<String>,
    archive_volume_id: Option<String>,
    archive_file_id: Option<String>,
    archive_bytes: Option<u64>,
    archive_modified_unix_seconds: Option<i64>,
    archive_blake3: Option<String>,
    raw_backup_blake3: Option<String>,
    stream_count: Option<u32>,
    proof_schema: Option<String>,
    proof_status: Option<String>,
    proof_held_volume_id: Option<String>,
    proof_held_file_id: Option<String>,
    proof_held_bytes: Option<u64>,
    proof_held_modified_unix_seconds: Option<i64>,
    proof_held_blake3: Option<String>,
    proof_semantic: Option<String>,
    proof_roundtrip: Option<String>,
    proof_security: Option<bool>,
    proof_cleanup: Option<bool>,
    proof_verified_at: Option<String>,
}

#[derive(Debug, Clone)]
struct ClassifiedRow {
    row: PreviewRow,
    project_group_id: String,
    topology_group_id: String,
    decision: FinalRemoveObjectDecision,
}

/// Persist a complete, server-side preview. The query is intentionally
/// unpaginated; recent activity is never an authorization source.
pub fn build_final_remove_preview(
    conn: &Connection,
    scope: FinalRemoveScope,
) -> Result<FinalRemovePreview, FinalRemoveError> {
    validate_scope(&scope)?;
    // Topology membership is global across every held row. Filtering first
    // would let a project-scoped preview authorize one pathname of an object
    // whose other hardlink/pathname lives in another removal group.
    let mut all_classified = load_preview_rows(conn)?
        .into_iter()
        .map(classify_row)
        .collect::<Vec<_>>();
    let selected_entry_ids = all_classified
        .iter()
        .filter(|item| scope_selects(&scope, &item.project_group_id))
        .map(|item| item.row.entry_id)
        .collect::<BTreeSet<_>>();
    if selected_entry_ids.is_empty() {
        return Err(FinalRemoveError::InvalidPreview(
            "the requested scope has no currently held objects".to_string(),
        ));
    }
    if selected_entry_ids.len() > u32::MAX as usize {
        return Err(FinalRemoveError::InvalidPreview(
            "preview exceeds the journal's 32-bit object-count representation".to_string(),
        ));
    }
    propagate_topology_blockage(&mut all_classified, &selected_entry_ids);
    let mut classified = all_classified
        .into_iter()
        .filter(|item| selected_entry_ids.contains(&item.row.entry_id))
        .collect::<Vec<_>>();
    block_unsupported_multi_path_topologies(&mut classified);
    apply_capacity_block(&mut classified);
    propagate_selected_group_blockage(&mut classified);
    apply_project_relative_paths(&mut classified);

    let eligible_topology_group_ids = wholly_eligible_topology_groups(&classified);
    let projects = project_previews(&classified);
    let volumes = volume_impacts(&classified, &eligible_topology_group_ids);
    let objects = classified
        .iter()
        .map(|item| item.decision.clone())
        .collect::<Vec<_>>();
    let requires_elevation = objects
        .iter()
        .any(|item| item.eligibility == "needsArchiveV2");
    let blocked_objects = objects
        .iter()
        .filter(|item| item.eligibility == "blocked")
        .count() as u64;
    let digest = preview_digest(&classified, &eligible_topology_group_ids);
    let preview_id = secure_random_hex_256()?;
    let created = Utc::now();
    let expires = created + Duration::minutes(PREVIEW_TTL_MINUTES);
    let scope_json = serde_json::to_string(&scope)
        .map_err(|error| FinalRemoveError::InvalidPreview(error.to_string()))?;
    let entry_ids_json = serde_json::to_string(
        &classified
            .iter()
            .map(|item| item.row.entry_id)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| FinalRemoveError::InvalidPreview(error.to_string()))?;
    let groups_json = serde_json::to_string(&eligible_topology_group_ids)
        .map_err(|error| FinalRemoveError::InvalidPreview(error.to_string()))?;

    let transaction = conn.unchecked_transaction()?;
    for item in &classified {
        transaction.execute(
            "UPDATE quarantine_entry
             SET removal_group_id = COALESCE(removal_group_id, ?2),
                 removal_group_fingerprint = COALESCE(removal_group_fingerprint, ?3)
             WHERE id = ?1 AND status = 'quarantined'",
            params![
                item.row.entry_id,
                item.project_group_id,
                item.row
                    .removal_group_fingerprint
                    .as_deref()
                    .or(item.row.operation_target_fingerprint.as_deref())
                    .unwrap_or(&digest),
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO permanent_delete_preview(
            preview_id, preview_digest, scope_json, entry_ids_json,
            topology_groups_json, target_count, created_at, expires_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            preview_id,
            digest,
            scope_json,
            entry_ids_json,
            groups_json,
            classified.len() as i64,
            created.to_rfc3339(),
            expires.to_rfc3339(),
        ],
    )?;
    for (ordinal, item) in classified.iter().enumerate() {
        transaction.execute(
            "INSERT INTO permanent_delete_preview_item(
                preview_id, quarantine_entry_id, removal_group_id,
                topology_group_id, ordinal, eligibility, reason
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                preview_id,
                item.row.entry_id,
                item.project_group_id,
                item.topology_group_id,
                ordinal as i64,
                item.decision.eligibility,
                item.decision.reason_code,
            ],
        )?;
    }
    transaction.commit()?;

    Ok(FinalRemovePreview {
        preview_id,
        preview_digest: digest,
        expires_at: expires.to_rfc3339(),
        projects,
        objects,
        volumes,
        eligible_topology_group_ids,
        requires_elevation,
        max_delete_objects: MAX_CAPABILITIES_PER_INVOCATION as u64,
        blocked_objects,
        archives_retained: true,
    })
}

/// Rebuild the exact grant for a non-empty subset of wholly eligible topology
/// groups. The selected target count is recomputed from persisted item rows.
pub fn final_remove_confirmation_binding(
    conn: &Connection,
    preview_id: &str,
    preview_digest: &str,
    mut selected_topology_groups: Vec<String>,
) -> Result<ConfirmationBinding, FinalRemoveError> {
    let row = conn
        .query_row(
            "SELECT preview_digest, expires_at, consumed_at
             FROM permanent_delete_preview WHERE preview_id = ?1",
            [preview_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let (stored_digest, expires_at, consumed_at) =
        row.ok_or_else(|| FinalRemoveError::InvalidPreview("preview was not found".to_string()))?;
    if consumed_at.is_some() || stored_digest != preview_digest {
        return Err(FinalRemoveError::InvalidPreview(
            "preview was consumed or its digest does not match".to_string(),
        ));
    }
    let expiry = DateTime::parse_from_rfc3339(&expires_at)
        .map_err(|_| FinalRemoveError::InvalidPreview("preview expiry is invalid".to_string()))?
        .with_timezone(&Utc);
    if expiry < Utc::now() {
        return Err(FinalRemoveError::InvalidPreview(
            "preview has expired; rebuild it before confirming".to_string(),
        ));
    }
    if selected_topology_groups.is_empty() {
        return Err(FinalRemoveError::InvalidPreview(
            "select at least one wholly eligible topology group".to_string(),
        ));
    }
    let supplied_len = selected_topology_groups.len();
    selected_topology_groups.sort_unstable();
    selected_topology_groups.dedup();
    if selected_topology_groups.len() != supplied_len {
        return Err(FinalRemoveError::InvalidPreview(
            "selected topology groups contain duplicates".to_string(),
        ));
    }
    let rows = load_persisted_preview_items(conn, preview_id)?;
    let eligible = eligible_persisted_topology_groups(&rows);
    if selected_topology_groups
        .iter()
        .any(|group| !eligible.contains(group))
    {
        return Err(FinalRemoveError::InvalidPreview(
            "selection includes a blocked, partial, or unknown topology group".to_string(),
        ));
    }
    let selected_set = selected_topology_groups
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let target_count = rows
        .iter()
        .filter(|row| selected_set.contains(&row.topology_group_id))
        .count();
    if target_count == 0 || target_count > MAX_CAPABILITIES_PER_INVOCATION {
        return Err(FinalRemoveError::InvalidPreview(format!(
            "selected topology groups contain {target_count} objects; the immutable one-UAC operation bound is {MAX_CAPABILITIES_PER_INVOCATION}"
        )));
    }
    ConfirmationBinding::new(
        preview_id,
        preview_digest,
        selected_topology_groups,
        target_count as u32,
    )
    .map_err(FinalRemoveError::Random)
}

#[derive(Debug, Clone)]
struct JournaledBatchItem {
    batch_item_id: i64,
    batch_id: i64,
    batch_operation_id: i64,
    operation_item_id: i64,
    row: PreviewRow,
    project_group_id: String,
    topology_group_id: String,
}

type BatchItemFailure = (Box<JournaledBatchItem>, String, String);

enum ArchiveBinding {
    New {
        container: ObjectArchiveContainer,
        final_path: PathBuf,
    },
    Existing {
        archive: CommittedObjectArchive,
        archive_id: i64,
        raw_backup_blake3: String,
        semantic_blake3: String,
        stream_count: u32,
    },
}

struct PreparedBatchItem {
    journal: JournaledBatchItem,
    held: Option<BoundObjectProof>,
    scratch: BoundScratchRoot,
    archive: Option<ArchiveBinding>,
    allow_existing_archive_directory_time_drift: bool,
}

#[derive(Clone)]
enum PreflightArchive {
    New,
    Existing {
        path: PathBuf,
        stamp: FileStamp,
        archive_blake3: String,
        semantic_blake3: String,
    },
}

#[derive(Clone)]
struct PreflightBatchItem {
    journal: JournaledBatchItem,
    source_stamp: FileStamp,
    source_blake3: String,
    scratch_path: PathBuf,
    scratch_stamp: FileStamp,
    destination_volume_id: String,
    available_space_bytes: Option<u64>,
    persistent_archive_bytes: u64,
    transient_peak_bytes: u64,
    archive: PreflightArchive,
    allow_existing_archive_directory_time_drift: bool,
}

/// Execute one preview-bound selection. The helper only creates/verifies
/// archives; every delete remains a medium-integrity parent exact-handle
/// disposition after a durable per-item CAS.
#[allow(clippy::too_many_arguments)]
pub fn execute_final_remove_batch(
    conn: &Connection,
    tokens: &ConfirmTokenStore,
    confirmation_token: &str,
    preview_id: &str,
    preview_digest_value: &str,
    selected_topology_groups: Vec<String>,
    helper_path: &Path,
    public_batch_id: &str,
) -> Result<FinalRemoveBatchResult, FinalRemoveError> {
    let control = || false;
    let mut observer = |_: FinalRemoveBatchProgress| {};
    execute_final_remove_batch_controlled(
        conn,
        tokens,
        confirmation_token,
        preview_id,
        preview_digest_value,
        selected_topology_groups,
        helper_path,
        public_batch_id,
        &control,
        &mut observer,
    )
}

/// Execute one preview-bound selection with cooperative stop and progress
/// observation. The compatibility wrapper above remains source-compatible and
/// simply supplies a never-stop control plus a no-op observer.
#[allow(clippy::too_many_arguments)]
pub fn execute_final_remove_batch_controlled(
    conn: &Connection,
    tokens: &ConfirmTokenStore,
    confirmation_token: &str,
    preview_id: &str,
    preview_digest_value: &str,
    selected_topology_groups: Vec<String>,
    helper_path: &Path,
    public_batch_id: &str,
    control: &dyn FinalRemoveBatchControl,
    observer: &mut dyn FinalRemoveBatchObserver,
) -> Result<FinalRemoveBatchResult, FinalRemoveError> {
    if public_batch_id.is_empty()
        || public_batch_id.len() > 128
        || !public_batch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(FinalRemoveError::InvalidPreview(
            "public batch id has an invalid bounded encoding".to_string(),
        ));
    }
    let binding = final_remove_confirmation_binding(
        conn,
        preview_id,
        preview_digest_value,
        selected_topology_groups.clone(),
    )?;
    if !tokens.consume_scoped(confirmation_token, ConfirmAction::PermanentDelete, &binding) {
        return Err(FinalRemoveError::ConfirmRequired);
    }

    // Rebuild every persisted item before consuming the preview. A caller can
    // select a subset, but cannot keep an old digest after any DB authority row
    // changed.
    let persisted = load_persisted_preview_items(conn, preview_id)?;
    let current_rows = load_preview_rows(conn)?
        .into_iter()
        .map(|row| (row.entry_id, row))
        .collect::<BTreeMap<_, _>>();
    let mut current = Vec::with_capacity(persisted.len());
    for item in &persisted {
        let row = current_rows.get(&item.entry_id).cloned().ok_or_else(|| {
            FinalRemoveError::InvalidPreview(format!(
                "held entry {} changed or left quarantine after preview",
                item.entry_id
            ))
        })?;
        let classified = classify_row(row);
        if classified.project_group_id != item.project_group_id
            || classified.topology_group_id != item.topology_group_id
        {
            return Err(FinalRemoveError::InvalidPreview(format!(
                "held entry {} changed project/topology identity after preview",
                item.entry_id
            )));
        }
        current.push(classified);
    }
    block_unsupported_multi_path_topologies(&mut current);
    apply_capacity_block(&mut current);
    let current_eligible = wholly_eligible_topology_groups(&current);
    if preview_digest(&current, &current_eligible) != preview_digest_value {
        return Err(FinalRemoveError::InvalidPreview(
            "held identities or archive eligibility changed; rebuild the preview".to_string(),
        ));
    }
    let selected = binding
        .topology_groups
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut selected_rows = current
        .into_iter()
        .filter(|item| selected.contains(&item.topology_group_id))
        .collect::<Vec<_>>();
    if selected_rows.len() != binding.target_count as usize {
        return Err(FinalRemoveError::InvalidPreview(
            "selected topology target count changed after confirmation".to_string(),
        ));
    }
    // Children are archived and disposed before their containing directories.
    // Stable entry id tie-breaking keeps the immutable template order
    // deterministic across restarts.
    selected_rows.sort_by_key(|item| {
        (
            std::cmp::Reverse(item.row.held_path.matches(['\\', '/']).count()),
            item.row.entry_id,
        )
    });
    let requested_total = selected_rows.len() as u64;

    let now = Utc::now().to_rfc3339();
    let groups_json = serde_json::to_string(&binding.topology_groups)
        .map_err(|error| FinalRemoveError::InvalidPreview(error.to_string()))?;
    let plan_json = serde_json::json!({
        "schema": "permanent_delete_batch/1",
        "previewId": preview_id,
        "previewDigest": preview_digest_value,
        "selectedTopologyGroupIds": binding.topology_groups,
        "archiveRetained": true,
    })
    .to_string();
    let nonce_seed = secure_random_hex_256()?;
    let transaction = conn.unchecked_transaction()?;
    if !consume_preview_cas(&transaction, preview_id, preview_digest_value, &now)? {
        transaction.rollback()?;
        return Err(FinalRemoveError::InvalidPreview(
            "preview was already consumed or expired".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO operation(kind, status, plan_json, target_fingerprint,
                               created_at, started_at)
         VALUES('permanent_delete_batch', 'waiting_for_uac', ?1, ?2, ?3, ?3)",
        params![plan_json, preview_digest_value, now],
    )?;
    let operation_id = transaction.last_insert_rowid();
    transaction.execute(
        "INSERT INTO permanent_delete_batch(
            public_id, operation_id, preview_id, preview_digest, selected_groups_json,
            requested_count, status, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'waiting_for_uac', ?7)",
        params![
            public_batch_id,
            operation_id,
            preview_id,
            preview_digest_value,
            groups_json,
            selected_rows.len() as i64,
            now,
        ],
    )?;
    let batch_id = transaction.last_insert_rowid();
    let mut journaled = Vec::with_capacity(selected_rows.len());
    for item in selected_rows {
        let stamp = row_stamp(&item.row)?;
        let hash = item.row.result_blake3.as_deref().ok_or_else(|| {
            FinalRemoveError::InvalidPreview("selected item has no content hash".to_string())
        })?;
        transaction.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, bytes, checksum_before,
                expected_volume_id, expected_file_id, expected_blake3,
                expected_modified_unix_seconds, status
             ) VALUES(?1, 'final_remove_bound', ?2, ?3, ?4, ?5, ?6, ?4, ?7, 'pending')",
            params![
                operation_id,
                item.row.held_path,
                stamp.bytes as i64,
                hash,
                stamp.volume_id,
                stamp.file_id,
                stamp.modified_unix_seconds,
            ],
        )?;
        let operation_item_id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO permanent_delete_batch_item(
                batch_id, operation_item_id, quarantine_entry_id,
                removal_group_id, topology_group_id, held_path,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds, expected_content_blake3,
                logical_bytes, phase, status, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                      ?11, ?12, 'preflight', 'planned', ?13, ?13)",
            params![
                batch_id,
                operation_item_id,
                item.row.entry_id,
                item.project_group_id,
                item.topology_group_id,
                item.row.held_path,
                stamp.volume_id,
                stamp.file_id,
                stamp.bytes as i64,
                stamp.modified_unix_seconds,
                hash,
                item.row.logical_bytes as i64,
                now,
            ],
        )?;
        journaled.push(JournaledBatchItem {
            batch_item_id: transaction.last_insert_rowid(),
            batch_id,
            batch_operation_id: operation_id,
            operation_item_id,
            row: item.row,
            project_group_id: item.project_group_id,
            topology_group_id: item.topology_group_id,
        });
    }
    let request_digest =
        journal_request_digest(operation_id, preview_digest_value, &binding.topology_groups);
    let nonce_digest = blake3::hash(nonce_seed.as_bytes()).to_hex().to_string();
    transaction.execute(
        "INSERT INTO elevation_capability(
            operation_id, request_digest, transport_nonce, nonce_digest, status, issued_at
         ) VALUES(?1, ?2, ?3, ?4, 'issued', ?5)",
        params![operation_id, request_digest, nonce_seed, nonce_digest, now],
    )?;
    let elevation_id = transaction.last_insert_rowid();
    transaction.commit()?;
    observer.on_progress(FinalRemoveBatchProgress {
        batch_id: public_batch_id.to_string(),
        phase: FinalRemoveBatchPhase::WaitingForUac,
        total: requested_total,
        completed: 0,
        current_path: None,
    });

    let mut preflighted = Vec::new();
    let mut blocked_groups = BTreeSet::new();
    for item in journaled {
        match preflight_batch_item(conn, item) {
            Ok(value) => preflighted.push(value),
            Err((item, code, message)) => {
                blocked_groups.insert(item.topology_group_id.clone());
                mark_batch_item_blocked(conn, &item, &code, &message)?;
            }
        }
    }
    // Topology is atomic: if one member failed preflight, drop and block every
    // prepared member before the helper sees a capability.
    let mut retained = Vec::with_capacity(preflighted.len());
    for item in preflighted {
        if blocked_groups.contains(&item.journal.topology_group_id) {
            mark_batch_item_blocked(
                conn,
                &item.journal,
                "identityChanged",
                "another member of the same topology group failed preflight",
            )?;
        } else {
            retained.push(item);
        }
    }
    let preflighted = enforce_aggregate_archive_space(conn, retained)?;
    if preflighted.is_empty() {
        finish_batch_without_delete(
            conn,
            operation_id,
            batch_id,
            elevation_id,
            "failed",
            "no selected topology group passed handle-bound archive preflight",
        )?;
        let result = load_batch_result(conn, batch_id)?;
        observer.on_progress(FinalRemoveBatchProgress {
            batch_id: public_batch_id.to_string(),
            phase: FinalRemoveBatchPhase::Finished,
            total: requested_total,
            completed: requested_total,
            current_path: None,
        });
        return Ok(result);
    }

    journal_capability_layouts(conn, elevation_id, &preflighted, &nonce_seed)?;
    let parent = current_parent_binding()?;
    let issued_at = Utc::now().timestamp();
    let capabilities = preflighted
        .iter()
        .enumerate()
        .map(|(index, item)| capability_template(item, &nonce_seed, index as u32))
        .collect::<Result<Vec<_>, _>>()?;
    let request = ElevatedRequest {
        schema: PROTOCOL_SCHEMA.to_string(),
        nonce: nonce_seed.clone(),
        issued_at_unix_seconds: issued_at,
        expires_at_unix_seconds: issued_at + 120,
        parent,
        plan_fingerprint: preview_digest_value.to_string(),
        operation_id,
        journal_capability_blake3: request_digest,
        synthetic_test: false,
        capabilities,
    };
    let operation_uuid = secure_random_hex_256()?[..32].to_string();
    let progress_completed = requested_total.saturating_sub(preflighted.len() as u64);
    let mut lazy_batch = FinalRemoveLazyBatch::new(
        conn,
        preflighted,
        nonce_seed,
        helper_path,
        public_batch_id,
        requested_total,
        progress_completed,
        control,
        observer,
    );
    let response =
        match invoke_elevated_helper_lazy(request, &operation_uuid, helper_path, &mut lazy_batch) {
            Ok(response) => response,
            Err(error) => {
                let interruption = lazy_batch.interruption_reason();
                let error_message = error.to_string();
                let (code, message) = if let Some(reason) = interruption {
                    let (code, message, _) = interruption_details(reason);
                    (code, message)
                } else {
                    (helper_reason_code(&error_message), error_message.as_str())
                };
                lazy_batch.mark_unprocessed(code, message)?;
                if let Some(reason) = interruption {
                    let (_, _, elevation_status) = interruption_details(reason);
                    finalize_batch(conn, operation_id, batch_id, Some(reason))?;
                    conn.execute(
                        "UPDATE elevation_capability
                         SET status = ?2, finished_at = ?3, error = ?4
                         WHERE id = ?1 AND status = 'issued'",
                        params![
                            elevation_id,
                            elevation_status,
                            Utc::now().to_rfc3339(),
                            error_message
                        ],
                    )?;
                    let result = load_batch_result(conn, batch_id)?;
                    let phase = if result.status == "interrupted" {
                        FinalRemoveBatchPhase::Interrupted
                    } else {
                        FinalRemoveBatchPhase::Finished
                    };
                    lazy_batch.finish_progress(phase);
                    return Ok(result);
                }
                let has_disposition = batch_has_started_disposition(conn, batch_id)?;
                if has_disposition {
                    finalize_batch(conn, operation_id, batch_id, None)?;
                    conn.execute(
                        "UPDATE elevation_capability
                     SET status = 'failed', finished_at = ?2, error = ?3
                     WHERE id = ?1 AND status = 'issued'",
                        params![elevation_id, Utc::now().to_rfc3339(), error.to_string()],
                    )?;
                } else {
                    finish_batch_without_delete(
                        conn,
                        operation_id,
                        batch_id,
                        elevation_id,
                        if code == "uacCancelled" {
                            "cancelled"
                        } else {
                            "failed"
                        },
                        &error_message,
                    )?;
                }
                let result = load_batch_result(conn, batch_id)?;
                let phase = if result.status == "interrupted" {
                    FinalRemoveBatchPhase::Interrupted
                } else {
                    FinalRemoveBatchPhase::Finished
                };
                lazy_batch.finish_progress(phase);
                return Ok(result);
            }
        };
    let success = match response {
        ElevatedResponse::Success(success) => success,
        ElevatedResponse::Failure(failure) => {
            lazy_batch.mark_unprocessed(&failure.code, &failure.message)?;
            if batch_has_started_disposition(conn, batch_id)? {
                finalize_batch(conn, operation_id, batch_id, None)?;
                conn.execute(
                    "UPDATE elevation_capability
                     SET status = 'failed', finished_at = ?2, error = ?3
                     WHERE id = ?1 AND status = 'issued'",
                    params![elevation_id, Utc::now().to_rfc3339(), failure.message],
                )?;
            } else {
                finish_batch_without_delete(
                    conn,
                    operation_id,
                    batch_id,
                    elevation_id,
                    "failed",
                    &failure.message,
                )?;
            }
            let result = load_batch_result(conn, batch_id)?;
            let phase = if result.status == "interrupted" {
                FinalRemoveBatchPhase::Interrupted
            } else {
                FinalRemoveBatchPhase::Finished
            };
            lazy_batch.finish_progress(phase);
            return Ok(result);
        }
    };
    if let Some(reason) = lazy_batch.interruption_reason() {
        let (code, message, elevation_status) = interruption_details(reason);
        lazy_batch.mark_unprocessed(code, message)?;
        conn.execute(
            "UPDATE elevation_capability
             SET status = ?2, helper_image_sha256 = ?3,
                 finished_at = ?4, error = ?5
             WHERE id = ?1 AND status = 'issued'",
            params![
                elevation_id,
                elevation_status,
                success.helper_image_sha256,
                Utc::now().to_rfc3339(),
                message,
            ],
        )?;
        finalize_batch(conn, operation_id, batch_id, Some(reason))?;
        let result = load_batch_result(conn, batch_id)?;
        let phase = if result.status == "interrupted" {
            FinalRemoveBatchPhase::Interrupted
        } else {
            FinalRemoveBatchPhase::Finished
        };
        lazy_batch.finish_progress(phase);
        return Ok(result);
    }
    conn.execute(
        "UPDATE elevation_capability
         SET status = 'consumed', helper_image_sha256 = ?2,
             consumed_at = ?3, finished_at = ?3
         WHERE id = ?1 AND status = 'issued'",
        params![
            elevation_id,
            success.helper_image_sha256,
            Utc::now().to_rfc3339()
        ],
    )?;
    // All per-item proof commits and exact parent dispositions happened inside
    // `consume_chunk` while each bounded handle guard was still live. Never run
    // a second path-based/post-helper deletion pass here.
    let _ = success;
    finalize_batch(conn, operation_id, batch_id, None)?;
    let result = load_batch_result(conn, batch_id)?;
    let phase = if result.status == "interrupted" {
        FinalRemoveBatchPhase::Interrupted
    } else {
        FinalRemoveBatchPhase::Finished
    };
    lazy_batch.finish_progress(phase);
    Ok(result)
}

fn consume_preview_cas(
    transaction: &rusqlite::Transaction<'_>,
    preview_id: &str,
    preview_digest: &str,
    now: &str,
) -> Result<bool, rusqlite::Error> {
    Ok(transaction.execute(
        "UPDATE permanent_delete_preview SET consumed_at = ?2
         WHERE preview_id = ?1 AND preview_digest = ?3
           AND consumed_at IS NULL AND expires_at >= ?2",
        params![preview_id, now, preview_digest],
    )? == 1)
}

fn row_stamp(row: &PreviewRow) -> Result<FileStamp, FinalRemoveError> {
    let volume_id = row
        .result_volume_id
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FinalRemoveError::InvalidPreview("held volume id is missing".to_string()))?;
    let file_id = row
        .result_file_id
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FinalRemoveError::InvalidPreview("held file id is missing".to_string()))?;
    let bytes = row.result_bytes.ok_or_else(|| {
        FinalRemoveError::InvalidPreview("held byte length is missing".to_string())
    })?;
    let modified_unix_seconds = row.result_modified_unix_seconds.ok_or_else(|| {
        FinalRemoveError::InvalidPreview("held modification time is missing".to_string())
    })?;
    Ok(FileStamp {
        volume_id,
        file_id,
        bytes,
        modified_unix_seconds: Some(modified_unix_seconds),
    })
}

fn journal_request_digest(
    operation_id: i64,
    preview_digest: &str,
    topology_groups: &[String],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"codehangar/elevated-journal-capability/1\0");
    hasher.update(&operation_id.to_le_bytes());
    hasher.update(&(preview_digest.len() as u64).to_le_bytes());
    hasher.update(preview_digest.as_bytes());
    hasher.update(&(topology_groups.len() as u64).to_le_bytes());
    for group in topology_groups {
        hasher.update(&(group.len() as u64).to_le_bytes());
        hasher.update(group.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn preflight_batch_item(
    conn: &Connection,
    item: JournaledBatchItem,
) -> Result<PreflightBatchItem, BatchItemFailure> {
    fn blocked<T>(
        item: JournaledBatchItem,
        code: &str,
        error: impl std::fmt::Display,
    ) -> Result<T, BatchItemFailure> {
        Err((Box::new(item), code.to_string(), error.to_string()))
    }
    let source_stamp = match row_stamp(&item.row) {
        Ok(value) => value,
        Err(error) => return blocked(item, "identityChanged", error),
    };
    let source_blake3 = match item
        .row
        .result_blake3
        .clone()
        .filter(|value| is_hex_64(value))
    {
        Some(value) => value,
        None => return blocked(item, "identityChanged", "held content hash is missing"),
    };
    let source = match BoundObjectProof::open_for_archive(
        Path::new(&item.row.held_path),
        &source_stamp,
        &source_blake3,
    ) {
        Ok(value) => value,
        Err(error) => return blocked(item, "objectClassUnsupported", error),
    };
    let backup_id = match item.row.backup_id.filter(|value| *value > 0) {
        Some(value) => value,
        None => return blocked(item, "archiveMissing", "verified backup id is missing"),
    };
    let scratch_path = match conn
        .query_row(
            "SELECT destination FROM backup WHERE id = ?1 AND verified = 1",
            [backup_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        Ok(Some(value)) => PathBuf::from(value),
        Ok(None) => {
            return blocked(
                item,
                "archiveMissing",
                "verified backup destination is not available",
            )
        }
        Err(error) => return blocked(item, "journalInvalid", error),
    };
    if let Err(error) = crate::bound_fs::validate_local_mutation_path(&scratch_path) {
        return blocked(item, "archiveDestinationUnsafe", error);
    }
    let source_parent = Path::new(&item.row.held_path)
        .parent()
        .unwrap_or_else(|| Path::new(&item.row.held_path));
    let containment =
        match crate::bound_fs::bind_destination_outside_directory(&scratch_path, source_parent) {
            Ok(value) => value,
            Err(error) => return blocked(item, "archiveDestinationUnsafe", error),
        };
    let available_space_bytes = containment.available_space_bytes();
    let stream_logical_bytes = source.stream_logical_bytes();
    let transient_peak_bytes = match stream_logical_bytes.checked_add(OBJECT_ARCHIVE_OVERHEAD_BYTES)
    {
        Some(value) => value,
        None => {
            return blocked(
                item,
                "archiveInsufficientSpace",
                "archive size estimate overflowed",
            )
        }
    };
    let scratch = match BoundScratchRoot::open(&scratch_path) {
        Ok(value) => value,
        Err(error) => return blocked(item, "archiveDestinationUnsafe", error),
    };
    let scratch_stamp = scratch.stamp().clone();
    let destination_volume_id = scratch_stamp.volume_id.clone();
    let archive = if item.row.archive_id.is_some() {
        let path = match item.row.archive_path.as_deref() {
            Some(value) if valid_bounded_local_path_text(value) => PathBuf::from(value),
            _ => return blocked(item, "archiveCorrupt", "archive path is missing or unsafe"),
        };
        let stamp = match archive_stamp_from_row(&item.row) {
            Ok(value) => value,
            Err(error) => return blocked(item, "archiveCorrupt", error),
        };
        let archive_blake3 = match item
            .row
            .archive_blake3
            .clone()
            .filter(|value| is_hex_64(value))
        {
            Some(value) => value,
            None => return blocked(item, "archiveCorrupt", "archive digest is missing"),
        };
        if let Err(error) = CommittedObjectArchive::open_existing(&path, &stamp, &archive_blake3) {
            return blocked(item, "archiveCorrupt", error);
        }
        PreflightArchive::Existing {
            path,
            stamp,
            archive_blake3,
            semantic_blake3: item.row.proof_semantic.clone().unwrap_or_default(),
        }
    } else {
        PreflightArchive::New
    };
    let allow_existing_archive_directory_time_drift =
        source.is_directory() && matches!(&archive, PreflightArchive::Existing { .. });
    let persistent_archive_bytes = if matches!(archive, PreflightArchive::New) {
        transient_peak_bytes
    } else {
        0
    };
    // The pre-UAC proof is intentionally handle-neutral. Replacements after
    // this point are caught when the lazy provider reopens the same stamps.
    drop((source, scratch, containment));
    Ok(PreflightBatchItem {
        journal: item,
        source_stamp,
        source_blake3,
        scratch_path,
        scratch_stamp,
        destination_volume_id,
        available_space_bytes,
        persistent_archive_bytes,
        transient_peak_bytes,
        archive,
        allow_existing_archive_directory_time_drift,
    })
}

fn enforce_aggregate_archive_space(
    conn: &Connection,
    items: Vec<PreflightBatchItem>,
) -> Result<Vec<PreflightBatchItem>, FinalRemoveError> {
    #[derive(Default)]
    struct Budget {
        available: Option<u64>,
        availability_unknown: bool,
        persistent_new_archives: u64,
        max_transient: u64,
        overflowed: bool,
    }

    let mut budgets = BTreeMap::<String, Budget>::new();
    for item in &items {
        let budget = budgets
            .entry(item.destination_volume_id.clone())
            .or_default();
        match item.available_space_bytes {
            Some(available) => {
                budget.available = Some(
                    budget
                        .available
                        .map_or(available, |current| current.min(available)),
                );
            }
            None => budget.availability_unknown = true,
        }
        budget.persistent_new_archives = match budget
            .persistent_new_archives
            .checked_add(item.persistent_archive_bytes)
        {
            Some(value) => value,
            None => {
                budget.overflowed = true;
                u64::MAX
            }
        };
        budget.max_transient = budget.max_transient.max(item.transient_peak_bytes);
    }

    let mut blocked_volumes = BTreeMap::<String, (String, String)>::new();
    for (volume, budget) in &budgets {
        if budget.availability_unknown || budget.available.is_none() {
            blocked_volumes.insert(
                volume.clone(),
                (
                    "archiveSpaceUnknown".to_string(),
                    format!(
                        "free space could not be proven from every bound archive destination on volume {volume}"
                    ),
                ),
            );
            continue;
        }
        let required = budget
            .persistent_new_archives
            .checked_add(budget.max_transient);
        let Some(required) = required.filter(|_| !budget.overflowed) else {
            blocked_volumes.insert(
                volume.clone(),
                (
                    "archiveInsufficientSpace".to_string(),
                    format!(
                        "aggregate object_archive/2 size estimate overflowed on volume {volume}"
                    ),
                ),
            );
            continue;
        };
        let available = budget.available.unwrap_or(0);
        if available < required {
            blocked_volumes.insert(
                volume.clone(),
                (
                    "archiveInsufficientSpace".to_string(),
                    format!(
                        "volume {volume} has {available} proven free bytes but the batch needs {required}: all persistent NEW archives ({}) plus one sequential scratch peak ({})",
                        budget.persistent_new_archives, budget.max_transient
                    ),
                ),
            );
        }
    }

    let mut blocked_groups = BTreeMap::<String, (String, String)>::new();
    for item in &items {
        if let Some(reason) = blocked_volumes.get(&item.destination_volume_id) {
            blocked_groups
                .entry(item.journal.topology_group_id.clone())
                .or_insert_with(|| reason.clone());
        }
    }
    let mut retained = Vec::with_capacity(items.len());
    for item in items {
        if let Some((code, message)) = blocked_groups.get(&item.journal.topology_group_id) {
            mark_batch_item_blocked(conn, &item.journal, code, message)?;
        } else {
            retained.push(item);
        }
    }
    Ok(retained)
}

/// Freeze the exact lazy-stream index and every archive pathname before the
/// helper can create or open an archive object. Recovery accepts neither the
/// persisted pathname nor the index independently: it re-derives the pathname
/// from this capability's persisted transport nonce and the persisted global
/// index, then requires an exact match.
fn journal_capability_layouts(
    conn: &Connection,
    elevation_id: i64,
    items: &[PreflightBatchItem],
    transport_nonce: &str,
) -> Result<(), FinalRemoveError> {
    if !is_hex_64(transport_nonce) {
        return Err(FinalRemoveError::InvalidPreview(
            "elevation transport nonce has an invalid encoding".to_string(),
        ));
    }
    let expected_nonce_digest = blake3::hash(transport_nonce.as_bytes())
        .to_hex()
        .to_string();
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let elevation_operation: i64 = transaction
        .query_row(
            "SELECT operation_id FROM elevation_capability
             WHERE id = ?1 AND status = 'issued' AND transport_nonce = ?2
               AND nonce_digest = ?3",
            params![elevation_id, transport_nonce, expected_nonce_digest],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            FinalRemoveError::InvalidPreview(
                "elevation capability lost its persisted nonce authority".to_string(),
            )
        })?;

    for (index, item) in items.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| {
            FinalRemoveError::InvalidPreview(
                "elevation capability index exceeds the authenticated protocol".to_string(),
            )
        })?;
        if item.journal.batch_operation_id != elevation_operation {
            return Err(FinalRemoveError::InvalidPreview(
                "batch item belongs to a different elevation operation".to_string(),
            ));
        }
        let (partial_path, final_path, phase, event_kind, event_archive_id) = match &item.archive {
            PreflightArchive::New => {
                let partial =
                    archive_path_for_capability(&item.scratch_path, transport_nonce, index);
                let final_path = item
                    .scratch_path
                    .join(".codehangar-object-archive-v2")
                    .join(format!("entry-{:016x}.chobj", item.journal.row.entry_id));
                (
                    Some(partial),
                    final_path,
                    "archive_path_intent",
                    "capture_layout",
                    None,
                )
            }
            PreflightArchive::Existing { path, .. } => (
                None,
                path.clone(),
                "archive_verify_intent",
                "reverify_layout",
                item.journal.row.archive_id,
            ),
        };
        crate::bound_fs::validate_local_mutation_path(&final_path)?;
        if let Some(path) = partial_path.as_deref() {
            crate::bound_fs::validate_local_mutation_path(path)?;
        }
        let updated = transaction.execute(
            "UPDATE permanent_delete_batch_item
             SET elevation_capability_id = ?2, capability_index = ?3,
                 archive_partial_path = ?4, archive_final_path = ?5,
                 phase = ?6, updated_at = ?7
             WHERE id = ?1 AND batch_id = ?8 AND status = 'planned'
               AND phase = 'preflight' AND archive_id IS NULL
               AND elevation_capability_id IS NULL AND capability_index IS NULL",
            params![
                item.journal.batch_item_id,
                elevation_id,
                index as i64,
                partial_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                final_path.to_string_lossy(),
                phase,
                now,
                item.journal.batch_id,
            ],
        )?;
        if updated != 1 {
            return Err(FinalRemoveError::InvalidPreview(format!(
                "batch item {} lost archive-layout journal authority",
                item.journal.batch_item_id
            )));
        }
        let message = serde_json::json!({
            "schema": "object_archive_intent/1",
            "batchItemId": item.journal.batch_item_id,
            "capabilityIndex": index,
            "partialPath": partial_path.as_ref().map(|path| path.to_string_lossy()),
            "finalPath": final_path.to_string_lossy(),
        })
        .to_string();
        transaction.execute(
            "INSERT INTO object_archive_event(
                archive_id, operation_id, kind, status, message, created_at
             ) VALUES(?1, ?2, ?3, 'pending', ?4, ?5)",
            params![
                event_archive_id,
                item.journal.batch_operation_id,
                event_kind,
                message,
                now,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn capability_template(
    item: &PreflightBatchItem,
    nonce: &str,
    index: u32,
) -> Result<ElevatedCapability, FinalRemoveError> {
    let source = ExpectedObject {
        path: PathBuf::from(&item.journal.row.held_path),
        parent_handle_value: 0,
        stamp: item.source_stamp.clone(),
        content_blake3: item.source_blake3.clone(),
        semantic_blake3: match &item.archive {
            PreflightArchive::Existing {
                semantic_blake3, ..
            } => Some(semantic_blake3.clone()),
            PreflightArchive::New => None,
        },
        allow_internal_directory_time_drift: item.allow_existing_archive_directory_time_drift,
    };
    let scratch_root = ExpectedScratchRoot {
        path: item.scratch_path.clone(),
        parent_handle_value: 0,
        stamp: item.scratch_stamp.clone(),
    };
    let scratch_leaf = scratch_leaf_for_capability(nonce, index);
    match &item.archive {
        PreflightArchive::New => Ok(ElevatedCapability::ObjectBackupV2 {
            source,
            parent_archive_handle_value: 0,
            archive_path: archive_path_for_capability(&item.scratch_path, nonce, index),
            scratch_root,
            scratch_leaf,
        }),
        PreflightArchive::Existing {
            path,
            stamp,
            archive_blake3,
            ..
        } => Ok(ElevatedCapability::RoundtripVerify {
            source,
            parent_archive_handle_value: 0,
            archive_path: path.clone(),
            expected_archive_stamp: stamp.clone(),
            expected_archive_blake3: archive_blake3.clone(),
            scratch_root,
            scratch_leaf,
        }),
    }
}

#[derive(Clone, Copy, Default)]
struct LazyItemState {
    processed: bool,
    ready: bool,
    disposition_finished: bool,
}

struct FinalRemoveLazyBatch<'conn, 'hooks> {
    conn: &'conn Connection,
    items: Vec<PreflightBatchItem>,
    transport_nonce: String,
    helper_path: &'hooks Path,
    states: Vec<LazyItemState>,
    group_members: BTreeMap<String, Vec<usize>>,
    group_remaining: BTreeMap<String, usize>,
    completed_groups: BTreeSet<String>,
    public_batch_id: String,
    progress_total: u64,
    progress_completed: u64,
    interruption_latched: Option<FinalRemoveInterruptionReason>,
    control: &'hooks dyn FinalRemoveBatchControl,
    observer: &'hooks mut dyn FinalRemoveBatchObserver,
}

impl<'conn, 'hooks> FinalRemoveLazyBatch<'conn, 'hooks> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        conn: &'conn Connection,
        items: Vec<PreflightBatchItem>,
        transport_nonce: String,
        helper_path: &'hooks Path,
        public_batch_id: &str,
        progress_total: u64,
        progress_completed: u64,
        control: &'hooks dyn FinalRemoveBatchControl,
        observer: &'hooks mut dyn FinalRemoveBatchObserver,
    ) -> Self {
        let mut group_members = BTreeMap::<String, Vec<usize>>::new();
        for (index, item) in items.iter().enumerate() {
            group_members
                .entry(item.journal.topology_group_id.clone())
                .or_default()
                .push(index);
        }
        let group_remaining = group_members
            .iter()
            .map(|(group, members)| (group.clone(), members.len()))
            .collect();
        Self {
            states: vec![LazyItemState::default(); items.len()],
            conn,
            items,
            transport_nonce,
            helper_path,
            group_members,
            group_remaining,
            completed_groups: BTreeSet::new(),
            public_batch_id: public_batch_id.to_string(),
            progress_total,
            progress_completed,
            interruption_latched: None,
            control,
            observer,
        }
    }

    fn emit_progress(&mut self, phase: FinalRemoveBatchPhase, current_path: Option<String>) {
        self.observer.on_progress(FinalRemoveBatchProgress {
            batch_id: self.public_batch_id.clone(),
            phase,
            total: self.progress_total,
            completed: self.progress_completed.min(self.progress_total),
            current_path,
        });
    }

    fn interruption_reason(&self) -> Option<FinalRemoveInterruptionReason> {
        let latched = self.interruption_latched?;
        prioritize_interruption(Some(latched), self.control.interruption_reason())
    }

    fn finish_progress(&mut self, phase: FinalRemoveBatchPhase) {
        self.progress_completed = self.progress_total;
        self.emit_progress(phase, None);
    }

    fn mark_unprocessed(&self, code: &str, message: &str) -> Result<(), FinalRemoveError> {
        let Some(batch_id) = self.items.first().map(|item| item.journal.batch_id) else {
            return Ok(());
        };
        // A previous chunk may have committed an archive for only part of an
        // atomic topology group. `processed` therefore does not imply that the
        // item reached parent disposition. Terminalize every item which is
        // still merely planned/ready, while preserving the recovery-significant
        // deleted/deleting/interrupted states verbatim.
        terminalize_undisposed_batch_items(self.conn, batch_id, code, message)
    }

    fn complete_ready_groups(
        &mut self,
        touched_groups: &BTreeSet<String>,
        direct_proofs: &mut BTreeMap<usize, BoundDeleteProof>,
    ) -> Result<(), crate::ElevatedTransportError> {
        let ready_groups = complete_group_order(
            touched_groups,
            &self.completed_groups,
            &self.group_remaining,
            &self.group_members,
        );
        for group in ready_groups {
            let indexes = self.group_members.get(&group).cloned().ok_or_else(|| {
                crate::ElevatedTransportError::Protocol(
                    "lazy topology group index is missing".to_string(),
                )
            })?;
            let current_path = indexes
                .first()
                .map(|index| self.items[*index].journal.row.held_path.clone());
            self.emit_progress(
                FinalRemoveBatchPhase::ParentDisposition,
                current_path.clone(),
            );
            if interruption_before_topology_group(&mut self.interruption_latched, self.control)
                .is_some()
            {
                self.emit_progress(FinalRemoveBatchPhase::Interrupted, current_path);
                break;
            }
            self.emit_progress(FinalRemoveBatchPhase::Deleting, current_path.clone());
            if indexes.iter().all(|index| self.states[*index].ready) {
                let all_direct = indexes
                    .iter()
                    .all(|index| direct_proofs.contains_key(index));
                let delete_result = if all_direct {
                    let proofs = indexes
                        .iter()
                        .map(|index| {
                            direct_proofs.remove(index).ok_or_else(|| {
                                FinalRemoveError::InvalidPreview(
                                    "helper-verified delete handle disappeared".to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| {
                            crate::ElevatedTransportError::Protocol(error.to_string())
                        })?;
                    delete_bound_topology_group(self.conn, proofs, self.helper_path)
                } else {
                    // A group split across chunks cannot retain every handle
                    // without defeating the bounded lazy contract. Drop any
                    // current-chunk proof, then rebind every member to the exact
                    // persisted stamp/hash/archive authority immediately before
                    // the journal-first disposition CAS.
                    for index in &indexes {
                        direct_proofs.remove(index);
                    }
                    let journals = indexes
                        .iter()
                        .map(|index| self.items[*index].journal.clone())
                        .collect::<Vec<_>>();
                    delete_ready_topology_group(self.conn, &journals, self.helper_path)
                };
                delete_result.map_err(|error| {
                    crate::ElevatedTransportError::Protocol(format!(
                        "parent exact-handle disposition stopped: {error}"
                    ))
                })?;
                for index in &indexes {
                    self.states[*index].disposition_finished = true;
                }
            } else {
                for index in &indexes {
                    if self.states[*index].ready {
                        mark_batch_item_blocked(
                            self.conn,
                            &self.items[*index].journal,
                            "externalHardlink",
                            "another member of the topology group failed object_archive/2 verification",
                        )
                        .map_err(|error| {
                            crate::ElevatedTransportError::Protocol(error.to_string())
                        })?;
                    }
                }
            }
            self.completed_groups.insert(group.clone());
            self.progress_completed = self
                .progress_completed
                .saturating_add(indexes.len() as u64)
                .min(self.progress_total);
            self.emit_progress(FinalRemoveBatchPhase::Deleting, current_path);
        }
        Ok(())
    }
}

/// Latch a cooperative stop exactly at a topology-group boundary. Keeping the
/// latch separate makes it impossible for later transport/chunk code to poll
/// the external signal while an indivisible group is being disposed.
fn interruption_before_topology_group(
    interruption_latched: &mut Option<FinalRemoveInterruptionReason>,
    control: &dyn FinalRemoveBatchControl,
) -> Option<FinalRemoveInterruptionReason> {
    *interruption_latched =
        prioritize_interruption(*interruption_latched, control.interruption_reason());
    *interruption_latched
}

fn prioritize_interruption(
    latched: Option<FinalRemoveInterruptionReason>,
    observed: Option<FinalRemoveInterruptionReason>,
) -> Option<FinalRemoveInterruptionReason> {
    if latched == Some(FinalRemoveInterruptionReason::ProgressObserverFailed)
        || observed == Some(FinalRemoveInterruptionReason::ProgressObserverFailed)
    {
        Some(FinalRemoveInterruptionReason::ProgressObserverFailed)
    } else {
        latched.or(observed)
    }
}

fn complete_group_order(
    touched_groups: &BTreeSet<String>,
    completed_groups: &BTreeSet<String>,
    group_remaining: &BTreeMap<String, usize>,
    group_members: &BTreeMap<String, Vec<usize>>,
) -> Vec<String> {
    let mut ready_groups = touched_groups
        .iter()
        .filter(|group| {
            !completed_groups.contains(*group)
                && group_remaining.get(*group).copied().unwrap_or(0) == 0
        })
        .cloned()
        .collect::<Vec<_>>();
    // `items` is globally deepest-first. Preserve that order here: an opaque
    // topology id must never cause a parent directory to be disposed before a
    // child merely because the parent id sorts first.
    ready_groups.sort_by_key(|group| {
        group_members
            .get(group)
            .and_then(|members| members.first())
            .copied()
            .unwrap_or(usize::MAX)
    });
    ready_groups
}

impl LazyElevatedCapabilityBatch for FinalRemoveLazyBatch<'_, '_> {
    type Guard = Vec<PreparedBatchItem>;

    fn total_capabilities(&self) -> usize {
        self.items.len()
    }

    fn materialize_chunk(
        &mut self,
        range: std::ops::Range<usize>,
        nonce: &str,
    ) -> Result<MaterializedCapabilityChunk<Self::Guard>, crate::ElevatedTransportError> {
        let current_path = self
            .items
            .get(range.start)
            .map(|item| item.journal.row.held_path.clone());
        self.emit_progress(FinalRemoveBatchPhase::VerifyingArchives, current_path);
        let mut guard = Vec::with_capacity(range.len());
        let mut capabilities = Vec::with_capacity(range.len());
        for index in range {
            let template = &self.items[index];
            let prepared =
                prepare_batch_item(self.conn, template.journal.clone(), nonce, index as u32)
                    .map_err(|(item, code, message)| {
                        let _ = mark_batch_item_blocked(self.conn, &item, &code, &message);
                        crate::ElevatedTransportError::Protocol(format!(
                            "lazy capability {index} could not be rebound: {code}: {message}"
                        ))
                    })?;
            let capability = capability_for_prepared(&prepared, nonce, index as u32)
                .map_err(|error| crate::ElevatedTransportError::Protocol(error.to_string()))?;
            capabilities.push(capability);
            guard.push(prepared);
        }
        Ok(MaterializedCapabilityChunk {
            capabilities,
            guard,
        })
    }

    fn consume_chunk(
        &mut self,
        start_index: usize,
        mut guard: Self::Guard,
        results: &[ElevatedItemResult],
    ) -> Result<(), crate::ElevatedTransportError> {
        if guard.len() != results.len() {
            return Err(crate::ElevatedTransportError::Protocol(
                "lazy result/handle guard length mismatch".to_string(),
            ));
        }
        let mut touched_groups = BTreeSet::new();
        for (offset, (item, result)) in guard.iter_mut().zip(results).enumerate() {
            let index = start_index + offset;
            if self.states[index].processed {
                return Err(crate::ElevatedTransportError::Protocol(
                    "lazy capability result was consumed more than once".to_string(),
                ));
            }
            self.states[index].processed = true;
            let group = self.items[index].journal.topology_group_id.clone();
            let remaining = self.group_remaining.get_mut(&group).ok_or_else(|| {
                crate::ElevatedTransportError::Protocol(
                    "lazy topology group counter is missing".to_string(),
                )
            })?;
            *remaining = remaining.checked_sub(1).ok_or_else(|| {
                crate::ElevatedTransportError::Protocol(
                    "lazy topology group counter underflowed".to_string(),
                )
            })?;
            touched_groups.insert(group);
            match result {
                ElevatedItemResult::Blocked { code, message, .. } => {
                    if code == "scratchCleanupPending" {
                        journal_scratch_cleanup_pending(
                            self.conn,
                            &item.journal,
                            item.scratch.path(),
                            &self.transport_nonce,
                            index as u32,
                            message,
                        )
                        .map_err(|error| {
                            crate::ElevatedTransportError::Protocol(error.to_string())
                        })?;
                    }
                    mark_batch_item_blocked(self.conn, &item.journal, code, message).map_err(
                        |error| crate::ElevatedTransportError::Protocol(error.to_string()),
                    )?;
                }
                ElevatedItemResult::Ready(proof) => {
                    match commit_archive_proof(self.conn, item, proof) {
                        Ok(()) => self.states[index].ready = true,
                        Err(error) => {
                            mark_batch_item_blocked(
                                self.conn,
                                &item.journal,
                                "archiveCorrupt",
                                &error.to_string(),
                            )
                            .map_err(|journal| {
                                crate::ElevatedTransportError::Protocol(journal.to_string())
                            })?;
                        }
                    }
                }
            }
        }
        let mut direct_proofs = BTreeMap::new();
        for (offset, item) in guard.iter_mut().enumerate() {
            let index = start_index + offset;
            if self.states[index].ready {
                let proof = take_helper_verified_delete_proof(item)
                    .map_err(|error| crate::ElevatedTransportError::Protocol(error.to_string()))?;
                direct_proofs.insert(index, proof);
            }
        }
        // Release scratch/archive guards before bottom-up disposition while
        // retaining the helper-verified, ancestor-free source handles above.
        drop(guard);
        self.complete_ready_groups(&touched_groups, &mut direct_proofs)
    }

    fn stop_stream_requested(&self) -> bool {
        self.interruption_latched.is_some()
    }
}

struct PersistedArchiveLayout {
    elevation_id: i64,
    partial_path: Option<PathBuf>,
    final_path: PathBuf,
    phase: String,
}

type PersistedArchiveLayoutRow = (
    i64,
    i64,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
);

fn load_persisted_archive_layout(
    conn: &Connection,
    item: &JournaledBatchItem,
    nonce: &str,
    capability_index: u32,
    destination: &Path,
) -> Result<PersistedArchiveLayout, FinalRemoveError> {
    let row: Option<PersistedArchiveLayoutRow> = conn
        .query_row(
            "SELECT bi.elevation_capability_id, bi.capability_index,
                    bi.archive_partial_path, bi.archive_final_path, bi.phase,
                    ec.transport_nonce, ec.nonce_digest, ec.operation_id
             FROM permanent_delete_batch_item bi
             JOIN elevation_capability ec ON ec.id = bi.elevation_capability_id
             WHERE bi.id = ?1 AND bi.batch_id = ?2 AND bi.status = 'planned'
               AND ec.status = 'issued'",
            params![item.batch_item_id, item.batch_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;
    let (
        elevation_id,
        persisted_index,
        persisted_partial,
        persisted_final,
        phase,
        persisted_nonce,
        nonce_digest,
        operation_id,
    ) = row.ok_or_else(|| {
        FinalRemoveError::InvalidPreview(
            "batch item has no issued persisted archive layout".to_string(),
        )
    })?;
    if operation_id != item.batch_operation_id
        || persisted_index != capability_index as i64
        || persisted_nonce != nonce
        || !is_hex_64(&persisted_nonce)
        || blake3::hash(persisted_nonce.as_bytes()).to_hex().as_str() != nonce_digest
    {
        return Err(FinalRemoveError::InvalidPreview(
            "batch archive layout does not match its persisted nonce/index authority".to_string(),
        ));
    }
    let derived_partial = archive_path_for_capability(destination, nonce, capability_index);
    let partial_path = persisted_partial.map(PathBuf::from);
    if let Some(path) = partial_path.as_deref() {
        crate::bound_fs::validate_local_mutation_path(path)?;
        if path != derived_partial {
            return Err(FinalRemoveError::InvalidPreview(
                "persisted partial archive path is not nonce/index-derived".to_string(),
            ));
        }
    }
    let final_path = PathBuf::from(persisted_final);
    crate::bound_fs::validate_local_mutation_path(&final_path)?;
    Ok(PersistedArchiveLayout {
        elevation_id,
        partial_path,
        final_path,
        phase,
    })
}

fn persist_archive_initial_stamp(
    conn: &Connection,
    item: &JournaledBatchItem,
    container: &ObjectArchiveContainer,
    layout: &PersistedArchiveLayout,
) -> Result<(), FinalRemoveError> {
    let stamp = container.initial_stamp();
    if stamp.volume_id.is_empty() || stamp.file_id.is_empty() {
        return Err(FinalRemoveError::InvalidPreview(
            "new archive container has no durable file identity".to_string(),
        ));
    }
    let partial_path = layout.partial_path.as_deref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview(
            "new archive container has no persisted partial path".to_string(),
        )
    })?;
    if container.path() != partial_path {
        return Err(FinalRemoveError::InvalidPreview(
            "new archive handle is not bound to its persisted partial path".to_string(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET archive_initial_volume_id = ?2, archive_initial_file_id = ?3,
             archive_initial_bytes = ?4,
             archive_initial_modified_unix_seconds = ?5,
             phase = 'archive_container_bound', updated_at = ?6
         WHERE id = ?1 AND batch_id = ?7 AND status = 'planned'
           AND phase = 'archive_path_intent' AND elevation_capability_id = ?8
           AND archive_partial_path = ?9 AND archive_final_path = ?10
           AND archive_initial_file_id IS NULL",
        params![
            item.batch_item_id,
            stamp.volume_id,
            stamp.file_id,
            stamp.bytes as i64,
            stamp.modified_unix_seconds,
            now,
            item.batch_id,
            layout.elevation_id,
            partial_path.to_string_lossy(),
            layout.final_path.to_string_lossy(),
        ],
    )?;
    if updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "new archive identity could not be journaled before helper capture".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO object_archive_event(
            archive_id, operation_id, kind, status, message, created_at
         ) VALUES(NULL, ?1, 'capture_container_bound', 'pending', ?2, ?3)",
        params![
            item.batch_operation_id,
            serde_json::json!({
                "schema": "object_archive_container/1",
                "batchItemId": item.batch_item_id,
                "partialPath": partial_path.to_string_lossy(),
                "initialVolumeId": stamp.volume_id,
                "initialFileId": stamp.file_id,
                "initialBytes": stamp.bytes,
                "initialModifiedUnixSeconds": stamp.modified_unix_seconds,
            })
            .to_string(),
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn prepare_batch_item(
    conn: &Connection,
    item: JournaledBatchItem,
    nonce: &str,
    capability_index: u32,
) -> Result<PreparedBatchItem, BatchItemFailure> {
    fn blocked<T>(
        item: JournaledBatchItem,
        code: &str,
        error: impl std::fmt::Display,
    ) -> Result<T, BatchItemFailure> {
        Err((Box::new(item), code.to_string(), error.to_string()))
    }

    let expected_stamp = match row_stamp(&item.row) {
        Ok(value) => value,
        Err(error) => return blocked(item, "identityChanged", error),
    };
    let expected_hash = match item
        .row
        .result_blake3
        .clone()
        .filter(|value| is_hex_64(value))
    {
        Some(value) => value,
        None => return blocked(item, "identityChanged", "held content hash is missing"),
    };
    // The helper receives a share-compatible exact handle. A containing
    // directory may have a different mtime only when an original existing
    // archive is already authoritative and every planned descendant has been
    // durably deleted by earlier chunks. Identity/type/hash/streams/emptiness
    // are still rebound from the live handle.
    let exact_held = BoundObjectProof::open_for_archive(
        Path::new(&item.row.held_path),
        &expected_stamp,
        &expected_hash,
    );
    let held = match exact_held {
        Ok(value) => value,
        Err(exact_error) => {
            #[cfg(windows)]
            {
                let can_relax_directory_time = item.row.archive_id.is_some()
                    && match planned_descendants_were_deleted(conn, &item) {
                        Ok(value) => value,
                        Err(error) => return blocked(item, "identityChanged", error),
                    };
                if can_relax_directory_time {
                    match BoundObjectProof::open_for_archive_allow_directory_time_drift(
                        Path::new(&item.row.held_path),
                        &expected_stamp,
                        &expected_hash,
                    ) {
                        Ok(value) => value,
                        Err(error) => return blocked(item, "identityChanged", error),
                    }
                } else {
                    return blocked(item, "identityChanged", exact_error);
                }
            }
            #[cfg(not(windows))]
            {
                return blocked(item, "identityChanged", exact_error);
            }
        }
    };
    let backup_id = match item.row.backup_id.filter(|value| *value > 0) {
        Some(value) => value,
        None => return blocked(item, "archiveMissing", "verified backup id is missing"),
    };
    let destination = match conn
        .query_row(
            "SELECT destination FROM backup WHERE id = ?1 AND verified = 1",
            [backup_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        Ok(Some(value)) => PathBuf::from(value),
        Ok(None) => {
            return blocked(
                item,
                "archiveMissing",
                "verified backup destination is not available",
            )
        }
        Err(error) => return blocked(item, "journalInvalid", error),
    };
    if let Err(error) = crate::bound_fs::validate_local_mutation_path(&destination) {
        return blocked(item, "archiveDestinationUnsafe", error);
    }
    let source_parent = Path::new(&item.row.held_path)
        .parent()
        .unwrap_or_else(|| Path::new(&item.row.held_path));
    let containment =
        match crate::bound_fs::bind_destination_outside_directory(&destination, source_parent) {
            Ok(value) => value,
            Err(error) => return blocked(item, "archiveDestinationUnsafe", error),
        };
    let needed = held
        .stream_logical_bytes()
        .saturating_mul(2)
        .saturating_add(16 * 1024 * 1024);
    match containment.available_space_bytes() {
        Some(available) if available >= needed => {}
        Some(available) => {
            return blocked(
                item,
                "archiveInsufficientSpace",
                format!("archive and roundtrip need at least {needed} bytes; {available} are proven available"),
            )
        }
        None => {
            return blocked(
                item,
                "archiveSpaceUnknown",
                "free space could not be proven from the bound destination volume",
            )
        }
    }
    let scratch = match BoundScratchRoot::open(&destination) {
        Ok(value) => value,
        Err(error) => return blocked(item, "archiveDestinationUnsafe", error),
    };

    let layout =
        match load_persisted_archive_layout(conn, &item, nonce, capability_index, &destination) {
            Ok(value) => value,
            Err(error) => return blocked(item, "journalInvalid", error),
        };

    let archive = if let Some(archive_id) = item.row.archive_id {
        let path = match item.row.archive_path.as_deref() {
            Some(value) if valid_bounded_local_path_text(value) => PathBuf::from(value),
            _ => return blocked(item, "archiveCorrupt", "archive path is missing or unsafe"),
        };
        let archive_stamp = match archive_stamp_from_row(&item.row) {
            Ok(value) => value,
            Err(error) => return blocked(item, "archiveCorrupt", error),
        };
        let archive_hash = match item.row.archive_blake3.as_deref() {
            Some(value) if is_hex_64(value) => value,
            _ => return blocked(item, "archiveCorrupt", "archive digest is missing"),
        };
        let committed =
            match CommittedObjectArchive::open_existing(&path, &archive_stamp, archive_hash) {
                Ok(value) => value,
                Err(error) => return blocked(item, "archiveCorrupt", error),
            };
        if layout.partial_path.is_some()
            || layout.final_path != path
            || layout.phase != "archive_verify_intent"
        {
            return blocked(
                item,
                "journalInvalid",
                "persisted existing-archive layout does not match its authority row",
            );
        }
        ArchiveBinding::Existing {
            archive: committed,
            archive_id,
            raw_backup_blake3: item.row.raw_backup_blake3.clone().unwrap_or_default(),
            semantic_blake3: item.row.proof_semantic.clone().unwrap_or_default(),
            stream_count: item.row.stream_count.unwrap_or(0),
        }
    } else {
        let final_path = destination
            .join(".codehangar-object-archive-v2")
            .join(format!("entry-{:016x}.chobj", item.row.entry_id));
        let partial_path = archive_path_for_capability(&destination, nonce, capability_index);
        if layout.partial_path.as_deref() != Some(partial_path.as_path())
            || layout.final_path != final_path
            || layout.phase != "archive_path_intent"
        {
            return blocked(
                item,
                "journalInvalid",
                "persisted new-archive layout differs from nonce/index derivation",
            );
        }
        if let Err(error) = crate::bound_fs::validate_local_mutation_path(&partial_path) {
            return blocked(item, "archiveDestinationUnsafe", error);
        }
        if let Err(error) = crate::bound_fs::validate_local_mutation_path(&final_path) {
            return blocked(item, "archiveDestinationUnsafe", error);
        }
        let container = match ObjectArchiveContainer::create_new(&partial_path) {
            Ok(value) => value,
            Err(error) => return blocked(item, "archiveCreateFailed", error),
        };
        if let Err(error) = persist_archive_initial_stamp(conn, &item, &container, &layout) {
            // `container` is still armed delete-on-close here. Returning drops
            // it and cannot strand an unjournaled partial file.
            return blocked(item, "journalInvalid", error);
        }
        ArchiveBinding::New {
            container,
            final_path,
        }
    };
    let allow_existing_archive_directory_time_drift =
        held.is_directory() && matches!(&archive, ArchiveBinding::Existing { .. });
    #[cfg(windows)]
    let held = match held.release_ancestors_for_helper() {
        Ok(value) => value,
        Err(error) => return blocked(item, "identityChanged", error),
    };
    // The containment proof is needed while binding the exact source,
    // scratch-root and archive handles. Those handles now carry the authority
    // across the helper exchange. Retaining the source ancestor chain for the
    // whole chunk would make a child block acquisition of its parent target.
    drop(containment);
    Ok(PreparedBatchItem {
        journal: item,
        held: Some(held),
        scratch,
        archive: Some(archive),
        allow_existing_archive_directory_time_drift,
    })
}

fn archive_stamp_from_row(row: &PreviewRow) -> Result<FileStamp, FinalRemoveError> {
    Ok(FileStamp {
        volume_id: row
            .archive_volume_id
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                FinalRemoveError::InvalidPreview("archive volume id is missing".into())
            })?,
        file_id: row
            .archive_file_id
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FinalRemoveError::InvalidPreview("archive file id is missing".into()))?,
        bytes: row
            .archive_bytes
            .filter(|value| *value > 0)
            .ok_or_else(|| FinalRemoveError::InvalidPreview("archive size is missing".into()))?,
        modified_unix_seconds: Some(row.archive_modified_unix_seconds.ok_or_else(|| {
            FinalRemoveError::InvalidPreview("archive modification time is missing".into())
        })?),
    })
}

fn capability_for_prepared(
    item: &PreparedBatchItem,
    nonce: &str,
    index: u32,
) -> Result<ElevatedCapability, FinalRemoveError> {
    let held = item.held.as_ref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("held proof handle is unavailable".to_string())
    })?;
    let source = ExpectedObject {
        path: PathBuf::from(&item.journal.row.held_path),
        parent_handle_value: held.raw_handle_value(),
        stamp: held.authority_stamp().clone(),
        content_blake3: held.content_hash().to_string(),
        semantic_blake3: match item.archive.as_ref() {
            Some(ArchiveBinding::Existing {
                semantic_blake3, ..
            }) => Some(semantic_blake3.clone()),
            _ => None,
        },
        allow_internal_directory_time_drift: item.allow_existing_archive_directory_time_drift,
    };
    let scratch_root = ExpectedScratchRoot {
        path: item.scratch.path().to_path_buf(),
        parent_handle_value: item.scratch.raw_handle_value(),
        stamp: item.scratch.stamp().clone(),
    };
    let scratch_leaf = scratch_leaf_for_capability(nonce, index);
    match item.archive.as_ref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("archive proof handle is unavailable".to_string())
    })? {
        ArchiveBinding::New { container, .. } => Ok(ElevatedCapability::ObjectBackupV2 {
            source,
            parent_archive_handle_value: container.raw_handle_value(),
            archive_path: container.path().to_path_buf(),
            scratch_root,
            scratch_leaf,
        }),
        ArchiveBinding::Existing { archive, .. } => Ok(ElevatedCapability::RoundtripVerify {
            source,
            parent_archive_handle_value: archive.raw_handle_value(),
            archive_path: archive.path().to_path_buf(),
            expected_archive_stamp: archive.stamp().clone(),
            expected_archive_blake3: archive.hash().to_string(),
            scratch_root,
            scratch_leaf,
        }),
    }
}

fn journal_scratch_cleanup_pending(
    conn: &Connection,
    item: &JournaledBatchItem,
    scratch_root: &Path,
    transport_nonce: &str,
    capability_index: u32,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let scratch_path = scratch_root.join(scratch_leaf_for_capability(
        transport_nonce,
        capability_index,
    ));
    crate::bound_fs::validate_local_mutation_path(&scratch_path)?;
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let authority: i64 = transaction.query_row(
        "SELECT COUNT(*)
         FROM permanent_delete_batch_item bi
         JOIN elevation_capability ec ON ec.id = bi.elevation_capability_id
         WHERE bi.id = ?1 AND bi.batch_id = ?2
           AND bi.capability_index = ?3 AND ec.operation_id = ?4
           AND ec.transport_nonce = ?5
           AND ec.nonce_digest = ?6",
        params![
            item.batch_item_id,
            item.batch_id,
            capability_index as i64,
            item.batch_operation_id,
            transport_nonce,
            blake3::hash(transport_nonce.as_bytes())
                .to_hex()
                .to_string(),
        ],
        |row| row.get(0),
    )?;
    if authority != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "scratch cleanup report has no persisted nonce/index authority".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO archive_cleanup(
            archive_id, operation_id, scratch_path, expected_volume_id,
            expected_file_id, status, created_at, error
         ) VALUES(?1, ?2, ?3, NULL, NULL,
                  'pending_identity_proof', ?4, ?5)",
        params![
            item.row.archive_id,
            item.batch_operation_id,
            scratch_path.to_string_lossy(),
            now,
            message,
        ],
    )?;
    transaction.execute(
        "INSERT INTO object_archive_event(
            archive_id, operation_id, kind, status, reason_code, message, created_at
         ) VALUES(?1, ?2, 'scratch_cleanup_pending', 'pending',
                  'scratchCleanupPending', ?3, ?4)",
        params![
            item.row.archive_id,
            item.batch_operation_id,
            serde_json::json!({
                "schema": "object_archive_scratch_cleanup/1",
                "batchId": item.batch_id,
                "batchItemId": item.batch_item_id,
                "capabilityIndex": capability_index,
                "scratchPath": scratch_path.to_string_lossy(),
                "identityKnown": false,
                "message": message,
            })
            .to_string(),
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn helper_reason_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("cancel") {
        "uacCancelled"
    } else if lower.contains("identity") || lower.contains("signature") {
        "helperIdentityInvalid"
    } else if lower.contains("unsupported") {
        "helperUnsupported"
    } else if lower.contains("timeout") {
        "helperTimeout"
    } else {
        "helperFailed"
    }
}

fn interruption_details(
    reason: FinalRemoveInterruptionReason,
) -> (&'static str, &'static str, &'static str) {
    match reason {
        FinalRemoveInterruptionReason::OwnerStop => (
            "stopRequested",
            "Stop requested by the caller; every undisposed topology group was preserved.",
            "cancelled",
        ),
        FinalRemoveInterruptionReason::ProgressObserverFailed => (
            "progressObserverFailed",
            "The internal progress observer failed; every undisposed topology group was preserved and the batch requires interruption review.",
            "failed",
        ),
    }
}

fn mark_batch_item_blocked(
    conn: &Connection,
    item: &JournaledBatchItem,
    code: &str,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = CASE
               WHEN ?2 = 'scratchCleanupPending'
               THEN 'scratch_cleanup_pending'
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archive_recovery_pending' ELSE 'blocked' END,
             status = CASE
               WHEN ?2 = 'scratchCleanupPending' THEN 'interrupted'
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'interrupted' ELSE 'blocked' END,
             reason_code = CASE
               WHEN ?2 = 'scratchCleanupPending' THEN ?2
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archivePromotionPending' ELSE ?2 END,
             message = ?3, updated_at = ?4
         WHERE id = ?1 AND status IN ('planned', 'ready')",
        params![item.batch_item_id, code, message, now],
    )?;
    transaction.execute(
        "UPDATE operation_item SET status = 'skipped'
         WHERE id = ?1 AND status = 'pending'
           AND EXISTS (
             SELECT 1 FROM permanent_delete_batch_item
             WHERE operation_item_id = ?1 AND status = 'blocked'
           )",
        [item.operation_item_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn terminalize_undisposed_batch_items(
    conn: &Connection,
    batch_id: i64,
    code: &str,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archive_recovery_pending' ELSE 'blocked' END,
             status = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'interrupted' ELSE 'blocked' END,
             reason_code = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archivePromotionPending' ELSE ?2 END,
             message = ?3, updated_at = ?4
         WHERE batch_id = ?1 AND status IN ('planned', 'ready')",
        params![batch_id, code, message, now],
    )?;
    transaction.execute(
        "UPDATE operation_item
         SET status = 'skipped'
         WHERE status = 'pending' AND id IN (
             SELECT operation_item_id FROM permanent_delete_batch_item
             WHERE batch_id = ?1 AND status = 'blocked'
         )",
        [batch_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn batch_has_started_disposition(
    conn: &Connection,
    batch_id: i64,
) -> Result<bool, FinalRemoveError> {
    Ok(conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM permanent_delete_batch_item
            WHERE batch_id = ?1 AND status IN ('deleted', 'deleting', 'interrupted')
         )",
        [batch_id],
        |row| row.get(0),
    )?)
}

fn finish_batch_without_delete(
    conn: &Connection,
    operation_id: i64,
    batch_id: i64,
    elevation_id: i64,
    status: &str,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archive_recovery_pending' ELSE 'blocked' END,
             status = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'interrupted' ELSE 'blocked' END,
             reason_code = CASE
               WHEN archive_id IS NULL AND archive_proof_blake3 IS NOT NULL
               THEN 'archivePromotionPending'
               ELSE COALESCE(reason_code, 'helperFailed') END,
             message = COALESCE(message, ?2), updated_at = ?3
         WHERE batch_id = ?1 AND status IN ('planned', 'ready')",
        params![batch_id, message, now],
    )?;
    transaction.execute(
        "UPDATE operation_item SET status = 'skipped'
         WHERE operation_id = ?1 AND status = 'pending'
           AND id IN (
             SELECT operation_item_id FROM permanent_delete_batch_item
             WHERE batch_id = ?2 AND status = 'blocked'
           )",
        params![operation_id, batch_id],
    )?;
    let blocked: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM permanent_delete_batch_item
         WHERE batch_id = ?1 AND status = 'blocked'",
        [batch_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "UPDATE permanent_delete_batch
         SET status = ?2, blocked_count = ?3, finished_at = ?4, error = ?5
         WHERE id = ?1",
        params![batch_id, status, blocked, now, message],
    )?;
    transaction.execute(
        "UPDATE operation SET status = ?2, finished_at = ?3, error = ?4
         WHERE id = ?1",
        params![operation_id, status, now, message],
    )?;
    transaction.execute(
        "UPDATE elevation_capability
         SET status = 'failed', finished_at = ?2, error = ?3
         WHERE id = ?1 AND status = 'issued'",
        params![elevation_id, now, message],
    )?;
    transaction.commit()?;
    Ok(())
}

fn proof_field<'a>(value: &'a Option<String>, label: &str) -> Result<&'a str, FinalRemoveError> {
    value
        .as_deref()
        .filter(|value| is_hex_64(value))
        .ok_or_else(|| FinalRemoveError::InvalidPreview(format!("helper omitted {label}")))
}

fn persist_archive_promotion_proof(
    conn: &Connection,
    item: &PreparedBatchItem,
    proof: &ElevatedObjectResult,
) -> Result<(), FinalRemoveError> {
    let stamp = proof.archive_stamp.as_ref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("helper omitted archive identity".to_string())
    })?;
    let archive_hash = proof_field(&proof.archive_blake3, "archive digest")?;
    let raw_hash = proof_field(&proof.raw_backup_blake3, "raw stream digest")?;
    let semantic = proof_field(&proof.semantic_blake3, "semantic digest")?;
    let roundtrip = proof_field(&proof.roundtrip_blake3, "roundtrip digest")?;
    let stream_count = proof.stream_count.ok_or_else(|| {
        FinalRemoveError::InvalidPreview("helper omitted archive stream count".to_string())
    })?;
    if stamp.volume_id.is_empty()
        || stamp.file_id.is_empty()
        || stamp.bytes == 0
        || stamp.modified_unix_seconds.is_none()
        || semantic != roundtrip
    {
        return Err(FinalRemoveError::InvalidPreview(
            "helper archive proof is incomplete or internally inconsistent".to_string(),
        ));
    }
    let (expected_phase, event_kind, initial_stamp, final_path) = match item.archive.as_ref() {
        Some(ArchiveBinding::New {
            container,
            final_path,
        }) => {
            if !container.initial_stamp().same_object(stamp) {
                return Err(FinalRemoveError::InvalidPreview(
                    "helper proof does not identify the journaled CREATE_NEW archive".to_string(),
                ));
            }
            (
                "archive_container_bound",
                "promotion_proof",
                Some(container.initial_stamp().clone()),
                final_path.as_path(),
            )
        }
        Some(ArchiveBinding::Existing { archive, .. }) => {
            if archive.stamp() != stamp || archive.hash() != archive_hash {
                return Err(FinalRemoveError::InvalidPreview(
                    "helper proof does not identify the persisted archive authority".to_string(),
                ));
            }
            (
                "archive_verify_intent",
                "reverify_proof",
                None,
                archive.path(),
            )
        }
        None => {
            return Err(FinalRemoveError::InvalidPreview(
                "archive binding disappeared before proof journaling".to_string(),
            ))
        }
    };
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET archive_proof_volume_id = ?2, archive_proof_file_id = ?3,
             archive_proof_bytes = ?4,
             archive_proof_modified_unix_seconds = ?5,
             archive_proof_blake3 = ?6, archive_raw_backup_blake3 = ?7,
             archive_semantic_blake3 = ?8, archive_roundtrip_blake3 = ?9,
             archive_stream_count = ?10, archive_security_stream_present = ?11,
             archive_cleanup_complete = ?12, archive_proof_schema = ?13,
             phase = 'archive_proof_persisted', updated_at = ?14
         WHERE id = ?1 AND batch_id = ?15 AND status = 'planned'
           AND phase = ?16 AND archive_id IS NULL
           AND archive_final_path = ?17
           AND (
             ?18 = 0 OR (
               archive_initial_volume_id = ?19
               AND archive_initial_file_id = ?20
               AND archive_initial_bytes = ?21
               AND archive_initial_modified_unix_seconds IS ?22
             )
           )",
        params![
            item.journal.batch_item_id,
            stamp.volume_id,
            stamp.file_id,
            stamp.bytes as i64,
            stamp.modified_unix_seconds,
            archive_hash,
            raw_hash,
            semantic,
            roundtrip,
            stream_count as i64,
            i64::from(proof.security_stream_present),
            i64::from(proof.cleanup_complete),
            proof.proof_schema,
            now,
            item.journal.batch_id,
            expected_phase,
            final_path.to_string_lossy(),
            i64::from(initial_stamp.is_some()),
            initial_stamp.as_ref().map(|stamp| stamp.volume_id.as_str()),
            initial_stamp.as_ref().map(|stamp| stamp.file_id.as_str()),
            initial_stamp.as_ref().map(|stamp| stamp.bytes as i64),
            initial_stamp
                .as_ref()
                .and_then(|stamp| stamp.modified_unix_seconds),
        ],
    )?;
    if updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "helper archive proof could not be durably claimed before promotion".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO object_archive_event(
            archive_id, operation_id, kind, status, message, created_at
         ) VALUES(?1, ?2, ?3, 'pending', ?4, ?5)",
        params![
            item.journal.row.archive_id,
            item.journal.batch_operation_id,
            event_kind,
            serde_json::json!({
                "schema": "object_archive_promotion_proof/1",
                "batchItemId": item.journal.batch_item_id,
                "archiveVolumeId": stamp.volume_id,
                "archiveFileId": stamp.file_id,
                "archiveBytes": stamp.bytes,
                "archiveModifiedUnixSeconds": stamp.modified_unix_seconds,
                "archiveBlake3": archive_hash,
                "rawBackupBlake3": raw_hash,
                "semanticBlake3": semantic,
                "roundtripBlake3": roundtrip,
                "streamCount": stream_count,
                "securityStreamPresent": proof.security_stream_present,
                "cleanupComplete": proof.cleanup_complete,
                "proofSchema": proof.proof_schema,
            })
            .to_string(),
            now,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn commit_archive_proof(
    conn: &Connection,
    item: &mut PreparedBatchItem,
    proof: &ElevatedObjectResult,
) -> Result<(), FinalRemoveError> {
    if proof.proof_schema != OBJECT_ARCHIVE_PROOF_SCHEMA
        || !proof.security_stream_present
        || !proof.cleanup_complete
        || proof
            .stream_count
            .is_none_or(|value| value == 0 || value > 4_096)
    {
        return Err(FinalRemoveError::InvalidPreview(
            "helper result is not object_archive/2-complete".to_string(),
        ));
    }
    let expected = row_stamp(&item.journal.row)?;
    let live_expected = item
        .held
        .as_ref()
        .ok_or_else(|| {
            FinalRemoveError::InvalidPreview(
                "held proof handle disappeared before archive proof commit".to_string(),
            )
        })?
        .stamp();
    if proof.source_before != *live_expected || proof.source_after != *live_expected {
        return Err(FinalRemoveError::InvalidPreview(
            "held object changed during elevated archive verification".to_string(),
        ));
    }
    let archive_hash = proof_field(&proof.archive_blake3, "archive digest")?;
    let raw_hash = proof_field(&proof.raw_backup_blake3, "raw stream digest")?;
    let semantic = proof_field(&proof.semantic_blake3, "semantic digest")?;
    let roundtrip = proof_field(&proof.roundtrip_blake3, "roundtrip digest")?;
    if semantic != roundtrip {
        return Err(FinalRemoveError::InvalidPreview(
            "archive semantic and roundtrip digests differ".to_string(),
        ));
    }
    let helper_archive_stamp = proof.archive_stamp.as_ref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("helper omitted archive identity".to_string())
    })?;
    // This transaction is the durable recovery authority for the narrow
    // delete-on-close cancellation -> exact-handle rename boundary inside
    // `verify_and_commit`. It MUST commit before that method is called.
    persist_archive_promotion_proof(conn, item, proof)?;
    let binding = item.archive.take().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("archive handle was already consumed".to_string())
    })?;
    let (committed, archive_id) = match binding {
        ArchiveBinding::New {
            container,
            final_path,
        } => {
            let committed =
                container.verify_and_commit(&final_path, helper_archive_stamp, archive_hash)?;
            let now = Utc::now().to_rfc3339();
            let backup_id = item.journal.row.backup_id.ok_or_else(|| {
                FinalRemoveError::InvalidPreview("archive backup id is missing".to_string())
            })?;
            let held_hash = item.journal.row.result_blake3.as_deref().ok_or_else(|| {
                FinalRemoveError::InvalidPreview("held content hash is missing".to_string())
            })?;
            let transaction = conn.unchecked_transaction()?;
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
                    item.journal.row.entry_id,
                    item.journal.project_group_id,
                    item.journal.row.original_path,
                    item.journal.row.held_path,
                    expected.volume_id,
                    expected.file_id,
                    expected.bytes as i64,
                    expected.modified_unix_seconds,
                    held_hash,
                    committed.path().to_string_lossy(),
                    committed.stamp().volume_id,
                    committed.stamp().file_id,
                    committed.stamp().bytes as i64,
                    committed.stamp().modified_unix_seconds,
                    committed.hash(),
                    raw_hash,
                    semantic,
                    roundtrip,
                    proof.stream_count.unwrap_or(0) as i64,
                    OBJECT_ARCHIVE_PROOF_SCHEMA,
                    now,
                ],
            )?;
            let archive_id = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO object_archive_event(
                    archive_id, operation_id, kind, status, created_at
                 ) VALUES(?1, ?2, 'roundtrip_verified', 'done', ?3)",
                params![archive_id, item.journal.batch_operation_id, now],
            )?;
            let item_updated = transaction.execute(
                "UPDATE permanent_delete_batch_item
                 SET archive_id = ?2, phase = 'archive_ready', status = 'ready',
                     updated_at = ?3
                 WHERE id = ?1 AND status = 'planned'
                   AND phase = 'archive_proof_persisted'",
                params![item.journal.batch_item_id, archive_id, now],
            )?;
            if item_updated != 1 {
                return Err(FinalRemoveError::InvalidPreview(
                    "promoted archive lost its batch-item proof CAS".to_string(),
                ));
            }
            transaction.commit()?;
            (committed, archive_id)
        }
        ArchiveBinding::Existing {
            archive,
            archive_id,
            raw_backup_blake3,
            semantic_blake3,
            stream_count,
        } => {
            if archive.stamp() != helper_archive_stamp
                || archive.hash() != archive_hash
                || raw_backup_blake3 != raw_hash
                || semantic_blake3 != semantic
                || stream_count != proof.stream_count.unwrap_or(0)
            {
                return Err(FinalRemoveError::InvalidPreview(
                    "existing archive helper result differs from its persisted proof".to_string(),
                ));
            }
            let now = Utc::now().to_rfc3339();
            let transaction = conn.unchecked_transaction()?;
            let updated = transaction.execute(
                "UPDATE object_archive
                 SET verified_at = ?2, status = 'ready', blocked_reason = NULL,
                     security_stream_present = 1, cleanup_complete = 1
                 WHERE id = ?1 AND quarantine_entry_id = ?3
                   AND proof_schema = ?4 AND archive_blake3 = ?5
                   AND semantic_blake3 = ?6 AND roundtrip_blake3 = ?6",
                params![
                    archive_id,
                    now,
                    item.journal.row.entry_id,
                    OBJECT_ARCHIVE_PROOF_SCHEMA,
                    archive_hash,
                    semantic,
                ],
            )?;
            if updated != 1 {
                return Err(FinalRemoveError::InvalidPreview(
                    "existing archive authority row changed before verification commit".into(),
                ));
            }
            transaction.execute(
                "INSERT INTO object_archive_event(
                    archive_id, operation_id, kind, status, created_at
                 ) VALUES(?1, ?2, 'roundtrip_reverified', 'done', ?3)",
                params![archive_id, item.journal.batch_operation_id, now],
            )?;
            let item_updated = transaction.execute(
                "UPDATE permanent_delete_batch_item
                 SET archive_id = ?2, phase = 'archive_ready', status = 'ready',
                     updated_at = ?3
                 WHERE id = ?1 AND status = 'planned'
                   AND phase = 'archive_proof_persisted'",
                params![item.journal.batch_item_id, archive_id, now],
            )?;
            if item_updated != 1 {
                return Err(FinalRemoveError::InvalidPreview(
                    "reverified archive lost its batch-item proof CAS".to_string(),
                ));
            }
            transaction.commit()?;
            (archive, archive_id)
        }
    };
    item.archive = Some(ArchiveBinding::Existing {
        archive: committed,
        archive_id,
        raw_backup_blake3: raw_hash.to_string(),
        semantic_blake3: semantic.to_string(),
        stream_count: proof.stream_count.unwrap_or(0),
    });
    Ok(())
}

struct BoundDeleteProof {
    journal: JournaledBatchItem,
    held: BoundObjectProof,
    archive: CommittedObjectArchive,
}

fn take_helper_verified_delete_proof(
    item: &mut PreparedBatchItem,
) -> Result<BoundDeleteProof, FinalRemoveError> {
    let held = item.held.take().ok_or_else(|| {
        FinalRemoveError::InvalidPreview(
            "helper-verified held handle is unavailable for disposition".to_string(),
        )
    })?;
    // The helper-verified source handle is share-compatible and ancestor-free.
    // It is retained until its topology group is ready; the final share-zero
    // rebind then uses the durable archive/journal authority bottom-up.
    let binding = item.archive.take().ok_or_else(|| {
        FinalRemoveError::InvalidPreview(
            "helper-verified archive handle is unavailable for disposition".to_string(),
        )
    })?;
    let archive = match binding {
        ArchiveBinding::Existing { archive, .. } => archive,
        ArchiveBinding::New { .. } => {
            return Err(FinalRemoveError::InvalidPreview(
                "uncommitted archive container cannot authorize disposition".to_string(),
            ))
        }
    };
    Ok(BoundDeleteProof {
        journal: item.journal.clone(),
        held,
        archive,
    })
}

fn validate_bound_delete_proof_authority(
    conn: &Connection,
    proof: &BoundDeleteProof,
) -> Result<(), FinalRemoveError> {
    let held_stamp = proof.held.authority_stamp();
    let held_hash = proof.held.content_hash();
    let authority = conn
        .query_row(
            "SELECT oa.archive_path, oa.archive_volume_id, oa.archive_file_id,
                    oa.archive_bytes, oa.archive_modified_unix_seconds,
                    oa.archive_blake3
             FROM permanent_delete_batch_item bi
             JOIN object_archive oa ON oa.id = bi.archive_id
             WHERE bi.id = ?1 AND bi.batch_id = ?2 AND bi.status = 'ready'
               AND oa.quarantine_entry_id = bi.quarantine_entry_id
               AND oa.status = 'ready' AND oa.proof_schema = ?3
               AND oa.security_stream_present = 1 AND oa.cleanup_complete = 1
               AND oa.semantic_blake3 = oa.roundtrip_blake3
               AND oa.held_volume_id = ?4 AND oa.held_file_id = ?5
               AND oa.held_bytes = ?6 AND oa.held_modified_unix_seconds = ?7
               AND oa.held_content_blake3 = ?8",
            params![
                proof.journal.batch_item_id,
                proof.journal.batch_id,
                OBJECT_ARCHIVE_PROOF_SCHEMA,
                held_stamp.volume_id,
                held_stamp.file_id,
                held_stamp.bytes as i64,
                held_stamp.modified_unix_seconds,
                held_hash,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    FileStamp {
                        volume_id: row.get(1)?,
                        file_id: row.get(2)?,
                        bytes: row.get::<_, i64>(3)?.max(0) as u64,
                        modified_unix_seconds: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            FinalRemoveError::InvalidPreview(
                "helper-verified handles no longer match object_archive/2 journal authority"
                    .to_string(),
            )
        })?;
    if !valid_bounded_local_path_text(&authority.0)
        || proof.archive.path() != Path::new(&authority.0)
        || proof.archive.stamp() != &authority.1
        || proof.archive.hash() != authority.2
    {
        return Err(FinalRemoveError::InvalidPreview(
            "helper-verified archive handle differs from durable authority".to_string(),
        ));
    }
    Ok(())
}

fn bind_ready_delete_proof(
    conn: &Connection,
    item: &JournaledBatchItem,
) -> Result<BoundDeleteProof, FinalRemoveError> {
    let authority = conn
        .query_row(
            "SELECT archive_path, archive_volume_id, archive_file_id,
                    archive_bytes, archive_modified_unix_seconds, archive_blake3
             FROM object_archive
             WHERE quarantine_entry_id = ?1 AND status = 'ready'
               AND proof_schema = ?2 AND security_stream_present = 1
               AND cleanup_complete = 1
               AND held_volume_id = ?3 AND held_file_id = ?4
               AND held_bytes = ?5 AND held_content_blake3 = ?6
               AND held_modified_unix_seconds = ?7
               AND semantic_blake3 = roundtrip_blake3",
            params![
                item.row.entry_id,
                OBJECT_ARCHIVE_PROOF_SCHEMA,
                item.row.result_volume_id,
                item.row.result_file_id,
                item.row.result_bytes.map(|value| value as i64),
                item.row.result_blake3,
                item.row.result_modified_unix_seconds,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    FileStamp {
                        volume_id: row.get(1)?,
                        file_id: row.get(2)?,
                        bytes: row.get::<_, i64>(3)?.max(0) as u64,
                        modified_unix_seconds: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            FinalRemoveError::InvalidPreview(
                "object_archive/2 authority changed before parent disposition".to_string(),
            )
        })?;
    let archive_path = PathBuf::from(&authority.0);
    if !valid_bounded_local_path_text(&authority.0) || !is_hex_64(&authority.2) {
        return Err(FinalRemoveError::InvalidPreview(
            "object archive authority has an unsafe path or digest".to_string(),
        ));
    }
    let archive = CommittedObjectArchive::open_existing(&archive_path, &authority.1, &authority.2)?;
    let stamp = row_stamp(&item.row)?;
    let content_hash = item.row.result_blake3.as_deref().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("held content hash is missing".to_string())
    })?;
    let exact_held = BoundObjectProof::open_for_archive_delete(
        Path::new(&item.row.held_path),
        &stamp,
        content_hash,
    );
    let held = match exact_held {
        Ok(value) => value,
        Err(exact_error) => {
            #[cfg(windows)]
            {
                if item.row.archive_id.is_some() && planned_descendants_were_deleted(conn, item)? {
                    BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
                        Path::new(&item.row.held_path),
                        &stamp,
                        content_hash,
                    )?
                } else {
                    return Err(exact_error.into());
                }
            }
            #[cfg(not(windows))]
            {
                return Err(exact_error.into());
            }
        }
    };
    Ok(BoundDeleteProof {
        journal: item.clone(),
        held,
        archive,
    })
}

#[cfg(any(not(windows), test))]
#[allow(dead_code)] // journal-only tests call the projection helper directly
fn claim_delete_group_intent(
    conn: &Connection,
    items: &[BoundDeleteProof],
) -> Result<(), FinalRemoveError> {
    let journals = items.iter().map(|item| &item.journal).collect::<Vec<_>>();
    claim_delete_group_intent_journals(conn, &journals)
}

#[cfg(any(not(windows), test))]
fn claim_delete_group_intent_journals(
    conn: &Connection,
    items: &[&JournaledBatchItem],
) -> Result<(), FinalRemoveError> {
    let first = items.first().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("empty topology group cannot be disposed".to_string())
    })?;
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch SET status = 'parent_disposition' WHERE id = ?1",
        [first.batch_id],
    )?;
    transaction.execute(
        "UPDATE operation SET status = 'parent_disposition' WHERE id = ?1",
        [first.batch_operation_id],
    )?;
    for item in items {
        let claimed = transaction.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'parent_disposition', status = 'deleting',
                 reason_code = ?4,
                 message = 'The same-handle final disposition profile is not durably proved yet.',
                 updated_at = ?2
             WHERE id = ?1 AND batch_id = ?3 AND status = 'ready'
               AND archive_id IS NOT NULL",
            params![
                item.batch_item_id,
                now,
                item.batch_id,
                UNPROVED_FINAL_PROFILE_REASON_CODE,
            ],
        )?;
        let entry_claimed = transaction.execute(
            "UPDATE quarantine_entry SET status = 'deleting'
             WHERE id = ?1 AND status = 'quarantined'
               AND quarantine_path = ?2 AND backup_id = ?3",
            params![item.row.entry_id, item.row.held_path, item.row.backup_id,],
        )?;
        let operation_claimed = transaction.execute(
            "UPDATE operation_item SET status = 'deleting'
             WHERE id = ?1 AND status = 'pending'",
            [item.operation_item_id],
        )?;
        if claimed != 1 || entry_claimed != 1 || operation_claimed != 1 {
            return Err(FinalRemoveError::InvalidPreview(
                "topology delete-intent CAS lost authority before filesystem disposition"
                    .to_string(),
            ));
        }
    }
    transaction.commit()?;
    Ok(())
}

fn delete_ready_topology_group(
    conn: &Connection,
    journals: &[JournaledBatchItem],
    helper_path: &Path,
) -> Result<(), FinalRemoveError> {
    let mut proofs = Vec::with_capacity(journals.len());
    for item in journals {
        proofs.push(bind_ready_delete_proof(conn, item)?);
    }
    delete_bound_topology_group(conn, proofs, helper_path)
}

#[cfg(windows)]
fn windows_path_is_strict_descendant(candidate: &Path, parent: &Path) -> bool {
    let candidate = candidate
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let parent = parent
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    candidate.len() > parent.len()
        && parent
            .iter()
            .zip(&candidate)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

/// Prove that every descendant committed to this immutable batch has already
/// reached durable deletion before allowing the containing directory's mtime
/// to differ from its original archive authority. Unknown children are proved
/// separately from the bound directory immediately before the final rebind.
#[cfg(windows)]
fn planned_descendants_were_deleted(
    conn: &Connection,
    item: &JournaledBatchItem,
) -> Result<bool, FinalRemoveError> {
    let parent = Path::new(&item.row.held_path);
    let mut statement = conn.prepare(
        "SELECT held_path, status FROM permanent_delete_batch_item
         WHERE batch_id = ?1 AND id != ?2",
    )?;
    let rows = statement
        .query_map(params![item.batch_id, item.batch_item_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut descendants = 0usize;
    for (path, status) in rows {
        if !windows_path_is_strict_descendant(Path::new(&path), parent) {
            continue;
        }
        descendants += 1;
        if status != "deleted" {
            return Err(FinalRemoveError::InvalidPreview(format!(
                "directory {} cannot accept internal mtime drift while planned descendant {} is {status}",
                parent.display(),
                path
            )));
        }
    }
    Ok(descendants > 0)
}

fn delete_bound_topology_group(
    conn: &Connection,
    mut proofs: Vec<BoundDeleteProof>,
    helper_path: &Path,
) -> Result<(), FinalRemoveError> {
    for proof in &proofs {
        validate_bound_delete_proof_authority(conn, proof)?;
    }
    // `object_archive.stream_count` includes BackupRead descriptors such as
    // security/EA metadata and is therefore not a final-disposition stream
    // profile.  BoundObjectProof's count is captured from FileStreamInfo on the
    // exact held-object handle.  Reject a named/non-default stream while every
    // item is still merely `ready`: no delete-pending bit has been armed and a
    // supported peer topology group can continue as a normal partial result.
    // A stream/link race after this precheck remains covered by independent
    // same-handle post-arm validation in both parent and guardian.
    if proofs
        .iter()
        .any(|proof| !bound_object_matches_v013_final_stream_profile(&proof.held))
    {
        let journals = proofs
            .iter()
            .map(|proof| &proof.journal)
            .collect::<Vec<_>>();
        block_ready_topology_group(
            conn,
            &journals,
            UNSUPPORTED_FINAL_STREAM_PROFILE_REASON_CODE,
            UNSUPPORTED_FINAL_STREAM_PROFILE_MESSAGE,
        )?;
        return Ok(());
    }
    #[cfg(windows)]
    {
        // All earlier child groups are terminal. Revalidate each exact
        // share-zero target without reopening its pathname. Direct chunks and
        // groups rebound after a split use the same final primitive.
        for proof in &mut proofs {
            if proof.held.is_directory() && planned_descendants_were_deleted(conn, &proof.journal)?
            {
                proof.held.authorize_internal_directory_time_drift()?;
            }
        }
        let mut detached = Vec::with_capacity(proofs.len());
        for mut proof in proofs {
            proof.held = proof.held.detach_exclusive_for_final_disposition()?;
            validate_bound_delete_proof_authority(conn, &proof)?;
            detached.push(proof);
        }
        delete_bound_topology_group_guarded(conn, detached, helper_path)
    }
    #[cfg(not(windows))]
    {
        let _ = helper_path;
        claim_delete_group_intent(conn, &proofs)?;
        for proof in proofs {
            let journal = proof.journal;
            match proof.held.delete_exact() {
                Ok(()) => record_delete_success(
                    conn,
                    journal.batch_operation_id,
                    journal.batch_id,
                    &journal,
                )?,
                Err(error) => {
                    record_delete_failure(
                        conn,
                        journal.batch_operation_id,
                        &journal,
                        &error.to_string(),
                    )?;
                    return Err(FinalRemoveError::InvalidPreview(format!(
                        "exact-handle disposition became ambiguous for entry {}: {error}",
                        journal.row.entry_id
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
struct GuardedDeleteProof {
    proof: BoundDeleteProof,
    guardian: DispositionGuardian,
}

#[cfg(windows)]
fn delete_bound_topology_group_guarded(
    conn: &Connection,
    proofs: Vec<BoundDeleteProof>,
    helper_path: &Path,
) -> Result<(), FinalRemoveError> {
    let mut guarded = Vec::with_capacity(proofs.len());
    for proof in proofs {
        proof.held.validate_final_disposition_prearm()?;
        let guardian_nonce = secure_random_hex_256()?;
        let guardian_nonce_digest = blake3::hash(guardian_nonce.as_bytes()).to_hex().to_string();
        let operation_uuid = secure_random_hex_256()?[..32].to_string();
        let receipt_parent = proof.archive.path().parent().ok_or_else(|| {
            FinalRemoveError::InvalidPreview(
                "object archive has no parent for its guardian receipt".to_string(),
            )
        })?;
        let receipt_path = receipt_parent.join(format!(
            ".codehangar-guardian-receipt-v1-{}-{guardian_nonce_digest}.bin",
            proof.journal.batch_item_id
        ));
        let guardian = launch_disposition_guardian(DispositionGuardianLaunch {
            helper_path,
            operation_uuid: &operation_uuid,
            guardian_nonce: &guardian_nonce,
            operation_id: proof.journal.batch_operation_id,
            batch_item_id: proof.journal.batch_item_id,
            parent_handle_value: proof.held.raw_handle_value(),
            expected_stamp: proof.held.stamp(),
            receipt_path: &receipt_path,
        })?;
        guarded.push(GuardedDeleteProof { proof, guardian });
    }

    // The guardian handle binding and the ordinary delete intent become
    // durable in one transaction before either process may arm disposition.
    claim_delete_group_intent_guarded(conn, &mut guarded)?;
    for guarded_item in guarded {
        dispose_one_guarded(conn, guarded_item)?;
    }
    Ok(())
}

#[cfg(windows)]
fn claim_delete_group_intent_guarded(
    conn: &Connection,
    items: &mut [GuardedDeleteProof],
) -> Result<(), FinalRemoveError> {
    let first = items.first().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("empty topology group cannot be disposed".to_string())
    })?;
    if items.iter().any(|item| {
        item.proof.journal.batch_id != first.proof.journal.batch_id
            || item.proof.journal.batch_operation_id != first.proof.journal.batch_operation_id
            || item.proof.journal.topology_group_id != first.proof.journal.topology_group_id
    }) {
        return Err(FinalRemoveError::InvalidPreview(
            "mixed topology group cannot bind guardians atomically".to_string(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch SET status = 'parent_disposition' WHERE id = ?1",
        [first.proof.journal.batch_id],
    )?;
    transaction.execute(
        "UPDATE operation SET status = 'parent_disposition' WHERE id = ?1",
        [first.proof.journal.batch_operation_id],
    )?;
    for item in items.iter() {
        let journal = &item.proof.journal;
        let identity = item.guardian.identity();
        let guardian_started = i64::try_from(identity.process_started_100ns).map_err(|_| {
            FinalRemoveError::InvalidPreview(
                "guardian process-start identity exceeds the durable journal range".to_string(),
            )
        })?;
        let guardian_bound = transaction.execute(
            "INSERT INTO final_disposition_guardian(
                batch_item_id, operation_id, nonce_digest, guardian_pid,
                guardian_started_100ns, guardian_image_sha256,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds,
                receipt_path, receipt_volume_id, receipt_file_id,
                receipt_key_dpapi, state,
                created_at, updated_at
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, 'handle_bound', ?15, ?15
             WHERE EXISTS(
               SELECT 1 FROM permanent_delete_batch_item
               WHERE id = ?1 AND batch_id = ?16 AND status = 'ready'
                 AND archive_id IS NOT NULL
             )",
            params![
                journal.batch_item_id,
                journal.batch_operation_id,
                identity.nonce_digest,
                i64::from(identity.pid),
                guardian_started,
                identity.image_sha256,
                item.proof.held.stamp().volume_id,
                item.proof.held.stamp().file_id,
                item.proof.held.stamp().bytes as i64,
                item.proof.held.stamp().modified_unix_seconds,
                identity.receipt.path.to_string_lossy(),
                identity.receipt.initial_stamp.volume_id,
                identity.receipt.initial_stamp.file_id,
                identity.receipt.protected_key_hex,
                now,
                journal.batch_id,
            ],
        )?;
        let claimed = transaction.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'parent_disposition', status = 'deleting',
                 reason_code = ?4,
                 message = 'A separately signed guardian holds the exact handle; the final profile is not proved yet.',
                 updated_at = ?2
             WHERE id = ?1 AND batch_id = ?3 AND status = 'ready'
               AND archive_id IS NOT NULL",
            params![
                journal.batch_item_id,
                now,
                journal.batch_id,
                UNPROVED_FINAL_PROFILE_REASON_CODE,
            ],
        )?;
        let entry_claimed = transaction.execute(
            "UPDATE quarantine_entry SET status = 'deleting'
             WHERE id = ?1 AND status = 'quarantined'
               AND quarantine_path = ?2 AND backup_id = ?3",
            params![
                journal.row.entry_id,
                journal.row.held_path,
                journal.row.backup_id,
            ],
        )?;
        let operation_claimed = transaction.execute(
            "UPDATE operation_item SET status = 'deleting'
             WHERE id = ?1 AND status = 'pending'",
            [journal.operation_item_id],
        )?;
        if guardian_bound != 1 || claimed != 1 || entry_claimed != 1 || operation_claimed != 1 {
            return Err(FinalRemoveError::InvalidPreview(
                "guardian-bound topology intent CAS lost authority before disposition".to_string(),
            ));
        }
    }
    transaction.commit()?;
    for item in items.iter_mut() {
        item.guardian.mark_receipt_journaled();
    }
    Ok(())
}

#[cfg(windows)]
fn transition_guardian_state(
    conn: &Connection,
    journal: &JournaledBatchItem,
    expected_state: &str,
    next_state: &str,
    mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
    error: Option<&str>,
    profile_proved: bool,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE final_disposition_guardian
         SET state = ?3, disposition_mode = COALESCE(?4, disposition_mode),
             updated_at = ?5, error = ?6
         WHERE batch_item_id = ?1 AND operation_id = ?2 AND state = ?7",
        params![
            journal.batch_item_id,
            journal.batch_operation_id,
            next_state,
            mode.map(|value| value.journal_label()),
            now,
            error.map(bounded_guardian_journal_message),
            expected_state,
        ],
    )?;
    if updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(format!(
            "guardian durable state lost {expected_state}->{next_state} authority"
        )));
    }
    if profile_proved {
        let item_updated = transaction.execute(
            "UPDATE permanent_delete_batch_item
             SET reason_code = ?2,
                 message = 'Parent and guardian independently proved the same armed final-disposition profile; close is not authorized yet.',
                 updated_at = ?3
             WHERE id = ?1 AND status = 'deleting'
               AND reason_code = ?4",
            params![
                journal.batch_item_id,
                PROVED_FINAL_PROFILE_REASON_CODE,
                now,
                UNPROVED_FINAL_PROFILE_REASON_CODE,
            ],
        )?;
        if item_updated != 1 {
            return Err(FinalRemoveError::InvalidPreview(
                "final profile proof could not clear the unproved journal marker".to_string(),
            ));
        }
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(windows)]
fn mark_guardian_parent_handle_closed(
    conn: &Connection,
    journal: &JournaledBatchItem,
    mode: crate::bound_fs::WindowsDeleteDispositionMode,
) -> Result<(), FinalRemoveError> {
    let updated = conn.execute(
        "UPDATE final_disposition_guardian
         SET state = 'parent_handle_closed', disposition_mode = ?3,
             updated_at = ?4, error = NULL
         WHERE batch_item_id = ?1 AND operation_id = ?2
           AND state = 'guardian_handle_closed'",
        params![
            journal.batch_item_id,
            journal.batch_operation_id,
            mode.journal_label(),
            Utc::now().to_rfc3339(),
        ],
    )?;
    if updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "parent close could not advance the guardian journal".to_string(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn bounded_guardian_journal_message(message: &str) -> String {
    const MAX: usize = 512;
    if message.len() <= MAX {
        return message.to_string();
    }
    let mut boundary = MAX;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}...", &message[..boundary])
}

#[cfg(windows)]
fn dispose_one_guarded(
    conn: &Connection,
    guarded: GuardedDeleteProof,
) -> Result<(), FinalRemoveError> {
    let GuardedDeleteProof {
        proof,
        mut guardian,
    } = guarded;
    let BoundDeleteProof {
        journal,
        held,
        archive: _archive,
    } = proof;

    transition_guardian_state(
        conn,
        &journal,
        "handle_bound",
        "arm_authorized_unproved",
        None,
        None,
        false,
    )?;
    if let Err(error) = guardian.arm_authorized() {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "arm_authorized_unproved",
            None,
            format!("guardian refused arm authorization: {error}"),
        );
    }

    let mode = match held.arm_final_disposition() {
        Ok(mode) => mode,
        Err(error) => {
            return cancel_guarded_disposition(
                conn,
                &journal,
                held,
                &mut guardian,
                "arm_authorized_unproved",
                None,
                format!("parent could not prove which disposition arm completed: {error}"),
            );
        }
    };
    if let Err(error) = transition_guardian_state(
        conn,
        &journal,
        "arm_authorized_unproved",
        "armed_unproved",
        Some(mode),
        None,
        false,
    ) {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "arm_authorized_unproved",
            Some(mode),
            error.to_string(),
        );
    }

    if let Err(error) = held.validate_armed_final_disposition(mode) {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "armed_unproved",
            Some(mode),
            format!("parent post-arm proof failed: {error}"),
        );
    }
    if let Err(error) = guardian.prove_armed(mode) {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "armed_unproved",
            Some(mode),
            format!("guardian post-arm proof failed: {error}"),
        );
    }
    if let Err(error) = transition_guardian_state(
        conn,
        &journal,
        "armed_unproved",
        "final_profile_proved_held",
        Some(mode),
        None,
        true,
    ) {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "armed_unproved",
            Some(mode),
            error.to_string(),
        );
    }
    if let Err(error) = transition_guardian_state(
        conn,
        &journal,
        "final_profile_proved_held",
        "close_authorized",
        Some(mode),
        None,
        false,
    ) {
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "final_profile_proved_held",
            Some(mode),
            error.to_string(),
        );
    }
    if let Err(error) = guardian.close_authorized() {
        // Never turn a lost/refused close frame into deletion by dropping the
        // parent's last armed handle. The guardian may have flushed its receipt
        // and closed before the ACK was lost, or it may have died beforehand;
        // parent-side cancellation is safe in both cases. Only a successful ACK
        // permits the normal close below. A dual cancellation failure retains
        // the handle and journal state fail-closed.
        return cancel_guarded_disposition(
            conn,
            &journal,
            held,
            &mut guardian,
            "close_authorized",
            Some(mode),
            format!("authenticated guardian close acknowledgement was not obtained: {error}"),
        );
    }
    // `close_authorized` is only durable intent. The guardian-written receipt
    // was flushed before its exact target handle closed; the authenticated ACK
    // advances the fast path, while recovery can still validate that receipt if
    // this journal transition is lost.
    let acknowledgement_persist = transition_guardian_state(
        conn,
        &journal,
        "close_authorized",
        "guardian_handle_closed",
        Some(mode),
        None,
        false,
    );
    held.close_proved_final_disposition();

    if let Err(error) = acknowledgement_persist {
        let message = format!(
            "authenticated guardian HandleClosed ACK was received but not durably persisted; recovery will require the guardian-written durable receipt: {error}"
        );
        record_proved_close_outcome_unknown(conn, journal.batch_operation_id, &journal, &message)?;
        return Err(FinalRemoveError::InvalidPreview(message));
    }
    if let Err(error) = mark_guardian_parent_handle_closed(conn, &journal, mode) {
        let message = format!(
            "parent final handle closed after a durable guardian ACK, but the parent-close receipt requires recovery: {error}"
        );
        record_proved_close_outcome_unknown(conn, journal.batch_operation_id, &journal, &message)?;
        return Err(FinalRemoveError::InvalidPreview(message));
    }
    record_delete_success(conn, journal.batch_operation_id, journal.batch_id, &journal)?;
    let cleanup = guardian.cleanup_receipt_after_terminal_state();
    record_guardian_receipt_cleanup_outcome(conn, &journal, cleanup.as_ref().err())?;
    Ok(())
}

#[cfg(windows)]
fn record_guardian_receipt_cleanup_outcome(
    conn: &Connection,
    journal: &JournaledBatchItem,
    error: Option<&crate::elevated_transport::ElevatedTransportError>,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let updated = if let Some(error) = error {
        conn.execute(
            "UPDATE final_disposition_guardian
             SET updated_at = ?3,
                 error = COALESCE(error || '; ', '') || ?4
             WHERE batch_item_id = ?1 AND operation_id = ?2
               AND state IN ('parent_handle_closed', 'cancelled_safe')
               AND receipt_cleanup_complete = 0",
            params![
                journal.batch_item_id,
                journal.batch_operation_id,
                now,
                bounded_guardian_journal_message(&format!(
                    "terminal deletion committed but exact receipt cleanup is pending: {error}"
                )),
            ],
        )?
    } else {
        conn.execute(
            "UPDATE final_disposition_guardian
             SET receipt_cleanup_complete = 1, updated_at = ?3
             WHERE batch_item_id = ?1 AND operation_id = ?2
               AND state IN ('parent_handle_closed', 'cancelled_safe')
               AND receipt_cleanup_complete = 0",
            params![journal.batch_item_id, journal.batch_operation_id, now],
        )?
    };
    if updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "guardian receipt cleanup result lost its durable row".to_string(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn cancel_guarded_disposition(
    conn: &Connection,
    journal: &JournaledBatchItem,
    held: BoundObjectProof,
    guardian: &mut DispositionGuardian,
    expected_guardian_state: &str,
    mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
    failure: String,
) -> Result<(), FinalRemoveError> {
    // The parent attempts cancellation first, but that proof is not enough to
    // terminate the separate guardian. An authenticated Cancel frame also tells
    // the guardian that this parent can never perform its one future arm. If we
    // skip it before ProveArmed, the child must conservatively retain its
    // share-zero duplicate until desktop exit and a same-session retry fails.
    let parent_cancelled = match mode {
        Some(mode) => held.cancel_final_disposition(mode).is_ok(),
        None => held.cancel_final_disposition_unknown_mode().is_ok(),
    };
    let guardian_outcome = guardian.cancel(mode).ok();
    let cancelled_safe = guardian_outcome == Some(GuardianCancelOutcome::CancelledSafe);
    let next_state = if cancelled_safe {
        "cancelled_safe"
    } else {
        "cancellation_pending_retained"
    };
    let journal_transition = transition_guardian_state(
        conn,
        journal,
        expected_guardian_state,
        next_state,
        mode,
        Some(&failure),
        false,
    );

    if cancelled_safe || parent_cancelled {
        // A proved parent cancellation makes this parent handle safe to close,
        // even if the guardian channel itself became ambiguous. Only the
        // guardian ACK, however, permits terminal receipt cleanup below.
        drop(held);
    } else {
        // Dropping the controller closes only the pipe/process handle. The
        // child is deliberately not terminated; it retains and retries the
        // duplicated file handle after detecting the parent disconnect.
        held.retain_unproved_final_disposition();
    }
    let receipt_cleanup = if cancelled_safe {
        Some(guardian.cleanup_receipt_after_terminal_state())
    } else {
        None
    };
    let record_result = record_delete_failure(conn, journal.batch_operation_id, journal, &failure);
    if let Err(error) = journal_transition {
        return Err(FinalRemoveError::InvalidPreview(format!(
            "{failure}; cancellation physical state was handled but guardian journaling failed: {error}"
        )));
    }
    record_result?;
    if let Some(cleanup) = receipt_cleanup {
        record_guardian_receipt_cleanup_outcome(conn, journal, cleanup.as_ref().err())?;
    }
    Err(FinalRemoveError::InvalidPreview(format!(
        "exact-handle disposition stopped for entry {}: {failure}",
        journal.row.entry_id
    )))
}

fn bound_object_matches_v013_final_stream_profile(proof: &BoundObjectProof) -> bool {
    proof.matches_final_stream_profile()
}

fn block_ready_topology_group(
    conn: &Connection,
    items: &[&JournaledBatchItem],
    code: &str,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let first = items.first().ok_or_else(|| {
        FinalRemoveError::InvalidPreview("empty topology group cannot be blocked".to_string())
    })?;
    if items.iter().any(|item| {
        item.batch_id != first.batch_id
            || item.batch_operation_id != first.batch_operation_id
            || item.topology_group_id != first.topology_group_id
    }) {
        return Err(FinalRemoveError::InvalidPreview(
            "mixed topology group cannot be blocked atomically".to_string(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    for item in items {
        let blocked = transaction.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'blocked', status = 'blocked', reason_code = ?4,
                 message = ?5, updated_at = ?2
             WHERE id = ?1 AND batch_id = ?3 AND status = 'ready'
               AND archive_id IS NOT NULL",
            params![item.batch_item_id, now, item.batch_id, code, message],
        )?;
        let skipped = transaction.execute(
            "UPDATE operation_item SET status = 'skipped'
             WHERE id = ?1 AND operation_id = ?2 AND status = 'pending'",
            params![item.operation_item_id, item.batch_operation_id],
        )?;
        if blocked != 1 || skipped != 1 {
            return Err(FinalRemoveError::InvalidPreview(
                "topology stream-profile block CAS lost authority before disposition".to_string(),
            ));
        }
    }
    transaction.commit()?;
    Ok(())
}

fn record_delete_success(
    conn: &Connection,
    operation_id: i64,
    batch_id: i64,
    item: &JournaledBatchItem,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let entry = transaction.execute(
        "UPDATE quarantine_entry SET status = 'permanently_deleted'
         WHERE id = ?1 AND status = 'deleting'",
        [item.row.entry_id],
    )?;
    let batch_item = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'finished', status = 'deleted', reason_code = NULL,
             message = NULL, updated_at = ?2
         WHERE id = ?1 AND status = 'deleting'",
        params![item.batch_item_id, now],
    )?;
    let operation_item = transaction.execute(
        "UPDATE operation_item SET status = 'done'
         WHERE id = ?1 AND status = 'deleting'",
        [item.operation_item_id],
    )?;
    if entry != 1 || batch_item != 1 || operation_item != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "filesystem disposition completed but durable success CAS was lost; recovery is required"
                .to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO mutation_space_effect(
            operation_id, operation_item_id, volume_id, lifecycle_stage,
            logical_bytes, allocated_bytes, free_space_delta_observed, created_at
         ) VALUES(?1, ?2, ?3, 'holding_object_removed', ?4, NULL, NULL, ?5)",
        params![
            operation_id,
            item.operation_item_id,
            item.row.result_volume_id,
            item.row.logical_bytes as i64,
            now,
        ],
    )?;
    transaction.execute(
        "UPDATE permanent_delete_batch SET removed_count = removed_count + 1
         WHERE id = ?1",
        [batch_id],
    )?;
    transaction.commit()?;
    Ok(())
}

fn record_delete_failure(
    conn: &Connection,
    operation_id: i64,
    item: &JournaledBatchItem,
    message: &str,
) -> Result<(), FinalRemoveError> {
    // A disposition error can be ambiguous after the kernel accepted a delete
    // intent. Never relabel the held entry as quarantined merely because a path
    // lookup might still succeed; durable recovery must rebind identity/hash.
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'parent_disposition', status = 'interrupted',
             reason_code = ?4, message = ?2, updated_at = ?3
         WHERE id = ?1 AND status = 'deleting'",
        params![
            item.batch_item_id,
            message,
            now,
            UNPROVED_FINAL_PROFILE_REASON_CODE,
        ],
    )?;
    transaction.execute(
        "UPDATE operation_item SET status = 'failed'
         WHERE id = ?1 AND status = 'deleting'",
        [item.operation_item_id],
    )?;
    transaction.execute(
        "UPDATE operation SET status = 'interrupted', error = ?2
         WHERE id = ?1",
        params![operation_id, message],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Record a transport ambiguity *after* the final profile and close authority
/// are durable. Unlike a pre-proof failure this must preserve the proved marker
/// so recovery may settle an absent path, while an exact present object is still
/// rolled back to quarantine.
#[cfg(windows)]
fn record_proved_close_outcome_unknown(
    conn: &Connection,
    operation_id: i64,
    item: &JournaledBatchItem,
    message: &str,
) -> Result<(), FinalRemoveError> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    let item_updated = transaction.execute(
        "UPDATE permanent_delete_batch_item
         SET phase = 'parent_disposition', status = 'interrupted',
             reason_code = ?4, message = ?2, updated_at = ?3
         WHERE id = ?1 AND status = 'deleting'
           AND reason_code = ?4",
        params![
            item.batch_item_id,
            message,
            now,
            PROVED_FINAL_PROFILE_REASON_CODE,
        ],
    )?;
    let operation_item_updated = transaction.execute(
        "UPDATE operation_item SET status = 'failed'
         WHERE id = ?1 AND status = 'deleting'",
        [item.operation_item_id],
    )?;
    if item_updated != 1 || operation_item_updated != 1 {
        return Err(FinalRemoveError::InvalidPreview(
            "proved close ambiguity could not retain its durable recovery marker".to_string(),
        ));
    }
    transaction.execute(
        "UPDATE operation SET status = 'interrupted', error = ?2
         WHERE id = ?1",
        params![operation_id, message],
    )?;
    transaction.commit()?;
    Ok(())
}

fn finalize_batch(
    conn: &Connection,
    operation_id: i64,
    batch_id: i64,
    interruption: Option<FinalRemoveInterruptionReason>,
) -> Result<(), FinalRemoveError> {
    let (requested, removed, blocked, failed, interrupted): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),
                    SUM(CASE WHEN status = 'deleted' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status IN ('deleting', 'interrupted') THEN 1 ELSE 0 END)
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
    let status = if interrupted > 0
        || interruption == Some(FinalRemoveInterruptionReason::ProgressObserverFailed)
    {
        "interrupted"
    } else if interruption == Some(FinalRemoveInterruptionReason::OwnerStop) {
        "cancelled"
    } else if removed == requested {
        "completed"
    } else if removed > 0 {
        "partial"
    } else {
        "failed"
    };
    let terminal_error = interruption.map(|reason| interruption_details(reason).1);
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "UPDATE permanent_delete_batch
         SET eligible_count = ?2, removed_count = ?3, blocked_count = ?4,
             failed_count = ?5, status = ?6, finished_at = ?7,
             error = COALESCE(?8, error)
         WHERE id = ?1",
        params![
            batch_id,
            requested - blocked,
            removed,
            blocked,
            failed + interrupted,
            status,
            now,
            terminal_error,
        ],
    )?;
    transaction.execute(
        "UPDATE operation SET status = ?2, finished_at = ?3,
             error = COALESCE(?4, error)
         WHERE id = ?1",
        params![operation_id, status, now, terminal_error],
    )?;
    transaction.commit()?;
    Ok(())
}

fn load_batch_result(
    conn: &Connection,
    batch_id: i64,
) -> Result<FinalRemoveBatchResult, FinalRemoveError> {
    let (public_id, status, requested): (Option<String>, String, u64) = conn
        .query_row(
            "SELECT public_id, status, requested_count
             FROM permanent_delete_batch WHERE id = ?1",
            [batch_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| FinalRemoveError::InvalidPreview("batch was not found".to_string()))?;
    #[derive(Debug)]
    struct ResultRow {
        entry_id: i64,
        project_group: String,
        state: String,
        reason_code: Option<String>,
        message: Option<String>,
        logical_bytes: u64,
        held_volume_id: String,
        archive_volume_id: Option<String>,
        archive_bytes: Option<u64>,
    }
    let mut statement = conn.prepare(
        "SELECT p.quarantine_entry_id, p.removal_group_id, p.status,
                p.reason_code, p.message, p.logical_bytes, p.expected_volume_id,
                oa.archive_volume_id, oa.archive_bytes
         FROM permanent_delete_batch_item p
         LEFT JOIN object_archive oa ON oa.id = p.archive_id
         WHERE p.batch_id = ?1 ORDER BY p.id",
    )?;
    let rows = statement
        .query_map([batch_id], |row| {
            Ok(ResultRow {
                entry_id: row.get(0)?,
                project_group: row.get(1)?,
                state: row.get(2)?,
                reason_code: row.get(3)?,
                message: row.get(4)?,
                logical_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                held_volume_id: row.get(6)?,
                archive_volume_id: row.get(7)?,
                archive_bytes: row
                    .get::<_, Option<i64>>(8)?
                    .map(|value| value.max(0) as u64),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let deleted = rows.iter().filter(|row| row.state == "deleted").count() as u64;
    let failed = rows
        .iter()
        .filter(|row| matches!(row.state.as_str(), "failed" | "interrupted" | "deleting"))
        .count() as u64;
    let kept = requested.saturating_sub(deleted).saturating_sub(failed);

    let mut project_totals: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut volume_totals: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for row in &rows {
        let totals = project_totals.entry(row.project_group.clone()).or_default();
        match row.state.as_str() {
            "deleted" => totals.0 += 1,
            "failed" | "interrupted" | "deleting" => totals.2 += 1,
            _ => totals.1 += 1,
        }
        let held = volume_totals.entry(row.held_volume_id.clone()).or_default();
        held.0 = held.0.saturating_add(row.logical_bytes);
        if row.state == "deleted" {
            held.1 = held.1.saturating_add(row.logical_bytes);
        }
    }
    let projects = project_totals
        .into_iter()
        .map(
            |(group_id, (deleted, kept, failed))| FinalRemoveProjectResult {
                group_id,
                deleted,
                kept,
                failed,
            },
        )
        .collect();
    let mut archive_totals = BTreeMap::<String, u64>::new();
    for row in &rows {
        if let (Some(volume), Some(bytes)) = (&row.archive_volume_id, row.archive_bytes) {
            *archive_totals.entry(volume.clone()).or_default() = archive_totals
                .get(volume)
                .copied()
                .unwrap_or_default()
                .saturating_add(bytes);
        }
    }
    let all_volumes = volume_totals
        .keys()
        .chain(archive_totals.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let volumes = all_volumes
        .into_iter()
        .map(|volume_id| {
            let (held, removed) = volume_totals.get(&volume_id).copied().unwrap_or_default();
            FinalRemoveVolumeImpact {
                label: volume_id.clone(),
                volume_id: volume_id.clone(),
                already_freed_from_source_bytes: 0,
                held_allocated_bytes: held,
                projected_release_bytes: removed,
                archive_retained_allocated_bytes: archive_totals
                    .get(&volume_id)
                    .copied()
                    .unwrap_or_default(),
                free_bytes_before: None,
                free_bytes_after: None,
                observed_delta_bytes: None,
                quality: "estimated".to_string(),
            }
        })
        .collect();
    let items = rows
        .into_iter()
        .map(|row| FinalRemoveBatchItemResult {
            entry_id: row.entry_id,
            state: match row.state.as_str() {
                "deleted" => "deleted",
                "failed" | "interrupted" | "deleting" => "deleteFailed",
                _ => "kept",
            }
            .to_string(),
            reason_code: row.reason_code.as_deref().map(wire_reason_code),
            error: row.message,
        })
        .collect();
    Ok(FinalRemoveBatchResult {
        batch_id: public_id.unwrap_or_else(|| batch_id.to_string()),
        status,
        requested_objects: requested,
        deleted_objects: deleted,
        kept_objects: kept,
        failed_objects: failed,
        projects,
        volumes,
        items,
        archive_retained: true,
    })
}

fn wire_reason_code(code: &str) -> String {
    match code {
        "archiveVerified"
        | "legacyContentOnly"
        | "archiveMissing"
        | "archiveCorrupt"
        | "unsupportedReparse"
        | "unsupportedEfs"
        | "unsupportedObjectStream"
        | "externalHardlink"
        | "nonNtfs"
        | "cloudOrRecall"
        | "locked"
        | "identityChanged"
        | "insufficientSpace"
        | "permissionDenied"
        | "helperUnsigned"
        | "helperUntrusted"
        | "releaseManifestMismatch"
        | "uacCancelled"
        | "capacityBlocked"
        | "scratchCleanupPending"
        | "stopRequested"
        | "interrupted" => code.to_string(),
        "archivePromotionPending" | "progressObserverFailed" => "interrupted".to_string(),
        "objectClassUnsupported" | "helperUnsupported" => "unsupportedObjectStream".to_string(),
        "archiveInsufficientSpace" | "archiveSpaceUnknown" => "insufficientSpace".to_string(),
        "helperIdentityInvalid" => "helperUntrusted".to_string(),
        "archiveDestinationUnsafe" | "archiveCreateFailed" => "permissionDenied".to_string(),
        "dispositionAmbiguous"
        | "unprovedFinalProfile"
        | "finalProfileProvedHeld"
        | "helperTimeout"
        | "helperFailed"
        | "journalInvalid"
        | "randomUnavailable" => "interrupted".to_string(),
        _ => "interrupted".to_string(),
    }
}

#[derive(Debug, Clone)]
struct PersistedPreviewItem {
    entry_id: i64,
    project_group_id: String,
    topology_group_id: String,
    eligibility: String,
}

fn load_persisted_preview_items(
    conn: &Connection,
    preview_id: &str,
) -> Result<Vec<PersistedPreviewItem>, FinalRemoveError> {
    let mut statement = conn.prepare(
        "SELECT quarantine_entry_id, removal_group_id, topology_group_id, eligibility
         FROM permanent_delete_preview_item
         WHERE preview_id = ?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([preview_id], |row| {
            Ok(PersistedPreviewItem {
                entry_id: row.get(0)?,
                project_group_id: row.get(1)?,
                topology_group_id: row.get(2)?,
                eligibility: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn eligible_persisted_topology_groups(rows: &[PersistedPreviewItem]) -> BTreeSet<String> {
    let mut grouped: BTreeMap<&str, Vec<&PersistedPreviewItem>> = BTreeMap::new();
    for row in rows {
        grouped
            .entry(row.topology_group_id.as_str())
            .or_default()
            .push(row);
    }
    grouped
        .into_iter()
        .filter_map(|(group, members)| {
            members
                .iter()
                .all(|member| member.eligibility != "blocked")
                .then(|| group.to_string())
        })
        .collect()
}

fn load_preview_rows(conn: &Connection) -> Result<Vec<PreviewRow>, FinalRemoveError> {
    let mut statement = conn.prepare(
        "SELECT qe.id, qe.operation_id, qe.original_path, qe.quarantine_path,
                COALESCE(qe.size, 0), COALESCE(qe.space_recovered, 0),
                qe.backup_id, COALESCE(b.verified, 0), qe.removal_group_id,
                qe.removal_group_fingerprint, o.target_fingerprint,
                oi.expected_volume_id, oi.result_volume_id, oi.result_file_id,
                oi.bytes, oi.result_modified_unix_seconds,
                COALESCE(oi.result_blake3, oi.checksum_after),
                oa.id, oa.archive_path, oa.archive_volume_id, oa.archive_file_id,
                oa.archive_bytes, oa.archive_modified_unix_seconds,
                oa.archive_blake3, oa.raw_backup_blake3, oa.stream_count,
                oa.proof_schema, oa.status, oa.held_volume_id, oa.held_file_id,
                oa.held_bytes, oa.held_modified_unix_seconds,
                oa.held_content_blake3, oa.semantic_blake3,
                oa.roundtrip_blake3, oa.security_stream_present,
                oa.cleanup_complete, oa.verified_at
         FROM quarantine_entry qe
         LEFT JOIN operation o ON o.id = qe.operation_id
         LEFT JOIN backup b ON b.id = qe.backup_id
         LEFT JOIN operation_item oi ON oi.id = (
             SELECT oi2.id FROM operation_item oi2
             WHERE oi2.operation_id = qe.operation_id
               AND oi2.to_path = qe.quarantine_path
               AND oi2.status = 'done'
             ORDER BY oi2.id DESC LIMIT 1
         )
         LEFT JOIN object_archive oa ON oa.id = (
             SELECT oa2.id FROM object_archive oa2
             WHERE oa2.quarantine_entry_id = qe.id
                OR (oa2.quarantine_entry_id IS NULL
                    AND oa2.backup_id = qe.backup_id
                    AND oa2.original_path = qe.original_path COLLATE NOCASE)
             ORDER BY (oa2.quarantine_entry_id IS NULL) ASC, oa2.id DESC LIMIT 1
         )
         WHERE qe.status = 'quarantined'
         ORDER BY qe.id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(PreviewRow {
                entry_id: row.get(0)?,
                operation_id: row.get(1)?,
                original_path: row.get(2)?,
                held_path: row.get(3)?,
                logical_bytes: row.get::<_, i64>(4)?.max(0) as u64,
                space_recovered: row.get::<_, i64>(5)?.max(0) as u64,
                backup_id: row.get(6)?,
                backup_verified: row.get::<_, i64>(7)? == 1,
                removal_group_id: row.get(8)?,
                removal_group_fingerprint: row.get(9)?,
                operation_target_fingerprint: row.get(10)?,
                expected_volume_id: row.get(11)?,
                result_volume_id: row.get(12)?,
                result_file_id: row.get(13)?,
                result_bytes: row
                    .get::<_, Option<i64>>(14)?
                    .map(|value| value.max(0) as u64),
                result_modified_unix_seconds: row.get(15)?,
                result_blake3: row.get(16)?,
                archive_id: row.get(17)?,
                archive_path: row.get(18)?,
                archive_volume_id: row.get(19)?,
                archive_file_id: row.get(20)?,
                archive_bytes: row
                    .get::<_, Option<i64>>(21)?
                    .map(|value| value.max(0) as u64),
                archive_modified_unix_seconds: row.get(22)?,
                archive_blake3: row.get(23)?,
                raw_backup_blake3: row.get(24)?,
                stream_count: row
                    .get::<_, Option<i64>>(25)?
                    .map(|value| value.max(0) as u32),
                proof_schema: row.get(26)?,
                proof_status: row.get(27)?,
                proof_held_volume_id: row.get(28)?,
                proof_held_file_id: row.get(29)?,
                proof_held_bytes: row
                    .get::<_, Option<i64>>(30)?
                    .map(|value| value.max(0) as u64),
                proof_held_modified_unix_seconds: row.get(31)?,
                proof_held_blake3: row.get(32)?,
                proof_semantic: row.get(33)?,
                proof_roundtrip: row.get(34)?,
                proof_security: row.get::<_, Option<i64>>(35)?.map(|value| value == 1),
                proof_cleanup: row.get::<_, Option<i64>>(36)?.map(|value| value == 1),
                proof_verified_at: row.get(37)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn validate_scope(scope: &FinalRemoveScope) -> Result<(), FinalRemoveError> {
    let groups = match scope {
        FinalRemoveScope::Project { group_id } => std::slice::from_ref(group_id),
        FinalRemoveScope::Groups { group_ids } => group_ids.as_slice(),
        FinalRemoveScope::AllEligible => return Ok(()),
    };
    if groups.is_empty()
        || groups
            .iter()
            .any(|group| group.is_empty() || group.len() > 256)
    {
        return Err(FinalRemoveError::InvalidPreview(
            "scope groups must contain bounded non-empty identifiers".to_string(),
        ));
    }
    let distinct = groups.iter().collect::<BTreeSet<_>>();
    if distinct.len() != groups.len() {
        return Err(FinalRemoveError::InvalidPreview(
            "scope groups contain duplicates".to_string(),
        ));
    }
    Ok(())
}

fn scope_selects(scope: &FinalRemoveScope, group: &str) -> bool {
    match scope {
        FinalRemoveScope::Project { group_id } => group_id == group,
        FinalRemoveScope::Groups { group_ids } => group_ids.iter().any(|value| value == group),
        FinalRemoveScope::AllEligible => true,
    }
}

fn project_group_id(row: &PreviewRow) -> String {
    row.removal_group_id
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            row.operation_target_fingerprint
                .as_ref()
                .filter(|value| !value.is_empty())
                .map(|value| format!("operation:{}:{value}", row.operation_id.unwrap_or(0)))
        })
        .unwrap_or_else(|| format!("legacy-entry:{}", row.entry_id))
}

fn topology_group_id(row: &PreviewRow) -> String {
    match (&row.result_volume_id, &row.result_file_id) {
        (Some(volume), Some(file)) if !volume.is_empty() && !file.is_empty() => {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"codehangar/final-remove-topology/1\0");
            hasher.update(&(volume.len() as u64).to_le_bytes());
            hasher.update(volume.as_bytes());
            hasher.update(&(file.len() as u64).to_le_bytes());
            hasher.update(file.as_bytes());
            format!("object:{}", hasher.finalize().to_hex())
        }
        _ => format!("unproven-entry:{}", row.entry_id),
    }
}

fn classify_row(row: PreviewRow) -> ClassifiedRow {
    let project_group_id = project_group_id(&row);
    let topology_group_id = topology_group_id(&row);
    let identity_complete = row
        .result_volume_id
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        && row
            .result_file_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        && row.result_bytes.is_some()
        && row.result_modified_unix_seconds.is_some()
        && row
            .result_blake3
            .as_ref()
            .is_some_and(|value| is_hex_64(value));
    let proof_identity_matches = row.proof_held_volume_id == row.result_volume_id
        && row.proof_held_file_id == row.result_file_id
        && row.proof_held_bytes == row.result_bytes
        && row.proof_held_blake3 == row.result_blake3
        && row.result_modified_unix_seconds.is_some()
        && row.proof_held_modified_unix_seconds == row.result_modified_unix_seconds;
    let archive_identity_complete = row.archive_id.is_some_and(|value| value > 0)
        && row
            .archive_path
            .as_deref()
            .is_some_and(valid_bounded_local_path_text)
        && row
            .archive_volume_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        && row
            .archive_file_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        && row.archive_bytes.is_some_and(|value| value > 0)
        && row.archive_modified_unix_seconds.is_some()
        && row.archive_blake3.as_deref().is_some_and(is_hex_64)
        && row.raw_backup_blake3.as_deref().is_some_and(is_hex_64)
        && row.proof_semantic.as_deref().is_some_and(is_hex_64)
        && row.proof_roundtrip.as_deref().is_some_and(is_hex_64)
        && row
            .stream_count
            .is_some_and(|value| value > 0 && value <= 4_096)
        && row
            .proof_verified_at
            .as_deref()
            .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok());
    let proof_ready = identity_complete
        && row.backup_verified
        && proof_identity_matches
        && archive_identity_complete
        && row.proof_schema.as_deref() == Some(OBJECT_ARCHIVE_PROOF_SCHEMA)
        && row.proof_status.as_deref() == Some("ready")
        && row.proof_semantic == row.proof_roundtrip
        && row.proof_security == Some(true)
        && row.proof_cleanup == Some(true);
    let (eligibility, reason_code, reason, remediation, archive_state) = if !identity_complete {
        (
            "blocked",
            "identityChanged",
            "The held object has no complete volume/file-id/size/hash journal proof.",
            Some("Reconcile or restore this entry before attempting final cleanup."),
            "invalid",
        )
    } else if row.backup_id.filter(|value| *value > 0).is_none() || !row.backup_verified {
        (
            "blocked",
            "archiveMissing",
            "No verified recovery destination is linked to this held object.",
            Some("Create and verify a recovery archive before final cleanup."),
            "none",
        )
    } else if proof_ready {
        (
            "ready",
            "archiveVerified",
            "The object-complete recovery archive and restore roundtrip are verified.",
            None,
            "objectCompleteVerified",
        )
    } else if row.archive_id.is_some() {
        (
            "blocked",
            "archiveCorrupt",
            "The recorded object_archive/2 row is incomplete, inconsistent, or no longer proves this exact held object.",
            Some("Keep the object in holding and reconcile the recorded archive before a replacement is captured."),
            "invalid",
        )
    } else {
        (
            "needsArchiveV2",
            "legacyContentOnly",
            "The existing backup does not yet prove every Windows object stream and security field.",
            Some("Windows will ask once to capture and roundtrip-verify object_archive/2 for the selected batch."),
            "contentOnlyLegacy",
        )
    };
    let held_volume_id = row
        .result_volume_id
        .clone()
        .unwrap_or_else(|| format!("unknown-entry-{}", row.entry_id));
    let decision = FinalRemoveObjectDecision {
        entry_id: row.entry_id,
        group_id: project_group_id.clone(),
        topology_group_id: topology_group_id.clone(),
        // Replaced with a common-root relative path after the complete project
        // group is classified. Keeping the full path here is unambiguous if a
        // malformed group cannot yield a common ancestor.
        relative_path: row.original_path.clone(),
        kind: "file".to_string(),
        lifecycle: "held".to_string(),
        eligibility: eligibility.to_string(),
        reason_code: reason_code.to_string(),
        reason: reason.to_string(),
        remediation: remediation.map(str::to_string),
        archive_id: row.archive_id.map(|value| value.to_string()),
        object_archive_state: archive_state.to_string(),
        held_volume_label: held_volume_id.clone(),
        held_volume_id,
        logical_bytes: row.logical_bytes,
        allocated_bytes: None,
        measurement: if proof_ready {
            "exactStreams".to_string()
        } else if identity_complete {
            "logicalUpperBound".to_string()
        } else {
            "unknown".to_string()
        },
    };
    ClassifiedRow {
        row,
        project_group_id,
        topology_group_id,
        decision,
    }
}

fn valid_bounded_local_path_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32_767
        && Path::new(value).is_absolute()
        && crate::bound_fs::validate_local_mutation_path(Path::new(value)).is_ok()
}

fn block_decision(item: &mut ClassifiedRow, reason_code: &str, reason: String, remediation: &str) {
    item.decision.eligibility = "blocked".to_string();
    item.decision.reason_code = reason_code.to_string();
    item.decision.reason = reason;
    item.decision.remediation = Some(remediation.to_string());
}

/// Apply global topology truth before projecting a user scope. If a selected
/// pathname shares an object identity with any held pathname outside the
/// requested project/groups, none of that object's selected members can be
/// authorized. Likewise, one blocked member blocks the complete atomic group.
fn propagate_topology_blockage(rows: &mut [ClassifiedRow], selected: &BTreeSet<i64>) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        groups
            .entry(row.topology_group_id.clone())
            .or_default()
            .push(index);
    }
    for (group, members) in groups {
        let selected_members = members
            .iter()
            .copied()
            .filter(|index| selected.contains(&rows[*index].row.entry_id))
            .collect::<Vec<_>>();
        if selected_members.is_empty() {
            continue;
        }
        if selected_members.len() != members.len() {
            for index in selected_members {
                if rows[index].decision.eligibility != "blocked" {
                    block_decision(
                        &mut rows[index],
                        "externalHardlink",
                        format!(
                            "Atomic topology group {group} also has a held pathname outside this preview scope."
                        ),
                        "Select every project/removal group containing this object, or restore the complete topology group.",
                    );
                }
            }
            continue;
        }
        let blocked_reason = members.iter().find_map(|index| {
            (rows[*index].decision.eligibility == "blocked")
                .then(|| rows[*index].decision.reason_code.clone())
        });
        if let Some(reason) = blocked_reason {
            for index in selected_members {
                if rows[index].decision.eligibility != "blocked" {
                    block_decision(
                        &mut rows[index],
                        "externalHardlink",
                        format!(
                            "Another member of atomic topology group {group} is blocked ({reason})."
                        ),
                        "Resolve every member of the topology group before final cleanup.",
                    );
                }
            }
        }
    }
}

fn propagate_selected_group_blockage(rows: &mut [ClassifiedRow]) {
    let selected = rows
        .iter()
        .map(|item| item.row.entry_id)
        .collect::<BTreeSet<_>>();
    propagate_topology_blockage(rows, &selected);
}

/// v0.1.3 keeps the atomic topology identity in the preview, but the shipped
/// final-disposition primitive proves only a one-pathname object.  Do not offer
/// a confirmation for a multi-path hardlink group and then discover that
/// limitation after UAC/archive work.
fn block_unsupported_multi_path_topologies(rows: &mut [ClassifiedRow]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, item) in rows.iter().enumerate() {
        groups
            .entry(item.topology_group_id.clone())
            .or_default()
            .push(index);
    }
    for (group, members) in groups {
        if members.len() <= 1 {
            continue;
        }
        let pathname_count = members.len();
        for index in members {
            if rows[index].decision.eligibility != "blocked" {
                block_decision(
                    &mut rows[index],
                    "externalHardlink",
                    format!(
                        "Atomic topology group {group} has {pathname_count} held pathnames for one file object. Code Hangar v0.1.3 preserves the group but cannot yet prove atomic final disposition of a multi-path hardlink topology."
                    ),
                    "Keep or restore the complete topology group; a later disposition profile can add multi-path support without losing this group identity.",
                );
            }
        }
    }
}

fn apply_capacity_block(rows: &mut [ClassifiedRow]) {
    let initially_eligible = rows
        .iter()
        .filter(|item| item.decision.eligibility != "blocked")
        .count();
    if initially_eligible <= MAX_CAPABILITIES_PER_INVOCATION {
        return;
    }
    for item in rows {
        if item.decision.eligibility != "blocked" {
            block_decision(
                item,
                "capacityBlocked",
                format!(
                    "This immutable preview contains {initially_eligible} eligible objects, above the one-UAC operation limit of {MAX_CAPABILITIES_PER_INVOCATION}."
                ),
                "Create explicit smaller selections as separate batches, each with its own fresh preview and confirmation. Code Hangar will not split this review into hidden prompts or remove a prefix automatically.",
            );
        }
    }
}

fn common_parent(paths: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    let mut paths = paths;
    let first = paths.next()?;
    let mut components = first
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect::<Vec<_>>();
    for path in paths {
        let other = path
            .components()
            .map(|component| component.as_os_str().to_os_string())
            .collect::<Vec<_>>();
        let shared = components
            .iter()
            .zip(other.iter())
            .take_while(|(left, right)| {
                left.to_string_lossy()
                    .eq_ignore_ascii_case(&right.to_string_lossy())
            })
            .count();
        components.truncate(shared);
    }
    if components.is_empty() {
        return None;
    }
    let mut root = PathBuf::new();
    for component in components {
        root.push(component);
    }
    Some(root)
}

fn project_root<'a>(members: impl Iterator<Item = &'a ClassifiedRow>) -> Option<PathBuf> {
    common_parent(members.map(|item| {
        Path::new(&item.row.original_path)
            .parent()
            .unwrap_or_else(|| Path::new(&item.row.original_path))
            .to_path_buf()
    }))
}

fn apply_project_relative_paths(rows: &mut [ClassifiedRow]) {
    let mut roots = BTreeMap::new();
    for group in rows
        .iter()
        .map(|item| item.project_group_id.clone())
        .collect::<BTreeSet<_>>()
    {
        if let Some(root) = project_root(rows.iter().filter(|item| item.project_group_id == group))
        {
            roots.insert(group, root);
        }
    }
    for item in rows {
        if let Some(root) = roots.get(&item.project_group_id) {
            item.decision.relative_path = Path::new(&item.row.original_path)
                .strip_prefix(root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| relative.to_string_lossy().to_string())
                .unwrap_or_else(|| item.row.original_path.clone());
        }
    }
}

fn wholly_eligible_topology_groups(rows: &[ClassifiedRow]) -> Vec<String> {
    let mut grouped: BTreeMap<&str, Vec<&ClassifiedRow>> = BTreeMap::new();
    for row in rows {
        grouped
            .entry(row.topology_group_id.as_str())
            .or_default()
            .push(row);
    }
    grouped
        .into_iter()
        .filter_map(|(group, members)| {
            members
                .iter()
                .all(|member| member.decision.eligibility != "blocked")
                .then(|| group.to_string())
        })
        .collect()
}

fn project_previews(rows: &[ClassifiedRow]) -> Vec<FinalRemoveProjectPreview> {
    let mut grouped: BTreeMap<&str, Vec<&ClassifiedRow>> = BTreeMap::new();
    for row in rows {
        grouped
            .entry(row.project_group_id.as_str())
            .or_default()
            .push(row);
    }
    grouped
        .into_iter()
        .map(|(group, members)| {
            let root = project_root(members.iter().copied())
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| members[0].row.original_path.clone());
            let name = Path::new(&root)
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| group.to_string());
            let blocked_members = members
                .iter()
                .filter(|item| item.decision.eligibility == "blocked")
                .collect::<Vec<_>>();
            FinalRemoveProjectPreview {
                group_id: group.to_string(),
                project_name: name,
                original_root: root,
                total_objects: members.len() as u64,
                ready: members
                    .iter()
                    .filter(|item| item.decision.eligibility == "ready")
                    .count() as u64,
                needs_archive_v2: members
                    .iter()
                    .filter(|item| item.decision.eligibility == "needsArchiveV2")
                    .count() as u64,
                blocked: blocked_members.len() as u64,
                blocked_subtrees: blocked_members
                    .iter()
                    .map(|item| FinalRemoveBlockedSubtree {
                        root: item.row.original_path.clone(),
                        count: 1,
                        reason_codes: vec![item.decision.reason_code.clone()],
                    })
                    .collect(),
            }
        })
        .collect()
}

fn volume_impacts(
    rows: &[ClassifiedRow],
    eligible_groups: &[String],
) -> Vec<FinalRemoveVolumeImpact> {
    #[derive(Default)]
    struct Totals {
        already: u64,
        held: u64,
        projected: u64,
        archive: u64,
    }
    let eligible = eligible_groups.iter().collect::<BTreeSet<_>>();
    let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
    for item in rows {
        let held_volume = item.decision.held_volume_id.clone();
        let held = totals.entry(held_volume).or_default();
        held.held = held.held.saturating_add(item.row.logical_bytes);
        if eligible.contains(&item.topology_group_id) {
            held.projected = held.projected.saturating_add(item.row.logical_bytes);
        }
        if item.row.expected_volume_id == item.row.result_volume_id {
            held.already = held.already.saturating_add(item.row.space_recovered);
        } else if let Some(source_volume) = &item.row.expected_volume_id {
            let source = totals.entry(source_volume.clone()).or_default();
            source.already = source.already.saturating_add(item.row.space_recovered);
        }
        if let (Some(archive_volume), Some(bytes)) =
            (&item.row.archive_volume_id, item.row.archive_bytes)
        {
            let archive = totals.entry(archive_volume.clone()).or_default();
            archive.archive = archive.archive.saturating_add(bytes);
        }
    }
    totals
        .into_iter()
        .map(|(volume_id, total)| FinalRemoveVolumeImpact {
            label: volume_id.clone(),
            volume_id,
            already_freed_from_source_bytes: total.already,
            held_allocated_bytes: total.held,
            projected_release_bytes: total.projected,
            archive_retained_allocated_bytes: total.archive,
            free_bytes_before: None,
            free_bytes_after: None,
            observed_delta_bytes: None,
            quality: "estimated".to_string(),
        })
        .collect()
}

fn preview_digest(rows: &[ClassifiedRow], eligible_groups: &[String]) -> String {
    fn field(hasher: &mut blake3::Hasher, value: &[u8]) {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value);
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"codehangar/permanent-delete-preview/3\0");
    hasher.update(&(rows.len() as u64).to_le_bytes());
    for item in rows {
        let row = &item.row;
        hasher.update(&row.entry_id.to_le_bytes());
        hasher.update(&row.operation_id.unwrap_or(0).to_le_bytes());
        field(&mut hasher, row.original_path.as_bytes());
        field(&mut hasher, row.held_path.as_bytes());
        hasher.update(&row.logical_bytes.to_le_bytes());
        hasher.update(&row.backup_id.unwrap_or(0).to_le_bytes());
        hasher.update(&[u8::from(row.backup_verified)]);
        field(&mut hasher, item.project_group_id.as_bytes());
        field(&mut hasher, item.topology_group_id.as_bytes());
        field(
            &mut hasher,
            row.removal_group_fingerprint
                .as_deref()
                .unwrap_or("")
                .as_bytes(),
        );
        field(
            &mut hasher,
            row.result_volume_id.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.result_file_id.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&row.result_bytes.unwrap_or(0).to_le_bytes());
        hasher.update(&row.result_modified_unix_seconds.unwrap_or(0).to_le_bytes());
        field(
            &mut hasher,
            row.result_blake3.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.proof_schema.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&row.archive_id.unwrap_or(0).to_le_bytes());
        field(
            &mut hasher,
            row.archive_path.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.archive_volume_id.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.archive_file_id.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&row.archive_bytes.unwrap_or(0).to_le_bytes());
        hasher.update(&row.archive_modified_unix_seconds.unwrap_or(0).to_le_bytes());
        field(
            &mut hasher,
            row.archive_blake3.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.raw_backup_blake3.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&row.stream_count.unwrap_or(0).to_le_bytes());
        field(
            &mut hasher,
            row.proof_status.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.proof_held_volume_id.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.proof_held_file_id.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&row.proof_held_bytes.unwrap_or(0).to_le_bytes());
        hasher.update(
            &row.proof_held_modified_unix_seconds
                .unwrap_or(0)
                .to_le_bytes(),
        );
        field(
            &mut hasher,
            row.proof_held_blake3.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.proof_semantic.as_deref().unwrap_or("").as_bytes(),
        );
        field(
            &mut hasher,
            row.proof_roundtrip.as_deref().unwrap_or("").as_bytes(),
        );
        hasher.update(&[u8::from(row.proof_security == Some(true))]);
        hasher.update(&[u8::from(row.proof_cleanup == Some(true))]);
        field(
            &mut hasher,
            row.proof_verified_at.as_deref().unwrap_or("").as_bytes(),
        );
        field(&mut hasher, item.decision.eligibility.as_bytes());
        field(&mut hasher, item.decision.reason_code.as_bytes());
    }
    hasher.update(&(eligible_groups.len() as u64).to_le_bytes());
    for group in eligible_groups {
        field(&mut hasher, group.as_bytes());
    }
    format!("v2:{}", hasher.finalize().to_hex())
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn conn() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&connection).unwrap();
        connection
    }

    fn seed_entry(conn: &Connection, id: i64, project_group: &str, file_id: &str) {
        conn.execute(
            "INSERT INTO operation(id, kind, status, plan_json, target_fingerprint, created_at)
             VALUES(?1, 'quarantine', 'done', '{}', ?2, '2026-08-23T00:00:00Z')",
            params![id, format!("v2:{}", "ab".repeat(32))],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO backup(id, level, destination, manifest_path, verified, created_at)
             VALUES(?1, 'full', 'C:\\backup', 'C:\\backup\\manifest.json', 1,
                    '2026-08-23T00:00:00Z')",
            [id],
        )
        .unwrap();
        let original = format!("C:\\project\\{id}.bin");
        let held = format!("C:\\holding\\{id}.bin");
        conn.execute(
            "INSERT INTO quarantine_entry(
                id, operation_id, original_path, quarantine_path, size, backup_id,
                status, manifest_json, removal_group_id
             ) VALUES(?1, ?1, ?2, ?3, 7, ?1, 'quarantined', '{}', ?4)",
            params![id, original, held, project_group],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO operation_item(
                operation_id, action, from_path, to_path, bytes, checksum_after,
                result_volume_id, result_file_id, result_blake3,
                result_modified_unix_seconds, status
             ) VALUES(?1, 'move', ?2, ?3, 7, ?4, 'volume-c', ?5, ?4, 42, 'done')",
            params![id, original, held, "12".repeat(32), file_id],
        )
        .unwrap();
    }

    fn seed_ready_archive(conn: &Connection, id: i64, archive_hash: &str, raw_hash: &str) {
        let content_hash = "12".repeat(32);
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
             ) VALUES(?1, ?1, 'project:7', ?2, ?3, 'volume-c', ?4, 7, 42, ?5,
                      ?6, 'volume-c', ?4, 7, 42, ?5,
                      'volume-d', ?7, 4096, 51, ?8, ?9, ?10, ?10, 3,
                      1, 1, ?11, 'ready', '2026-08-23T00:00:00Z')",
            params![
                id,
                format!("C:\\project\\{id}.bin"),
                format!("C:\\holding\\{id}.bin"),
                format!("file-{id}"),
                content_hash,
                format!("C:\\backup\\entry-{id}.chobj"),
                format!("archive-file-{id}"),
                archive_hash,
                raw_hash,
                semantic,
                OBJECT_ARCHIVE_PROOF_SCHEMA,
            ],
        )
        .unwrap();
    }

    fn seed_batch_header(conn: &Connection, id: i64) {
        conn.execute(
            "INSERT INTO operation(
                id, kind, status, plan_json, target_fingerprint, created_at
             ) VALUES(?1, 'permanent_delete_batch', 'waiting_for_uac', '{}', ?2,
                      '2026-08-23T00:00:00Z')",
            params![id, format!("v2:{}", "cd".repeat(32))],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO permanent_delete_batch(
                id, public_id, operation_id, preview_id, preview_digest,
                selected_groups_json, requested_count, status, created_at
             ) VALUES(?1, ?2, ?1, 'preview-test', ?3, '[\"topology:test\"]',
                      3, 'waiting_for_uac', '2026-08-23T00:00:00Z')",
            params![id, format!("batch-{id}"), format!("v2:{}", "ef".repeat(32))],
        )
        .unwrap();
    }

    fn seed_batch_item(
        conn: &Connection,
        batch_id: i64,
        row: PreviewRow,
        status: &str,
    ) -> JournaledBatchItem {
        let operation_item_id = 10_000 + row.entry_id;
        let batch_item_id = 20_000 + row.entry_id;
        conn.execute(
            "INSERT INTO operation_item(
                id, operation_id, action, from_path, bytes, status
             ) VALUES(?1, ?2, 'final_remove_bound', ?3, ?4, 'pending')",
            params![
                operation_item_id,
                batch_id,
                row.held_path,
                row.logical_bytes as i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO permanent_delete_batch_item(
                id, batch_id, operation_item_id, quarantine_entry_id, archive_id,
                removal_group_id, topology_group_id, held_path,
                expected_volume_id, expected_file_id, expected_bytes,
                expected_modified_unix_seconds, expected_content_blake3,
                logical_bytes, phase, status, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, 'project:test', 'topology:test', ?6,
                      ?7, ?8, ?9, ?10, ?11, ?12, 'archive_ready', ?13,
                      '2026-08-23T00:00:00Z', '2026-08-23T00:00:00Z')",
            params![
                batch_item_id,
                batch_id,
                operation_item_id,
                row.entry_id,
                row.archive_id,
                row.held_path,
                row.result_volume_id,
                row.result_file_id,
                row.result_bytes.map(|value| value as i64),
                row.result_modified_unix_seconds,
                row.result_blake3,
                row.logical_bytes as i64,
                status,
            ],
        )
        .unwrap();
        JournaledBatchItem {
            batch_item_id,
            batch_id,
            batch_operation_id: batch_id,
            operation_item_id,
            row,
            project_group_id: "project:test".to_string(),
            topology_group_id: "topology:test".to_string(),
        }
    }

    #[test]
    fn wire_scope_matches_tagged_frontend_contract() {
        let all: FinalRemoveScope = serde_json::from_str(r#"{"kind":"allEligible"}"#).unwrap();
        assert_eq!(all, FinalRemoveScope::AllEligible);
        let project: FinalRemoveScope =
            serde_json::from_str(r#"{"kind":"project","groupId":"project:7"}"#).unwrap();
        assert_eq!(
            project,
            FinalRemoveScope::Project {
                group_id: "project:7".to_string()
            }
        );
        assert_eq!(
            serde_json::to_value(FinalRemoveScope::Groups {
                group_ids: vec!["a".to_string(), "b".to_string()]
            })
            .unwrap(),
            serde_json::json!({"kind":"groups","groupIds":["a","b"]})
        );
    }

    #[test]
    fn preview_shape_and_confirmation_accept_exact_eligible_subset() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "file-a");
        seed_entry(&conn, 2, "project:7", "file-b");
        seed_entry(&conn, 3, "project:8", "file-c");
        let preview = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        assert_eq!(preview.projects.len(), 2);
        assert_eq!(preview.objects.len(), 3);
        assert_eq!(
            preview.max_delete_objects,
            MAX_CAPABILITIES_PER_INVOCATION as u64
        );
        let wire = serde_json::to_value(&preview).unwrap();
        for field in [
            "previewId",
            "previewDigest",
            "expiresAt",
            "projects",
            "objects",
            "volumes",
            "eligibleTopologyGroupIds",
            "requiresElevation",
            "maxDeleteObjects",
            "blockedObjects",
            "archivesRetained",
        ] {
            assert!(wire.get(field).is_some(), "missing wire field {field}");
        }
        let chosen = vec![preview.eligible_topology_group_ids[0].clone()];
        let binding = final_remove_confirmation_binding(
            &conn,
            &preview.preview_id,
            &preview.preview_digest,
            chosen.clone(),
        )
        .unwrap();
        assert_eq!(binding.topology_groups, chosen);
        assert_eq!(binding.target_count, 1);
        assert!(final_remove_confirmation_binding(
            &conn,
            &preview.preview_id,
            &preview.preview_digest,
            vec!["unknown".to_string()],
        )
        .is_err());
    }

    #[test]
    fn shared_file_identity_stays_grouped_but_v013_blocks_multi_path_disposition() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "same-file-id");
        seed_entry(&conn, 2, "project:7", "same-file-id");
        let preview = build_final_remove_preview(
            &conn,
            FinalRemoveScope::Project {
                group_id: "project:7".to_string(),
            },
        )
        .unwrap();
        assert!(preview.eligible_topology_group_ids.is_empty());
        assert_eq!(preview.blocked_objects, 2);
        assert_eq!(
            preview.objects[0].topology_group_id,
            preview.objects[1].topology_group_id
        );
        assert!(preview.objects.iter().all(|item| {
            item.eligibility == "blocked"
                && item.reason_code == "externalHardlink"
                && item.reason.contains("2 held pathnames")
        }));
        let rejected = final_remove_confirmation_binding(
            &conn,
            &preview.preview_id,
            &preview.preview_digest,
            vec![preview.objects[0].topology_group_id.clone()],
        );
        assert!(rejected.is_err());
    }

    #[test]
    fn project_scope_cannot_partially_authorize_cross_project_topology() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "same-file-id");
        seed_entry(&conn, 2, "project:8", "same-file-id");
        let preview = build_final_remove_preview(
            &conn,
            FinalRemoveScope::Project {
                group_id: "project:7".to_string(),
            },
        )
        .unwrap();
        assert_eq!(preview.objects.len(), 1);
        assert_eq!(preview.objects[0].eligibility, "blocked");
        assert_eq!(preview.objects[0].reason_code, "externalHardlink");
        assert!(preview.eligible_topology_group_ids.is_empty());
        assert!(final_remove_confirmation_binding(
            &conn,
            &preview.preview_id,
            &preview.preview_digest,
            vec![preview.objects[0].topology_group_id.clone()],
        )
        .is_err());
    }

    #[test]
    fn one_blocked_topology_member_blocks_every_member_and_project_counts() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "same-file-id");
        seed_entry(&conn, 2, "project:7", "same-file-id");
        conn.execute(
            "UPDATE operation_item
             SET result_blake3 = NULL, checksum_after = NULL
             WHERE operation_id = 2",
            [],
        )
        .unwrap();
        let preview = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        assert!(preview.eligible_topology_group_ids.is_empty());
        assert_eq!(preview.blocked_objects, 2);
        assert_eq!(preview.projects[0].blocked, 2);
        assert!(preview
            .objects
            .iter()
            .all(|item| item.eligibility == "blocked"));
        assert!(preview
            .objects
            .iter()
            .any(|item| item.reason_code == "externalHardlink"));
    }

    #[test]
    fn preview_digest_binds_exact_archive_hash_and_incomplete_row_is_not_ready() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "file-1");
        seed_ready_archive(&conn, 1, &"56".repeat(32), &"78".repeat(32));
        let first = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        assert_eq!(first.objects[0].eligibility, "ready");
        conn.execute(
            "UPDATE object_archive SET archive_blake3 = ?2 WHERE quarantine_entry_id = ?1",
            params![1, "9a".repeat(32)],
        )
        .unwrap();
        let second = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        assert_ne!(first.preview_digest, second.preview_digest);
        conn.execute(
            "UPDATE object_archive SET raw_backup_blake3 = 'not-a-digest'
             WHERE quarantine_entry_id = 1",
            [],
        )
        .unwrap();
        let incomplete = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        assert_eq!(incomplete.objects[0].eligibility, "blocked");
        assert_eq!(incomplete.objects[0].reason_code, "archiveCorrupt");
        assert!(incomplete.eligible_topology_group_ids.is_empty());
    }

    #[test]
    fn preview_consumption_is_an_atomic_one_shot_cas() {
        let mut conn = conn();
        seed_entry(&conn, 1, "project:7", "file-1");
        let preview = build_final_remove_preview(&conn, FinalRemoveScope::AllEligible).unwrap();
        let now = Utc::now().to_rfc3339();
        let first = conn.transaction().unwrap();
        assert!(
            consume_preview_cas(&first, &preview.preview_id, &preview.preview_digest, &now)
                .unwrap()
        );
        first.commit().unwrap();
        let replay = conn.transaction().unwrap();
        assert!(
            !consume_preview_cas(&replay, &preview.preview_id, &preview.preview_digest, &now)
                .unwrap()
        );
        replay.rollback().unwrap();
    }

    #[test]
    fn helper_abort_terminalizes_ready_peers_without_rewriting_ambiguous_items() {
        let conn = conn();
        for id in 1..=3 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        seed_batch_item(&conn, 100, rows[0].clone(), "ready");
        seed_batch_item(&conn, 100, rows[1].clone(), "planned");
        seed_batch_item(&conn, 100, rows[2].clone(), "interrupted");

        terminalize_undisposed_batch_items(&conn, 100, "helperFailed", "synthetic chunk failure")
            .unwrap();

        let states = conn
            .prepare(
                "SELECT status, reason_code FROM permanent_delete_batch_item
                 WHERE batch_id = 100 ORDER BY quarantine_entry_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            states[0],
            ("blocked".to_string(), Some("helperFailed".to_string()))
        );
        assert_eq!(
            states[1],
            ("blocked".to_string(), Some("helperFailed".to_string()))
        );
        assert_eq!(states[2], ("interrupted".to_string(), None));
        assert!(batch_has_started_disposition(&conn, 100).unwrap());
        let operation_states = conn
            .prepare(
                "SELECT status FROM operation_item
                 WHERE operation_id = 100 ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(operation_states, vec!["skipped", "skipped", "pending"]);
    }

    #[test]
    fn topology_delete_intent_cas_rolls_back_every_member_on_one_lost_entry() {
        let conn = conn();
        for id in 1..=2 {
            seed_entry(&conn, id, "project:7", "same-file-id");
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        let first = seed_batch_item(&conn, 100, rows[0].clone(), "ready");
        let second = seed_batch_item(&conn, 100, rows[1].clone(), "ready");
        conn.execute(
            "UPDATE quarantine_entry SET status = 'restored' WHERE id = ?1",
            [second.row.entry_id],
        )
        .unwrap();

        let error = claim_delete_group_intent_journals(&conn, &[&first, &second]).unwrap_err();
        assert!(error.to_string().contains("CAS lost authority"));
        let first_entry: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [first.row.entry_id],
                |row| row.get(0),
            )
            .unwrap();
        let first_item: String = conn
            .query_row(
                "SELECT status FROM permanent_delete_batch_item WHERE id = ?1",
                [first.batch_item_id],
                |row| row.get(0),
            )
            .unwrap();
        let first_operation_item: String = conn
            .query_row(
                "SELECT status FROM operation_item WHERE id = ?1",
                [first.operation_item_id],
                |row| row.get(0),
            )
            .unwrap();
        let operation: String = conn
            .query_row("SELECT status FROM operation WHERE id = 100", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(first_entry, "quarantined");
        assert_eq!(first_item, "ready");
        assert_eq!(first_operation_item, "pending");
        assert_eq!(operation, "waiting_for_uac");
    }

    #[test]
    fn delete_intent_is_marked_unproved_until_the_success_cas() {
        let conn = conn();
        seed_entry(&conn, 1, "project:7", "one-file-id");
        seed_ready_archive(&conn, 1, &"56".repeat(32), &"78".repeat(32));
        let row = load_preview_rows(&conn).unwrap().remove(0);
        seed_batch_header(&conn, 100);
        let journal = seed_batch_item(&conn, 100, row, "ready");

        claim_delete_group_intent_journals(&conn, &[&journal]).unwrap();
        let intent: (String, String) = conn
            .query_row(
                "SELECT status, reason_code FROM permanent_delete_batch_item WHERE id = ?1",
                [journal.batch_item_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            intent,
            (
                "deleting".to_string(),
                UNPROVED_FINAL_PROFILE_REASON_CODE.to_string(),
            )
        );

        record_delete_success(
            &conn,
            journal.batch_operation_id,
            journal.batch_id,
            &journal,
        )
        .unwrap();
        let committed: (String, Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT bi.status, bi.reason_code, bi.message, qe.status
                 FROM permanent_delete_batch_item bi
                 JOIN quarantine_entry qe ON qe.id = bi.quarantine_entry_id
                 WHERE bi.id = ?1",
                [journal.batch_item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            committed,
            (
                "deleted".to_string(),
                None,
                None,
                "permanently_deleted".to_string(),
            )
        );
    }

    #[test]
    fn known_stream_profile_block_is_per_object_and_keeps_batch_partial() {
        let conn = conn();
        for id in 1..=2 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        conn.execute(
            "UPDATE permanent_delete_batch SET requested_count = 2 WHERE id = 100",
            [],
        )
        .unwrap();
        let deleted = seed_batch_item(&conn, 100, rows[0].clone(), "deleted");
        let blocked = seed_batch_item(&conn, 100, rows[1].clone(), "ready");
        conn.execute(
            "UPDATE permanent_delete_batch_item SET phase = 'finished' WHERE id = ?1",
            [deleted.batch_item_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE operation_item SET status = 'done' WHERE id = ?1",
            [deleted.operation_item_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE quarantine_entry SET status = 'permanently_deleted' WHERE id = ?1",
            [deleted.row.entry_id],
        )
        .unwrap();

        block_ready_topology_group(
            &conn,
            &[&blocked],
            UNSUPPORTED_FINAL_STREAM_PROFILE_REASON_CODE,
            UNSUPPORTED_FINAL_STREAM_PROFILE_MESSAGE,
        )
        .unwrap();

        let blocked_state: (String, String, String) = conn
            .query_row(
                "SELECT p.status, p.reason_code, o.status
                 FROM permanent_delete_batch_item p
                 JOIN operation_item o ON o.id = p.operation_item_id
                 WHERE p.id = ?1",
                [blocked.batch_item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            blocked_state,
            (
                "blocked".to_string(),
                UNSUPPORTED_FINAL_STREAM_PROFILE_REASON_CODE.to_string(),
                "skipped".to_string(),
            )
        );
        let held_state: String = conn
            .query_row(
                "SELECT status FROM quarantine_entry WHERE id = ?1",
                [blocked.row.entry_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held_state, "quarantined");

        finalize_batch(&conn, 100, 100, None).unwrap();
        let result = load_batch_result(&conn, 100).unwrap();
        assert_eq!(result.status, "partial");
        assert_eq!(result.requested_objects, 2);
        assert_eq!(result.deleted_objects, 1);
        assert_eq!(result.kept_objects, 1);
        assert_eq!(result.failed_objects, 0);
        assert_eq!(result.items[1].state, "kept");
        assert_eq!(
            result.items[1].reason_code.as_deref(),
            Some(UNSUPPORTED_FINAL_STREAM_PROFILE_REASON_CODE)
        );
    }

    #[cfg(windows)]
    #[test]
    fn bound_proof_filestreaminfo_profile_detects_visible_named_ads() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("held.bin");
        std::fs::write(&source, b"held default stream").unwrap();
        let (mut stamp, hash) = crate::inspect_local_mutation_file(&source).unwrap();
        let ads = PathBuf::from(format!("{}:codehangar-visible", source.display()));
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&ads)
            .unwrap();
        stream.write_all(b"named ADS payload").unwrap();
        stream.sync_all().unwrap();
        drop(stream);

        // Adding a named stream may update the object timestamp. The final
        // profile test is about the same volume/file-id/default bytes/hash, so
        // use the legacy-compatible optional timestamp only in this unit test.
        stamp.modified_unix_seconds = None;
        let proof = BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash).unwrap();
        assert!(proof.stream_count() > 1);
        assert!(!bound_object_matches_v013_final_stream_profile(&proof));
        drop(proof);
        assert_eq!(std::fs::read(&source).unwrap(), b"held default stream");
        assert_eq!(std::fs::read(&ads).unwrap(), b"named ADS payload");
    }

    #[cfg(windows)]
    #[test]
    fn existing_archive_parent_after_65_child_chunk_accepts_only_internal_mtime_drift() {
        use std::io::Write as _;
        use std::time::Duration;

        const CHILDREN: i64 = 65;
        const BATCH_ID: i64 = 1_000;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("held-tree");
        std::fs::create_dir(&root).unwrap();
        let child_paths = (1..=CHILDREN)
            .map(|id| root.join(format!("child-{id:03}.bin")))
            .collect::<Vec<_>>();
        for path in &child_paths {
            std::fs::write(path, b"planned child").unwrap();
        }
        let (authority_stamp, authority_hash, directory_object) =
            crate::bound_fs::inspect_local_mutation_object_for_test(&root).unwrap();
        assert!(directory_object);

        // Force the removals into a later whole-second timestamp bucket because
        // FileStamp intentionally stores portable seconds, not raw FILETIME.
        std::thread::sleep(Duration::from_millis(1_100));
        for path in &child_paths {
            std::fs::remove_file(path).unwrap();
        }
        let (after_children, _, _) =
            crate::bound_fs::inspect_local_mutation_object_for_test(&root).unwrap();
        assert!(authority_stamp.same_object(&after_children));
        assert_ne!(
            authority_stamp.modified_unix_seconds, after_children.modified_unix_seconds,
            "the real child deletions must exercise the directory-mtime exception"
        );

        let conn = conn();
        for id in 1..=CHILDREN + 1 {
            seed_entry(&conn, id, "project:tree", &format!("tree-file-{id}"));
        }
        let parent_id = CHILDREN + 1;
        seed_ready_archive(&conn, parent_id, &"56".repeat(32), &"78".repeat(32));
        seed_batch_header(&conn, BATCH_ID);
        conn.execute(
            "UPDATE permanent_delete_batch SET requested_count = ?2 WHERE id = ?1",
            params![BATCH_ID, CHILDREN + 1],
        )
        .unwrap();
        let mut rows = load_preview_rows(&conn).unwrap();
        rows.sort_by_key(|row| row.entry_id);
        for (index, mut row) in rows
            .iter()
            .filter(|row| row.entry_id <= CHILDREN)
            .cloned()
            .enumerate()
        {
            row.held_path = child_paths[index].to_string_lossy().into_owned();
            seed_batch_item(&conn, BATCH_ID, row, "deleted");
        }
        let mut parent_row = rows
            .into_iter()
            .find(|row| row.entry_id == parent_id)
            .unwrap();
        parent_row.held_path = root.to_string_lossy().into_owned();
        parent_row.result_volume_id = Some(authority_stamp.volume_id.clone());
        parent_row.result_file_id = Some(authority_stamp.file_id.clone());
        parent_row.result_bytes = Some(authority_stamp.bytes);
        parent_row.result_modified_unix_seconds = authority_stamp.modified_unix_seconds;
        parent_row.result_blake3 = Some(authority_hash.clone());
        let parent = seed_batch_item(&conn, BATCH_ID, parent_row, "ready");

        assert!(planned_descendants_were_deleted(&conn, &parent).unwrap());
        let proof = BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
            &root,
            &authority_stamp,
            &authority_hash,
        )
        .unwrap()
        .detach_exclusive_for_final_disposition()
        .unwrap();
        assert!(proof.matches_final_stream_profile());
        assert!(proof.stamp().same_object(&authority_stamp));
        drop(proof);

        conn.execute(
            "UPDATE permanent_delete_batch_item SET status = 'ready'
             WHERE batch_id = ?1 AND held_path = ?2",
            params![BATCH_ID, child_paths[0].to_string_lossy()],
        )
        .unwrap();
        assert!(planned_descendants_were_deleted(&conn, &parent).is_err());
        conn.execute(
            "UPDATE permanent_delete_batch_item SET status = 'deleted'
             WHERE batch_id = ?1 AND held_path = ?2",
            params![BATCH_ID, child_paths[0].to_string_lossy()],
        )
        .unwrap();

        let unknown = root.join("unknown.bin");
        std::fs::write(&unknown, b"not in the immutable batch").unwrap();
        let error = BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
            &root,
            &authority_stamp,
            &authority_hash,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown or undisposed child"));
        std::fs::remove_file(&unknown).unwrap();

        let ads = PathBuf::from(format!("{}:unplanned", root.display()));
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&ads)
            .unwrap();
        stream.write_all(b"unarchived directory stream").unwrap();
        stream.sync_all().unwrap();
        drop(stream);
        let error = BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
            &root,
            &authority_stamp,
            &authority_hash,
        )
        .unwrap_err();
        assert!(error.to_string().contains("stream"));
        std::fs::remove_file(&ads).unwrap();

        std::fs::remove_dir(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        let error = BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
            &root,
            &authority_stamp,
            &authority_hash,
        )
        .unwrap_err();
        assert!(error.to_string().contains("identity changed"));

        let ordinary_file = directory.path().join("not-a-directory.bin");
        std::fs::write(&ordinary_file, b"ordinary file").unwrap();
        let (file_stamp, file_hash) = crate::inspect_local_mutation_file(&ordinary_file).unwrap();
        let error = BoundObjectProof::open_for_archive_delete_allow_directory_time_drift(
            &ordinary_file,
            &file_stamp,
            &file_hash,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("directory-time drift authority resolved to a file"));
    }

    #[test]
    fn aggregate_space_blocks_volume_overcommit_that_per_item_checks_would_miss() {
        let conn = conn();
        for id in 1..=2 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        let journals = rows
            .into_iter()
            .map(|row| seed_batch_item(&conn, 100, row, "planned"))
            .collect::<Vec<_>>();
        let items = journals
            .into_iter()
            .enumerate()
            .map(|(index, journal)| {
                let source_stamp = row_stamp(&journal.row).unwrap();
                PreflightBatchItem {
                    source_blake3: journal.row.result_blake3.clone().unwrap(),
                    source_stamp,
                    scratch_path: PathBuf::from(r"C:\backup"),
                    scratch_stamp: FileStamp {
                        volume_id: "backup-volume".to_string(),
                        file_id: format!("scratch-{index}"),
                        bytes: 0,
                        modified_unix_seconds: Some(42),
                    },
                    destination_volume_id: "backup-volume".to_string(),
                    available_space_bytes: Some(250),
                    // Each object alone would need 160 and pass a 250-byte
                    // per-item check. The batch needs 100 + 100 persisted plus
                    // one sequential 60-byte scratch peak = 260.
                    persistent_archive_bytes: 100,
                    transient_peak_bytes: 60,
                    archive: PreflightArchive::New,
                    allow_existing_archive_directory_time_drift: false,
                    journal,
                }
            })
            .collect::<Vec<_>>();

        let retained = enforce_aggregate_archive_space(&conn, items).unwrap();
        assert!(retained.is_empty());
        let blocked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permanent_delete_batch_item
                 WHERE batch_id = 100 AND status = 'blocked'
                   AND reason_code = 'archiveInsufficientSpace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blocked, 2);
    }

    #[test]
    fn requested_stop_keeps_undisposed_items_and_preserves_deleted_history() {
        let conn = conn();
        for id in 1..=3 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        seed_batch_item(&conn, 100, rows[0].clone(), "deleted");
        seed_batch_item(&conn, 100, rows[1].clone(), "ready");
        seed_batch_item(&conn, 100, rows[2].clone(), "planned");

        terminalize_undisposed_batch_items(&conn, 100, "stopRequested", "synthetic owner stop")
            .unwrap();
        finalize_batch(
            &conn,
            100,
            100,
            Some(FinalRemoveInterruptionReason::OwnerStop),
        )
        .unwrap();

        let states = conn
            .prepare(
                "SELECT status, reason_code FROM permanent_delete_batch_item
                 WHERE batch_id = 100 ORDER BY quarantine_entry_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(states[0], ("deleted".to_string(), None));
        assert_eq!(
            states[1],
            ("blocked".to_string(), Some("stopRequested".to_string()))
        );
        assert_eq!(
            states[2],
            ("blocked".to_string(), Some("stopRequested".to_string()))
        );
        let result = load_batch_result(&conn, 100).unwrap();
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.deleted_objects, 1);
        assert_eq!(result.kept_objects, 2);
        assert_eq!(result.failed_objects, 0);
    }

    #[test]
    fn requested_stop_never_rewrites_ambiguous_scratch_as_cleanly_kept() {
        let conn = conn();
        for id in 1..=3 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        seed_batch_item(&conn, 100, rows[0].clone(), "deleted");
        seed_batch_item(&conn, 100, rows[1].clone(), "ready");
        let ambiguous = seed_batch_item(&conn, 100, rows[2].clone(), "interrupted");
        conn.execute(
            "UPDATE permanent_delete_batch_item
             SET phase = 'scratch_cleanup_pending', reason_code = 'scratchCleanupPending'
             WHERE id = ?1",
            [ambiguous.batch_item_id],
        )
        .unwrap();

        terminalize_undisposed_batch_items(&conn, 100, "stopRequested", "synthetic owner stop")
            .unwrap();
        finalize_batch(
            &conn,
            100,
            100,
            Some(FinalRemoveInterruptionReason::OwnerStop),
        )
        .unwrap();

        let ambiguous_state: (String, String, String) = conn
            .query_row(
                "SELECT status, phase, reason_code
                 FROM permanent_delete_batch_item WHERE id = ?1",
                [ambiguous.batch_item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            ambiguous_state,
            (
                "interrupted".to_string(),
                "scratch_cleanup_pending".to_string(),
                "scratchCleanupPending".to_string(),
            )
        );
        let result = load_batch_result(&conn, 100).unwrap();
        assert_eq!(result.status, "interrupted");
        assert_eq!(result.deleted_objects, 1);
        assert_eq!(result.kept_objects, 1);
        assert_eq!(result.failed_objects, 1);
    }

    #[test]
    fn progress_observer_failure_is_interrupted_and_never_owner_cancelled() {
        let conn = conn();
        for id in 1..=3 {
            seed_entry(&conn, id, "project:7", &format!("file-{id}"));
            seed_ready_archive(&conn, id, &"56".repeat(32), &"78".repeat(32));
        }
        let rows = load_preview_rows(&conn).unwrap();
        seed_batch_header(&conn, 100);
        seed_batch_item(&conn, 100, rows[0].clone(), "deleted");
        seed_batch_item(&conn, 100, rows[1].clone(), "ready");
        seed_batch_item(&conn, 100, rows[2].clone(), "planned");
        let reason = FinalRemoveInterruptionReason::ProgressObserverFailed;
        let (code, message, elevation_status) = interruption_details(reason);
        assert_eq!(elevation_status, "failed");

        terminalize_undisposed_batch_items(&conn, 100, code, message).unwrap();
        finalize_batch(&conn, 100, 100, Some(reason)).unwrap();

        let result = load_batch_result(&conn, 100).unwrap();
        assert_eq!(result.status, "interrupted");
        assert_ne!(result.status, "cancelled");
        assert_eq!(result.deleted_objects, 1);
        assert_eq!(result.kept_objects, 2);
        assert_eq!(result.failed_objects, 0);
        assert!(result.items[1..]
            .iter()
            .all(|item| item.state == "kept" && item.error.as_deref() == Some(message)));
        let persisted: (String, String, String) = conn
            .query_row(
                "SELECT b.status, o.status, b.error
                 FROM permanent_delete_batch b
                 JOIN operation o ON o.id = b.operation_id
                 WHERE b.id = 100",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(persisted.0, "interrupted");
        assert_eq!(persisted.1, "interrupted");
        assert_eq!(persisted.2, message);
    }

    #[test]
    fn stop_is_polled_once_per_group_and_never_splits_an_inseparable_group() {
        let polls = AtomicUsize::new(0);
        let control = || polls.fetch_add(1, Ordering::SeqCst) >= 1;
        let groups = [vec![1, 2], vec![3, 4, 5], vec![6]];
        let mut interruption_latched = None;
        let mut disposed = Vec::new();
        for group in groups {
            if interruption_before_topology_group(&mut interruption_latched, &control).is_some() {
                break;
            }
            // This is the indivisible disposition body: the control is not
            // consulted again until every member has completed.
            disposed.extend(group);
        }
        assert_eq!(disposed, vec![1, 2]);
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        assert_eq!(
            interruption_latched,
            Some(FinalRemoveInterruptionReason::OwnerStop)
        );
        assert_eq!(
            prioritize_interruption(
                interruption_latched,
                Some(FinalRemoveInterruptionReason::ProgressObserverFailed),
            ),
            Some(FinalRemoveInterruptionReason::ProgressObserverFailed)
        );
    }

    #[test]
    fn progress_contract_matches_the_existing_api_wire_vocabulary() {
        let progress = FinalRemoveBatchProgress {
            batch_id: "batch-test".to_string(),
            phase: FinalRemoveBatchPhase::ParentDisposition,
            total: 7,
            completed: 3,
            current_path: Some(r"C:\holding\object.bin".to_string()),
        };
        let wire = serde_json::to_value(&progress).unwrap();
        assert_eq!(wire["batchId"], "batch-test");
        assert_eq!(wire["phase"], "parentDisposition");
        assert_eq!(wire["total"], 7);
        assert_eq!(wire["completed"], 3);
        assert_eq!(wire["currentPath"], r"C:\holding\object.bin");
        assert_eq!(FinalRemoveBatchPhase::Interrupted.as_str(), "interrupted");
    }

    #[test]
    fn disposition_order_is_bottom_up_not_topology_id_lexical_order() {
        let touched = BTreeSet::from([
            "a-parent".to_string(),
            "z-child".to_string(),
            "m-cross-chunk".to_string(),
        ]);
        let completed = BTreeSet::new();
        let remaining = BTreeMap::from([
            ("a-parent".to_string(), 0),
            ("z-child".to_string(), 0),
            // A group split across chunks must not be disposed until its final
            // member is processed, regardless of its first global index.
            ("m-cross-chunk".to_string(), 1),
        ]);
        let members = BTreeMap::from([
            ("z-child".to_string(), vec![0]),
            ("m-cross-chunk".to_string(), vec![1, 65]),
            ("a-parent".to_string(), vec![2]),
        ]);

        assert_eq!(
            complete_group_order(&touched, &completed, &remaining, &members),
            vec!["z-child".to_string(), "a-parent".to_string()]
        );
    }
}
