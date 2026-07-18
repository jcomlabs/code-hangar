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
mod confirm;
#[cfg(feature = "mutation")]
mod fsops;
#[cfg(feature = "mutation")]
mod journal;
#[cfg(feature = "mutation")]
mod lock;
#[cfg(feature = "mutation")]
mod longpath;
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
    create_backup, file_blake3, load_verified_backup, BackupCopy, BackupError, BackupItem,
    BackupLevel, BackupRequest, BackupResult, VerifiedBackup,
};
#[cfg(feature = "mutation")]
pub use confirm::{ConfirmAction, ConfirmTokenStore};
#[cfg(feature = "mutation")]
pub use journal::{ensure_journal_schema, JournalError};
#[cfg(feature = "mutation")]
pub use lock::{inspect_lock, LockState};
#[cfg(feature = "mutation")]
pub use purge::{permanent_delete_entry, PurgeError, PurgeOutcome};
#[cfg(feature = "mutation")]
pub use quarantine::{
    quarantine, remove_reparse_link, ItemOutcome, QuarantineEntryResult, QuarantineError,
    QuarantineItem, QuarantineRequest, QuarantineResult,
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
