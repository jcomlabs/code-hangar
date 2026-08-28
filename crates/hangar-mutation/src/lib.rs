//! Phase 3 mutation foundations: journaled backup / quarantine / restore.
//!
//! Every executable item is gated behind the `mutation` feature. With only
//! `core`, this crate is inert — it has no dependencies and no code — so the
//! strict core lane links no executor, journal, or confirmation surface from
//! here. Mutation is opt-in and off by default. Modules: the destructive-action
//! state machine and single-use confirmation tokens (control), the journal
//! schema, the verified non-destructive `backup` engine, the journaled
//! `quarantine` executor and its `restore` inverse, confirm-token-gated
//! permanent delete, and a best-effort file-lock inspector.

#[cfg(feature = "mutation")]
mod backup;
#[cfg(feature = "mutation")]
mod bound_fs;
#[cfg(feature = "mutation")]
mod confirm;
#[cfg(feature = "mutation")]
mod elevated_helper;
#[cfg(feature = "mutation")]
mod elevated_protocol;
#[cfg(feature = "mutation")]
mod elevated_transport;
#[cfg(feature = "mutation")]
mod final_remove;
#[cfg(all(feature = "mutation", test))]
mod fsops;
#[cfg(feature = "mutation")]
mod journal;
#[cfg(feature = "mutation")]
mod lock;
#[cfg(feature = "mutation")]
mod longpath;
#[cfg(feature = "mutation")]
mod object_archive;
#[cfg(feature = "mutation")]
mod purge;
#[cfg(feature = "mutation")]
mod quarantine;
#[cfg(feature = "mutation")]
mod recover;
#[cfg(feature = "mutation")]
mod restore;
#[cfg(feature = "mutation")]
mod state_machine;

#[cfg(feature = "mutation")]
pub use backup::{
    create_backup, load_verified_backup, BackupCopy, BackupError, BackupItem, BackupLevel,
    BackupRequest, BackupResult, VerifiedBackup, VerifiedBackupPayloadGuard,
};
#[cfg(feature = "mutation")]
pub use bound_fs::{
    inspect_local_mutation_file, validate_local_mutation_path, BoundObjectProof, BoundScratchRoot,
    CommittedObjectArchive, FileStamp, ObjectArchiveContainer,
};
#[cfg(feature = "mutation")]
pub use confirm::{ConfirmAction, ConfirmIssueError, ConfirmTokenStore, ConfirmationBinding};
#[cfg(feature = "mutation")]
pub use elevated_helper::{
    enable_object_backup_privileges, PrivilegeGuard, PrivilegeProof, PRIVILEGE_BACKUP,
    PRIVILEGE_RESTORE, PRIVILEGE_SECURITY, REQUIRED_OBJECT_PRIVILEGES,
};
#[cfg(feature = "mutation")]
pub use elevated_protocol::{
    archive_path_for_capability, decode_authenticated, encode_authenticated,
    scratch_leaf_for_capability, ElevatedCapability, ElevatedFailure, ElevatedItemResult,
    ElevatedObjectResult, ElevatedRequest, ElevatedResponse, ElevatedSuccess, ExpectedObject,
    ExpectedScratchRoot, FrameContext, FrameRole, ParentBinding, FRAME_HEADER_BYTES,
    FRAME_MAC_BYTES, FRAME_MIN_DECLARED_BYTES, FRAME_PREFIX_BYTES, FRAME_WIRE_VERSION,
    MAX_CAPABILITIES_PER_INVOCATION, MAX_CAPABILITY_LIFETIME_SECONDS, MAX_FRAME_BYTES,
    OBJECT_ARCHIVE_DIRECTORY_NAME, PROTOCOL_SCHEMA,
};
#[cfg(feature = "mutation")]
pub use elevated_transport::{
    current_parent_binding, invoke_elevated_helper, run_elevated_helper_cli,
    verify_release_installation, ElevatedTransportError, ReleaseInstallationVerification,
    MAX_RESIDENT_CAPABILITIES_PER_INVOCATION, RELEASE_IDENTITY_REQUIREMENT,
    RELEASE_MANIFEST_FILE_NAME, RELEASE_MANIFEST_SCHEMA,
};
#[cfg(feature = "mutation")]
pub use final_remove::{
    build_final_remove_preview, execute_final_remove_batch, execute_final_remove_batch_controlled,
    final_remove_confirmation_binding, FinalRemoveBatchControl, FinalRemoveBatchItemResult,
    FinalRemoveBatchObserver, FinalRemoveBatchPhase, FinalRemoveBatchProgress,
    FinalRemoveBatchResult, FinalRemoveBlockedSubtree, FinalRemoveError,
    FinalRemoveInterruptionReason, FinalRemoveObjectDecision, FinalRemovePreview,
    FinalRemoveProjectPreview, FinalRemoveProjectResult, FinalRemoveScope, FinalRemoveVolumeImpact,
    OBJECT_ARCHIVE_PROOF_SCHEMA,
};
#[cfg(feature = "mutation")]
pub use journal::{ensure_journal_schema, JournalError};
#[cfg(feature = "mutation")]
pub use lock::{inspect_lock, LockState};
#[cfg(feature = "mutation")]
pub use object_archive::{
    finalize_object_archive_v2, verify_object_archive_v2, FinalizeObjectArchiveParams,
    ObjectArchiveError, ObjectArchiveProof, VerifyObjectArchiveParams,
};
#[cfg(feature = "mutation")]
pub use purge::{permanent_delete_entry, PurgeError, PurgeOutcome};
#[cfg(feature = "mutation")]
pub use quarantine::{
    quarantine, ItemOutcome, QuarantineEntryResult, QuarantineError, QuarantineItem,
    QuarantineRequest, QuarantineResult,
};
#[cfg(feature = "mutation")]
pub use recover::{recover_interrupted, RecoveryError, RecoveryReport};
#[cfg(feature = "mutation")]
pub use restore::{restore_entry, restore_entry_to_folder, RestoreError, RestoreOutcome};
#[cfg(feature = "mutation")]
pub use state_machine::{OperationState, StateError};

/// Returns true. Used by `hangar-api` (under its `mutation` feature) to prove
/// the optional dependency chain links, without exposing any executor surface.
#[cfg(feature = "mutation")]
pub fn mutation_foundations_linked() -> bool {
    OperationState::Draft.can_transition_to(OperationState::Reviewed)
}

/// Cloud Files are never mutation inputs. Even `cloud_local` (fully materialized)
/// remains provider-backed and a local unlink can propagate remotely.
#[cfg(all(feature = "mutation", test))]
pub(crate) fn is_cloud_reparse_kind(kind: Option<&str>) -> bool {
    matches!(kind, Some("cloud_local" | "cloud_placeholder"))
}
